#!/usr/bin/env python3
"""Small fake Grok ACP server used by the executor wrapper tests."""

from __future__ import annotations

import json
import os
import sys
import threading
import time

write_lock = threading.Lock()
pending_lock = threading.Lock()
pending_prompt: dict[str, object] | None = None
session_id = "019d0000-0000-7000-8000-000000000001"
log_path = os.environ.get("FAKE_GROK_LOG")


def emit(message: dict[str, object]) -> None:
    with write_lock:
        sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
        sys.stdout.flush()


def log(message: dict[str, object]) -> None:
    if not log_path:
        return
    with open(log_path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(message, separators=(",", ":")) + "\n")


def response(request_id: object, result: object = None, error: object = None) -> None:
    message: dict[str, object] = {"jsonrpc": "2.0", "id": request_id}
    if error is not None:
        message["error"] = error
    else:
        message["result"] = result if result is not None else {}
    emit(message)


def update(text: str) -> None:
    emit(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": text},
                },
            },
        }
    )


def tool_update(title: str) -> None:
    emit(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "tool_call",
                    "toolCallId": "fake-tool-call",
                    "title": title,
                    "status": "in_progress",
                },
            },
        }
    )


def finish_prompt(stop_reason: str = "end_turn") -> None:
    global pending_prompt
    with pending_lock:
        current = pending_prompt
        pending_prompt = None
    if current is not None:
        response(current["id"], {"stopReason": stop_reason})


def delayed_finish() -> None:
    time.sleep(float(os.environ.get("FAKE_GROK_PROMPT_DELAY", "0.25")))
    finish_prompt()


def main() -> int:
    global pending_prompt
    log({"kind": "argv", "argv": sys.argv[1:]})
    print("FAKE_GROK_STDERR_MARKER", file=sys.stderr, flush=True)
    if "agent" not in sys.argv or "stdio" not in sys.argv:
        return 2
    for line in sys.stdin:
        message = json.loads(line)
        log({"kind": "rpc", "message": message})
        method = message.get("method")
        request_id = message.get("id")
        params = message.get("params", {})
        if method == "initialize":
            response(
                request_id,
                {
                    "protocolVersion": 1,
                    "authMethods": [{"id": "cached_token", "name": "cached"}],
                },
            )
        elif method == "authenticate":
            response(request_id, {})
        elif method == "session/new":
            response(request_id, {"sessionId": session_id})
        elif method == "session/prompt":
            with pending_lock:
                pending_prompt = {"id": request_id, "params": params}
            text = params.get("prompt", [{}])[0].get("text", "")
            tool_update("fake diagnostic tool")
            large_chars = int(os.environ.get("FAKE_GROK_LARGE_CHARS", "0"))
            if large_chars:
                update("STATUS:" + ("x" * large_chars))
            else:
                update(f"turn:{text}")
            threading.Thread(target=delayed_finish, daemon=True).start()
        elif method in {"x.ai/interject", "_x.ai/interject"}:
            if os.environ.get("FAKE_GROK_NO_INTERJECT") == "1":
                response(
                    request_id, error={"code": -32601, "message": "method not found"}
                )
            else:
                update(f"|interject:{params.get('text', '')}")
                response(request_id, {})
        elif method == "session/cancel":
            finish_prompt("cancelled")
        elif request_id is not None:
            response(request_id, error={"code": -32601, "message": "method not found"})
    finish_prompt("cancelled")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
