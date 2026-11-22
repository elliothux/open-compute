#!/usr/bin/env python3
"""Passively sample a supplied Linux PID and cgroup into a new JSONL file."""

import argparse
import json
import os
from pathlib import Path
import signal
import sys
import time


def read_fields(path, *keys):
    values = {}
    try:
        for line in path.read_text().splitlines():
            fields = line.split()
            if len(fields) >= 2 and fields[0] in keys and fields[1].isascii() and fields[1].isdigit():
                values[fields[0]] = int(fields[1])
    except (OSError, UnicodeError):
        pass
    return [values.get(key) for key in keys]


def start_ticks(stat):
    try:
        # The command name can contain spaces and parentheses. Field 3 follows the final ')'.
        _, closing, tail = stat.read_text(errors="replace").rpartition(")")
        ticks = tail.split()[19] if closing else ""
        return int(ticks) if ticks.isascii() and ticks.isdigit() else None
    except (OSError, IndexError):
        return None


def sample(args, output, proc, initial_ticks):
    def emit(event, **fields):
        print(json.dumps({"event": event, "monotonic_ns": time.monotonic_ns(), **fields}),
              file=output, flush=True)

    started = time.monotonic_ns()
    deadline = started + args.duration * 10**9
    interval = args.interval * 10**9
    next_sample = started
    emit("start", pid=args.pid, pid_start_ticks=initial_ticks, clock_ticks_per_second=args.clock_ticks,
         run=args.run, phase=args.phase, interval_seconds=args.interval, duration_seconds=args.duration)
    missing = 0
    samples = 0
    outcome = "complete"
    try:
        while True:
            now = time.monotonic_ns()
            if now >= deadline:
                break
            if now < next_sample:
                time.sleep((min(next_sample, deadline) - now) / 10**9)
                continue

            before = start_ticks(proc / "stat")
            dirty = rss = None
            if before == initial_ticks:
                dirty, rss = read_fields(proc / "smaps_rollup", "Private_Dirty:", "Rss:")
            anon, = read_fields(args.cgroup / "memory.stat", "anon")
            after = start_ticks(proc / "stat")
            replacement = next((ticks for ticks in (before, after)
                                if ticks is not None and ticks != initial_ticks), None)
            stable = before == after == initial_ticks
            if not stable:
                dirty = rss = None
            if replacement is not None:
                status = "pid_reused"
            elif None in (dirty, rss, anon):
                status = "missing"
                missing += 1
            else:
                status = "ok"
            emit("sample", status=status, pid_start_ticks=replacement if replacement is not None else after,
                 private_dirty_kib=dirty, rss_kib=rss, cgroup_anon_bytes=anon)
            samples += 1
            if replacement is not None:
                outcome = "pid_reused"
                break
            # Skip elapsed slots so a slow read never triggers catch-up sampling.
            elapsed = time.monotonic_ns() - started
            next_sample = started + (elapsed // interval + 1) * interval
    except KeyboardInterrupt:
        outcome = "interrupted"

    emit("end", status=outcome, samples=samples, missing_samples=missing)
    return 0 if outcome == "complete" else 1


def terminate(_signum, _frame):
    raise KeyboardInterrupt


def main():
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog="Null values indicate missing readings. A run needs an end record with "
               "status complete to confirm it reached its deadline.",
    )
    parser.add_argument("--pid", type=int, required=True, help="existing workerd PID in this PID namespace")
    parser.add_argument("--cgroup", type=Path, required=True, help="existing cgroup-v2 directory")
    parser.add_argument("--interval", type=int, required=True, help="sample interval in whole seconds")
    parser.add_argument("--duration", type=int, required=True, help="duration in whole seconds, up to 86400")
    parser.add_argument("--run", required=True, help="run label")
    parser.add_argument("--phase", required=True, help="phase label")
    parser.add_argument("--output", type=Path, required=True, help="new JSONL file")
    args = parser.parse_args()

    if sys.platform != "linux":
        parser.error("Linux is required")
    if args.pid <= 0 or not 1 <= args.interval <= args.duration <= 86400:
        parser.error("require PID > 0 and 1 <= interval <= duration <= 86400")
    if not args.run.strip() or not args.phase.strip():
        parser.error("run and phase labels must be nonempty")
    if not (args.cgroup / "cgroup.controllers").is_file():
        parser.error("cgroup must be a cgroup-v2 directory")
    proc = Path("/proc") / str(args.pid)
    initial_ticks = start_ticks(proc / "stat")
    if initial_ticks is None:
        parser.error("cannot read the supplied PID's start time")
    args.clock_ticks = os.sysconf("SC_CLK_TCK")
    signal.signal(signal.SIGTERM, terminate)
    try:
        with args.output.open("x") as output:
            return sample(args, output, proc, initial_ticks)
    except OSError as error:
        parser.exit(1, f"Output error: {error.strerror}\n")


if __name__ == "__main__":
    sys.exit(main())
