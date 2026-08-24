#!/usr/bin/env python3
"""Task-scoped ACP controller for the Grok executor skill.

The controller owns one fresh Grok ACP session for one bounded task.  It sends
the initial task prompt, keeps the process alive for follow-up turns, and reads
newline-delimited JSON control commands from stdin.
"""

from __future__ import annotations

import argparse
import collections
import json
import os
import signal
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, TextIO

MAX_DIAGNOSTIC_BYTES = 16 * 1024 * 1024
MAX_SUMMARY_BYTES = 4096
MAX_TURN_BUFFER_CHARS = 64 * 1024


class RpcError(RuntimeError):
    def __init__(self, method: str, error: Any):
        self.method = method
        self.error = error
        if isinstance(error, dict):
            self.code = error.get("code")
            detail = error.get("message", json.dumps(error, sort_keys=True))
        else:
            self.code = None
            detail = str(error)
        super().__init__(f"{method}: {detail}")


@dataclass
class PendingRequest:
    method: str
    event: threading.Event = field(default_factory=threading.Event)
    result: Any = None
    error: Any = None

    def wait(self, timeout: float | None = None) -> Any:
        if not self.event.wait(timeout):
            raise TimeoutError(f"{self.method} timed out")
        if self.error is not None:
            if isinstance(self.error, BaseException):
                raise self.error
            raise RpcError(self.method, self.error)
        return self.result


class AcpConnection:
    def __init__(
        self,
        command: list[str],
        cwd: Path,
        on_notification: Callable[[dict[str, Any]], None],
        on_agent_request: Callable[[dict[str, Any]], Any],
        on_wire_message: Callable[[str, dict[str, Any]], None],
        on_stderr: Callable[[str], None],
    ) -> None:
        self._on_notification = on_notification
        self._on_agent_request = on_agent_request
        self._on_wire_message = on_wire_message
        self._on_stderr = on_stderr
        self._write_lock = threading.Lock()
        self._pending_lock = threading.Lock()
        self._pending: dict[int, PendingRequest] = {}
        self._next_id = 1
        self._closed = threading.Event()
        self.process = subprocess.Popen(
            command,
            cwd=cwd,
            env=os.environ.copy(),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
            start_new_session=True,
        )
        if (
            self.process.stdin is None
            or self.process.stdout is None
            or self.process.stderr is None
        ):
            raise RuntimeError("failed to create Grok ACP pipes")
        self._stdout_thread = threading.Thread(target=self._read_stdout, daemon=True)
        self._stderr_thread = threading.Thread(target=self._read_stderr, daemon=True)
        self._stdout_thread.start()
        self._stderr_thread.start()

    @property
    def alive(self) -> bool:
        return self.process.poll() is None and not self._closed.is_set()

    def request(self, method: str, params: dict[str, Any]) -> PendingRequest:
        with self._pending_lock:
            request_id = self._next_id
            self._next_id += 1
            pending = PendingRequest(method)
            self._pending[request_id] = pending
        self._send(
            {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        )
        return pending

    def notify(self, method: str, params: dict[str, Any]) -> None:
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def respond(self, request_id: Any, result: Any = None, error: Any = None) -> None:
        message: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id}
        if error is not None:
            message["error"] = error
        else:
            message["result"] = result if result is not None else {}
        self._send(message)

    def _send(self, message: dict[str, Any]) -> None:
        if not self.alive:
            raise RuntimeError("Grok ACP process is not running")
        self._on_wire_message("outbound", message)
        line = json.dumps(message, separators=(",", ":"), ensure_ascii=False)
        with self._write_lock:
            assert self.process.stdin is not None
            self.process.stdin.write(line + "\n")
            self.process.stdin.flush()

    def _read_stdout(self) -> None:
        assert self.process.stdout is not None
        try:
            for raw_line in self.process.stdout:
                line = raw_line.strip()
                if not line:
                    continue
                try:
                    message = json.loads(line)
                except json.JSONDecodeError:
                    self._on_stderr(
                        f"GROK_ACP_PROTOCOL_ERROR non-JSON stdout: {line[:500]}\n"
                    )
                    continue
                self._on_wire_message("inbound", message)
                if "method" in message and "id" in message:
                    try:
                        result = self._on_agent_request(message)
                        self.respond(message["id"], result=result)
                    except (OSError, RuntimeError, TypeError, ValueError) as exc:
                        self.respond(
                            message["id"],
                            error={
                                "code": -32601,
                                "message": f"unsupported client request: {exc}",
                            },
                        )
                    continue
                if "method" in message:
                    self._on_notification(message)
                    continue
                request_id = message.get("id")
                with self._pending_lock:
                    pending = self._pending.pop(request_id, None)
                if pending is None:
                    continue
                pending.result = message.get("result")
                pending.error = message.get("error")
                pending.event.set()
        finally:
            self._closed.set()
            exit_code = self.process.poll()
            error = RuntimeError(
                f"Grok ACP process exited unexpectedly (status={exit_code})"
            )
            with self._pending_lock:
                pending_requests = list(self._pending.values())
                self._pending.clear()
            for pending in pending_requests:
                pending.error = error
                pending.event.set()

    def _read_stderr(self) -> None:
        assert self.process.stderr is not None
        for line in self.process.stderr:
            self._on_stderr(line)

    def close(self, grace_seconds: float = 5.0) -> int:
        status: int
        if self.process.poll() is None:
            try:
                assert self.process.stdin is not None
                self.process.stdin.close()
            except OSError:
                pass
            try:
                status = self.process.wait(timeout=grace_seconds)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                try:
                    status = self.process.wait(timeout=grace_seconds)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    status = self.process.wait()
        else:
            status = self.process.wait()
        self._stdout_thread.join(timeout=1)
        self._stderr_thread.join(timeout=1)
        return status


class Renderer:
    def __init__(
        self,
        output_format: str,
        stdout: TextIO,
        stderr: TextIO,
        diagnostic_file: Path,
    ) -> None:
        self.output_format = output_format
        self.stdout = stdout
        self.stderr = stderr
        self.diagnostic_file = diagnostic_file
        diagnostic_file.parent.mkdir(parents=True, exist_ok=True)
        self._diagnostic = diagnostic_file.open("w", encoding="utf-8")
        os.chmod(diagnostic_file, 0o600)
        self._output_lock = threading.Lock()
        self._state_lock = threading.Lock()
        self._diagnostic_lock = threading.Lock()
        self._diagnostic_bytes = 0
        self._diagnostic_truncated = False
        self._last_plain_was_newline = True
        self._turn = 0
        self._turn_started_at: float | None = None
        self._message_chunks: collections.deque[str] = collections.deque()
        self._message_chars = 0
        self._tool_calls = 0
        self._last_tool: str | None = None

    def diagnostic(self, kind: str, **payload: Any) -> None:
        record = (
            json.dumps(
                {"type": "diagnostic", "kind": kind, **payload},
                ensure_ascii=False,
                separators=(",", ":"),
            )
            + "\n"
        )
        encoded_size = len(record.encode("utf-8"))
        with self._diagnostic_lock:
            if self._diagnostic.closed or self._diagnostic_truncated:
                return
            if self._diagnostic_bytes + encoded_size > MAX_DIAGNOSTIC_BYTES:
                marker = (
                    json.dumps(
                        {"type": "diagnostic", "kind": "truncated"},
                        separators=(",", ":"),
                    )
                    + "\n"
                )
                marker_size = len(marker.encode("utf-8"))
                if self._diagnostic_bytes + marker_size <= MAX_DIAGNOSTIC_BYTES:
                    self._diagnostic.write(marker)
                    self._diagnostic.flush()
                    self._diagnostic_bytes += marker_size
                self._diagnostic_truncated = True
                return
            self._diagnostic.write(record)
            self._diagnostic.flush()
            self._diagnostic_bytes += encoded_size

    def wire_message(self, direction: str, message: dict[str, Any]) -> None:
        self.diagnostic("acp", direction=direction, message=message)

    def stderr_line(self, line: str) -> None:
        self.diagnostic("stderr", text=line)
        if self.output_format == "plain":
            with self._output_lock:
                self.stderr.write(line)
                self.stderr.flush()
        elif self.output_format == "streaming-json":
            with self._output_lock:
                self.stdout.write(
                    json.dumps({"type": "acp_stderr", "text": line}, ensure_ascii=False)
                    + "\n"
                )
                self.stdout.flush()

    def start_turn(self, turn: int) -> None:
        with self._state_lock:
            self._turn = turn
            self._turn_started_at = time.monotonic()
            self._message_chunks.clear()
            self._message_chars = 0
            self._tool_calls = 0
            self._last_tool = None

    def _append_message(self, text: str) -> None:
        with self._state_lock:
            if len(text) >= MAX_TURN_BUFFER_CHARS:
                self._message_chunks.clear()
                self._message_chunks.append(text[-MAX_TURN_BUFFER_CHARS:])
                self._message_chars = MAX_TURN_BUFFER_CHARS
                return
            self._message_chunks.append(text)
            self._message_chars += len(text)
            while (
                self._message_chars > MAX_TURN_BUFFER_CHARS
                and len(self._message_chunks) > 1
            ):
                removed = self._message_chunks.popleft()
                self._message_chars -= len(removed)
            if self._message_chars > MAX_TURN_BUFFER_CHARS:
                excess = self._message_chars - MAX_TURN_BUFFER_CHARS
                first = self._message_chunks.popleft()
                self._message_chunks.appendleft(first[excess:])
                self._message_chars = MAX_TURN_BUFFER_CHARS

    @staticmethod
    def _bounded_summary(text: str) -> str:
        text = text.strip()
        status_index = text.rfind("STATUS:")
        if status_index >= 0:
            text = text[status_index:]
        encoded = text.encode("utf-8")
        if len(encoded) <= MAX_SUMMARY_BYTES:
            return text
        marker = "...[truncated]\n"
        tail_bytes = MAX_SUMMARY_BYTES - len(marker.encode("utf-8"))
        tail = encoded[-tail_bytes:].decode("utf-8", errors="ignore")
        return marker + tail

    def finish_turn(
        self,
        turn: int,
        source: str,
        stop_reason: str,
        error: str | None,
    ) -> None:
        with self._state_lock:
            summary = self._bounded_summary("".join(self._message_chunks))
            elapsed = (
                round(time.monotonic() - self._turn_started_at, 3)
                if self._turn_started_at is not None
                else 0.0
            )
            payload: dict[str, Any] = {
                "turn": turn,
                "source": source,
                "stopReason": stop_reason,
                "summary": summary,
                "toolCalls": self._tool_calls,
                "elapsedSec": elapsed,
            }
            if self._last_tool is not None:
                payload["lastTool"] = self._last_tool
            if error is not None:
                payload["error"] = self._bounded_summary(error)
        self.event("RESULT", **payload)

    def progress(self) -> dict[str, Any]:
        with self._state_lock:
            elapsed = (
                round(time.monotonic() - self._turn_started_at, 3)
                if self._turn_started_at is not None
                else 0.0
            )
            progress: dict[str, Any] = {
                "elapsedSec": elapsed,
                "toolCalls": self._tool_calls,
            }
            if self._last_tool is not None:
                progress["lastTool"] = self._last_tool
            return progress

    def event(self, event: str, **payload: Any) -> None:
        self.diagnostic("controller", event=event, **payload)
        with self._output_lock:
            if self.output_format in {"summary", "plain"}:
                if not self._last_plain_was_newline:
                    self.stdout.write("\n")
                suffix = " " + json.dumps(payload, sort_keys=True) if payload else ""
                self.stdout.write(f"GROK_ACP_{event}{suffix}\n")
                self._last_plain_was_newline = True
            else:
                self.stdout.write(
                    json.dumps(
                        {"type": "controller", "event": event, **payload},
                        ensure_ascii=False,
                    )
                    + "\n"
                )
            self.stdout.flush()

    def notification(self, message: dict[str, Any]) -> None:
        if self.output_format == "streaming-json":
            with self._output_lock:
                self.stdout.write(
                    json.dumps({"type": "acp", "message": message}, ensure_ascii=False)
                    + "\n"
                )
                self.stdout.flush()
        if message.get("method") != "session/update":
            return
        update = message.get("params", {}).get("update", {})
        update_type = update.get("sessionUpdate")
        if update_type == "agent_message_chunk":
            text = update.get("content", {}).get("text")
            if not isinstance(text, str) or not text:
                return
            self._append_message(text)
            if self.output_format == "plain":
                with self._output_lock:
                    self.stdout.write(text)
                    self.stdout.flush()
                    self._last_plain_was_newline = text.endswith("\n")
        elif update_type == "tool_call":
            title = str(update.get("title", "tool"))[:500]
            with self._state_lock:
                self._tool_calls += 1
                self._last_tool = title
            if self.output_format == "plain":
                with self._output_lock:
                    if not self._last_plain_was_newline:
                        self.stdout.write("\n")
                    self.stdout.write(f"[grok tool] {title}\n")
                    self.stdout.flush()
                    self._last_plain_was_newline = True

    def close(self) -> None:
        with self._diagnostic_lock:
            if not self._diagnostic.closed:
                self._diagnostic.close()


class TaskController:
    def __init__(
        self,
        command: list[str],
        cwd: Path,
        renderer: Renderer,
    ) -> None:
        self.cwd = cwd
        self.renderer = renderer
        self.session_id = ""
        self._state_lock = threading.Lock()
        self._active = False
        self._closed = False
        self._turn = 0
        self._queue: collections.deque[tuple[str, str]] = collections.deque()
        self._idle = threading.Event()
        self._idle.set()
        self.connection = AcpConnection(
            command=command,
            cwd=cwd,
            on_notification=self._on_notification,
            on_agent_request=self._on_agent_request,
            on_wire_message=renderer.wire_message,
            on_stderr=renderer.stderr_line,
        )

    def initialize(self) -> None:
        init = self.connection.request(
            "initialize",
            {
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": {"readTextFile": False, "writeTextFile": False},
                    "terminal": False,
                },
                "clientInfo": {"name": "codex-grok-executor", "version": "1"},
            },
        ).wait(30)
        auth_methods = init.get("authMethods", []) if isinstance(init, dict) else []
        auth_ids = {
            method.get("id") if isinstance(method, dict) else method
            for method in auth_methods
        }
        if "cached_token" in auth_ids:
            self.connection.request(
                "authenticate",
                {"methodId": "cached_token", "_meta": {"headless": True}},
            ).wait(60)
        elif "xai.api_key" in auth_ids and os.environ.get("XAI_API_KEY"):
            self.connection.request(
                "authenticate", {"methodId": "xai.api_key", "_meta": {"headless": True}}
            ).wait(60)
        elif auth_ids:
            raise RuntimeError(
                "Grok ACP has no non-interactive cached authentication method"
            )

        created = self.connection.request(
            "session/new",
            {"cwd": str(self.cwd), "mcpServers": [], "_meta": {"yoloMode": True}},
        ).wait(60)
        if not isinstance(created, dict) or not isinstance(
            created.get("sessionId"), str
        ):
            raise TypeError("session/new did not return a sessionId")
        self.session_id = created["sessionId"]
        self.renderer.event(
            "SESSION",
            sessionId=self.session_id,
            diagnosticFile=str(self.renderer.diagnostic_file),
        )

    def _on_notification(self, message: dict[str, Any]) -> None:
        self.renderer.notification(message)

    def _on_agent_request(self, message: dict[str, Any]) -> Any:
        method = message.get("method")
        if method == "session/request_permission":
            self.renderer.event(
                "PERMISSION_DENIED", reason="unexpected ACP permission request"
            )
            return {"outcome": {"outcome": "cancelled"}}
        raise RuntimeError(str(method))

    def submit(self, text: str, source: str = "prompt") -> None:
        text = text.strip()
        if not text:
            raise ValueError("prompt text is empty")
        with self._state_lock:
            if self._closed:
                raise RuntimeError("controller is closed")
            if self._active:
                self._queue.append((source, text))
                position = len(self._queue)
                self.renderer.event("QUEUED", position=position, source=source)
                return
            self._start_prompt_locked(text, source)

    def _start_prompt_locked(self, text: str, source: str) -> None:
        self._active = True
        self._turn += 1
        turn = self._turn
        self._idle.clear()
        self.renderer.start_turn(turn)
        self.renderer.event("TURN_STARTED", turn=turn, source=source)
        threading.Thread(
            target=self._run_prompt, args=(text, source, turn), daemon=True
        ).start()

    def _run_prompt(self, text: str, source: str, turn: int) -> None:
        stop_reason = "error"
        error_text: str | None = None
        try:
            result = self.connection.request(
                "session/prompt",
                {
                    "sessionId": self.session_id,
                    "prompt": [{"type": "text", "text": text}],
                },
            ).wait(None)
            if isinstance(result, dict):
                stop_reason = str(result.get("stopReason", "unknown"))
            else:
                stop_reason = "unknown"
        except (OSError, RuntimeError) as exc:
            error_text = str(exc)

        self.renderer.finish_turn(turn, source, stop_reason, error_text)

        next_prompt: tuple[str, str] | None = None
        with self._state_lock:
            self._active = False
            if self._queue and not self._closed and self.connection.alive:
                next_prompt = self._queue.popleft()
            else:
                self._idle.set()

        if error_text is not None:
            self.renderer.event(
                "TURN_ERROR", turn=turn, source=source, error=error_text
            )
        self.renderer.event(
            "IDLE", turn=turn, stopReason=stop_reason, queued=len(self._queue)
        )

        if next_prompt is not None:
            next_source, next_text = next_prompt
            with self._state_lock:
                if not self._closed:
                    self._start_prompt_locked(next_text, next_source)

    def interject(self, text: str) -> None:
        text = text.strip()
        if not text:
            raise ValueError("interjection text is empty")
        with self._state_lock:
            active = self._active
        if not active:
            self.submit(text, source="interject-idle")
            return

        def finish_interject() -> None:
            for method in ("x.ai/interject", "_x.ai/interject"):
                pending = self.connection.request(
                    method, {"sessionId": self.session_id, "text": text}
                )
                try:
                    pending.wait(15)
                    self.renderer.event("INTERJECTED", method=method)
                    return
                except RpcError as exc:
                    if exc.code == -32601:
                        continue
                    self.renderer.event("INTERJECT_ERROR", error=str(exc))
                    return
                except TimeoutError:
                    break
                except (OSError, RuntimeError) as exc:
                    self.renderer.event("INTERJECT_ERROR", error=str(exc))
                    return
            self._fallback_interject(text)

        threading.Thread(target=finish_interject, daemon=True).start()

    def _fallback_interject(self, text: str) -> None:
        with self._state_lock:
            if self._closed:
                return
            if self._active:
                self._queue.appendleft(("interject-fallback", text))
                self.connection.notify("session/cancel", {"sessionId": self.session_id})
                self.renderer.event("INTERJECT_FALLBACK", action="cancel_then_prompt")
                return
        self.submit(text, source="interject-fallback")

    def cancel(self) -> None:
        with self._state_lock:
            active = self._active
        if active:
            self.connection.notify("session/cancel", {"sessionId": self.session_id})
            self.renderer.event("CANCEL_SENT")
        else:
            self.renderer.event("CANCEL_SKIPPED", reason="idle")

    def status(self) -> None:
        with self._state_lock:
            payload = {
                "sessionId": self.session_id,
                "active": self._active,
                "queued": len(self._queue),
                "turn": self._turn,
            }
        self.renderer.event("STATUS", **payload, **self.renderer.progress())

    def wait_idle(self, timeout: float | None = None) -> bool:
        return self._idle.wait(timeout)

    def close(self) -> int:
        with self._state_lock:
            if self._closed:
                return self.connection.process.poll() or 0
            self._closed = True
            active = self._active
            self._queue.clear()
        if active and self.connection.alive:
            try:
                self.connection.notify("session/cancel", {"sessionId": self.session_id})
            except RuntimeError:
                pass
            self.wait_idle(10)
        status = self.connection.close()
        self.renderer.event("CLOSED", sessionId=self.session_id, status=status)
        self.renderer.close()
        return status


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cwd", required=True, type=Path)
    parser.add_argument("--prompt-file", required=True, type=Path)
    parser.add_argument("--diagnostic-file", required=True, type=Path)
    parser.add_argument(
        "--output-format",
        choices=("summary", "plain", "json", "streaming-json"),
        default="summary",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.command and args.command[0] == "--":
        args.command = args.command[1:]
    if not args.command:
        parser.error("missing Grok agent command after --")
    return args


def read_prompt_file(path: Path) -> str:
    if not path.is_file():
        raise ValueError(f"prompt file is not a regular file: {path}")
    if path.stat().st_size > 1024 * 1024:
        raise ValueError(f"prompt file exceeds 1 MiB: {path}")
    text = path.read_text(encoding="utf-8")
    if "\x00" in text:
        raise ValueError("prompt file contains a NUL byte")
    if not text.strip():
        raise ValueError("prompt file is empty")
    return text


def parse_control(line: str) -> dict[str, Any]:
    try:
        command = json.loads(line)
    except json.JSONDecodeError as exc:
        raise ValueError(
            f"control input must be one JSON object per line: {exc}"
        ) from exc
    if not isinstance(command, dict) or not isinstance(command.get("type"), str):
        raise TypeError("control input requires a string 'type'")
    return command


def main() -> int:
    args = parse_args()
    cwd = args.cwd.resolve(strict=True)
    initial_prompt = read_prompt_file(args.prompt_file)
    renderer = Renderer(
        args.output_format, sys.stdout, sys.stderr, args.diagnostic_file
    )
    controller = TaskController(args.command, cwd, renderer)

    shutting_down = threading.Event()

    def handle_signal(_signum: int, _frame: Any) -> None:
        shutting_down.set()
        try:
            controller.cancel()
        except RuntimeError as exc:
            renderer.event("CANCEL_ERROR", error=str(exc))
        raise KeyboardInterrupt

    signal.signal(signal.SIGINT, handle_signal)
    signal.signal(signal.SIGTERM, handle_signal)

    controller_status = 0
    try:
        controller.initialize()
        controller.submit(initial_prompt, source="initial")

        while not shutting_down.is_set():
            line = sys.stdin.readline()
            if line == "":
                controller.wait_idle(None)
                break
            if not line.strip():
                continue
            try:
                command = parse_control(line)
                command_type = command["type"]
                if command_type == "prompt":
                    controller.submit(str(command.get("text", "")), source="followup")
                elif command_type == "prompt_file":
                    controller.submit(
                        read_prompt_file(Path(str(command.get("path", "")))),
                        source="followup-file",
                    )
                elif command_type == "interject":
                    controller.interject(str(command.get("text", "")))
                elif command_type == "cancel":
                    controller.cancel()
                elif command_type == "status":
                    controller.status()
                elif command_type == "close":
                    break
                else:
                    raise ValueError(f"unknown control type: {command_type}")
            except (OSError, RuntimeError, TypeError, ValueError) as exc:
                renderer.event("CONTROL_ERROR", error=str(exc))
    except KeyboardInterrupt:
        controller_status = 130
        renderer.event("INTERRUPTED")
    except (OSError, RuntimeError, TypeError, ValueError) as exc:
        controller_status = 1
        renderer.event("FATAL", error=str(exc))
    finally:
        close_status = controller.close()
    return controller_status if controller_status != 0 else close_status


if __name__ == "__main__":
    raise SystemExit(main())
