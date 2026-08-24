from __future__ import annotations

import codecs
import json
import os
import selectors
import signal
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

SKILL_ROOT = Path(__file__).resolve().parents[1]
WRAPPER = SKILL_ROOT / "scripts" / "run-grok-executor.sh"
FAKE_GROK = Path(__file__).with_name("fake-grok-acp.py")


class RunningWrapper:
    def __init__(
        self,
        root: Path,
        *,
        no_interject: bool = False,
        large_chars: int = 0,
    ) -> None:
        self.root = root
        self.log_path = root / "fake.log"
        self.prompt_path = root / "prompt.md"
        self.prompt_path.write_text("initial task", encoding="utf-8")
        env = os.environ.copy()
        env["GROK_EXECUTOR_GROK_BIN"] = str(FAKE_GROK)
        env["FAKE_GROK_LOG"] = str(self.log_path)
        env["FAKE_GROK_PROMPT_DELAY"] = "0.35"
        if no_interject:
            env["FAKE_GROK_NO_INTERJECT"] = "1"
        if large_chars:
            env["FAKE_GROK_LARGE_CHARS"] = str(large_chars)
        self.proc = subprocess.Popen(
            [
                str(WRAPPER),
                "--execute",
                "--cwd",
                str(root),
                "--prompt-file",
                str(self.prompt_path),
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            env=env,
            start_new_session=True,
        )
        assert self.proc.stdin is not None
        assert self.proc.stdout is not None
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.proc.stdout, selectors.EVENT_READ)
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.output = ""
        self.stderr_output = ""
        self.closed = False

    def send(self, command: dict[str, object]) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(command) + "\n")
        self.proc.stdin.flush()

    def _read_stdout(self, fileobj: object) -> None:
        chunk = os.read(fileobj.fileno(), 4096)  # type: ignore[attr-defined]
        if chunk:
            self.output += self.decoder.decode(chunk)

    def wait_for(self, needle: str, timeout: float = 10.0) -> str:
        deadline = time.monotonic() + timeout
        while needle not in self.output:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                stderr = self.proc.stderr.read() if self.proc.poll() is not None else ""
                self.close(force=True)
                raise AssertionError(
                    f"timed out waiting for {needle!r}\nstdout={self.output}\nstderr={stderr}"
                )
            events = self.selector.select(min(remaining, 0.2))
            for key, _ in events:
                self._read_stdout(key.fileobj)
        return self.output

    def wait_for_count(self, needle: str, count: int, timeout: float = 10.0) -> str:
        deadline = time.monotonic() + timeout
        while self.output.count(needle) < count:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                self.close(force=True)
                raise AssertionError(
                    f"timed out waiting for {count} occurrences of {needle!r}\n"
                    f"stdout={self.output}\nstderr={self.stderr_output}"
                )
            events = self.selector.select(min(remaining, 0.2))
            for key, _ in events:
                self._read_stdout(key.fileobj)
        return self.output

    def collect_for(self, seconds: float) -> str:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            remaining = max(0.0, deadline - time.monotonic())
            events = self.selector.select(min(remaining, 0.05))
            for key, _ in events:
                self._read_stdout(key.fileobj)
        return self.output

    def event_payloads(self, event: str) -> list[dict[str, object]]:
        prefix = f"GROK_ACP_{event} "
        return [
            json.loads(line.removeprefix(prefix))
            for line in self.output.splitlines(keepends=True)
            if line.startswith(prefix) and line.endswith("\n")
        ]

    def wait_for_event(
        self, event: str, count: int = 1, timeout: float = 10.0
    ) -> dict[str, object]:
        deadline = time.monotonic() + timeout
        while len(payloads := self.event_payloads(event)) < count:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                self.close(force=True)
                raise AssertionError(
                    f"timed out waiting for {count} complete {event} events\n"
                    f"stdout={self.output}\nstderr={self.stderr_output}"
                )
            for key, _ in self.selector.select(min(remaining, 0.2)):
                self._read_stdout(key.fileobj)
        return payloads[-1]

    def close(self, force: bool = False) -> int:
        if self.closed:
            return self.proc.returncode or 0
        if self.proc.poll() is None:
            try:
                self.send({"type": "close"})
            except (BrokenPipeError, OSError):
                pass
        try:
            status = self.proc.wait(timeout=2 if force else 5)
        except subprocess.TimeoutExpired:
            os.killpg(self.proc.pid, signal.SIGKILL)
            status = self.proc.wait(timeout=5)
        assert self.proc.stderr is not None
        self.stderr_output += self.proc.stderr.read()
        self.selector.close()
        for stream in (self.proc.stdin, self.proc.stdout, self.proc.stderr):
            if stream is not None and not stream.closed:
                stream.close()
        self.closed = True
        return status

    def log_messages(self) -> list[dict[str, object]]:
        return [
            json.loads(line)
            for line in self.log_path.read_text(encoding="utf-8").splitlines()
        ]


class GrokAcpExecutorTests(unittest.TestCase):
    def test_same_session_accepts_interject_and_followup(self) -> None:
        with tempfile.TemporaryDirectory(prefix="grok-acp-test-") as tmp:
            running = RunningWrapper(Path(tmp))
            try:
                running.wait_for("GROK_ACP_TURN_STARTED")
                running.collect_for(0.1)
                self.assertNotIn("turn:initial task", running.output)
                session = running.event_payloads("SESSION")[0]
                diagnostic = Path(str(session["diagnosticFile"]))
                self.assertTrue(diagnostic.is_file())
                self.assertEqual(diagnostic.stat().st_mode & 0o777, 0o600)
                running.send({"type": "prompt", "text": "verify the fix"})
                running.wait_for("GROK_ACP_QUEUED")
                running.send({"type": "interject", "text": "steer now"})
                running.wait_for("GROK_ACP_INTERJECTED")
                running.wait_for_count("GROK_ACP_RESULT", 2)
                self.assertIn("turn:verify the fix", running.output)
                self.assertNotIn("[grok tool]", running.output)
                diagnostic_text = diagnostic.read_text(encoding="utf-8")
                self.assertIn("FAKE_GROK_STDERR_MARKER", diagnostic_text)
                self.assertIn("agent_message_chunk", diagnostic_text)
                self.assertIn("fake diagnostic tool", diagnostic_text)
                self.assertEqual(running.close(), 0)
                self.assertNotIn("FAKE_GROK_STDERR_MARKER", running.stderr_output)
                self.assertFalse(diagnostic.exists())
            finally:
                running.close(force=True)

            messages = running.log_messages()
            rpcs = [entry["message"] for entry in messages if entry["kind"] == "rpc"]
            self.assertEqual(sum(msg.get("method") == "session/new" for msg in rpcs), 1)
            prompts = [msg for msg in rpcs if msg.get("method") == "session/prompt"]
            self.assertEqual(len(prompts), 2)
            self.assertTrue(
                all(
                    msg["params"]["sessionId"] == prompts[0]["params"]["sessionId"]
                    for msg in prompts
                )
            )
            self.assertEqual(
                sum(msg.get("method") == "x.ai/interject" for msg in rpcs), 1
            )
            argv = next(entry["argv"] for entry in messages if entry["kind"] == "argv")
            self.assertIn("workspace", argv)

    def test_unsupported_interject_cancels_then_prompts_same_session(self) -> None:
        with tempfile.TemporaryDirectory(prefix="grok-acp-test-") as tmp:
            running = RunningWrapper(Path(tmp), no_interject=True)
            try:
                running.wait_for("GROK_ACP_TURN_STARTED")
                running.send({"type": "interject", "text": "replacement direction"})
                running.wait_for("GROK_ACP_INTERJECT_FALLBACK")
                running.wait_for_count("GROK_ACP_RESULT", 2)
                self.assertIn("turn:replacement direction", running.output)
                self.assertEqual(running.close(), 0)
            finally:
                running.close(force=True)

            messages = running.log_messages()
            rpcs = [entry["message"] for entry in messages if entry["kind"] == "rpc"]
            self.assertEqual(sum(msg.get("method") == "session/new" for msg in rpcs), 1)
            self.assertEqual(
                sum(msg.get("method") == "x.ai/interject" for msg in rpcs), 1
            )
            self.assertEqual(
                sum(msg.get("method") == "_x.ai/interject" for msg in rpcs), 1
            )
            self.assertGreaterEqual(
                sum(msg.get("method") == "session/cancel" for msg in rpcs), 1
            )
            self.assertEqual(
                sum(msg.get("method") == "session/prompt" for msg in rpcs), 2
            )

    def test_eof_keeps_initial_turn_then_exits(self) -> None:
        with tempfile.TemporaryDirectory(prefix="grok-acp-test-") as tmp:
            root = Path(tmp)
            prompt = root / "prompt.md"
            log_path = root / "fake.log"
            prompt.write_text("one shot", encoding="utf-8")
            env = os.environ.copy()
            env["GROK_EXECUTOR_GROK_BIN"] = str(FAKE_GROK)
            env["FAKE_GROK_LOG"] = str(log_path)
            result = subprocess.run(
                [
                    str(WRAPPER),
                    "--inspect",
                    "--cwd",
                    str(root),
                    "--prompt-file",
                    str(prompt),
                ],
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                encoding="utf-8",
                env=env,
                timeout=10,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("turn:one shot", result.stdout)
            self.assertIn("GROK_ACP_RESULT", result.stdout)
            self.assertIn("GROK_ACP_CLOSED", result.stdout)
            self.assertNotIn("FAKE_GROK_STDERR_MARKER", result.stderr)
            argv = json.loads(log_path.read_text(encoding="utf-8").splitlines()[0])[
                "argv"
            ]
            self.assertIn("read-only", argv)
            self.assertIn("--no-plan", argv)
            self.assertIn("--no-subagents", argv)
            self.assertIn("--disable-web-search", argv)
            self.assertIn("MCPTool(*)", argv)

    def test_summary_is_bounded_and_status_is_compact(self) -> None:
        with tempfile.TemporaryDirectory(prefix="grok-acp-test-") as tmp:
            running = RunningWrapper(Path(tmp), large_chars=12_000)
            try:
                running.wait_for("GROK_ACP_TURN_STARTED")
                running.send({"type": "status"})
                running.wait_for("GROK_ACP_STATUS")
                status = running.event_payloads("STATUS")[-1]
                self.assertEqual(status["active"], True)
                self.assertIn("elapsedSec", status)
                self.assertIn("toolCalls", status)
                result = running.wait_for_event("RESULT")
                summary = str(result["summary"])
                self.assertLessEqual(len(summary.encode("utf-8")), 4096)
                self.assertTrue(summary.startswith("...[truncated]"))
                self.assertEqual(result["toolCalls"], 1)
                self.assertEqual(result["lastTool"], "fake diagnostic tool")
                self.assertEqual(running.close(), 0)
            finally:
                running.close(force=True)


if __name__ == "__main__":
    unittest.main()
