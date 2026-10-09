#!/usr/bin/env python3
"""One SLO evaluator for P0, peak and soak; missing samples never pass."""
import argparse
import json
import math
from pathlib import Path

slo_2c4g = {
    "http": {"p95_ms": 2500, "p99_ms": 3500, "error_rate_max": 0.01},
    "kv": {"p95_ms": 5000, "p99_ms": 6000, "error_rate_max": 0.01},
    "d1": {"p95_ms": 3000, "p99_ms": 10000, "error_rate_max": 0.05},
    "r2": {"p95_ms": 1000, "p99_ms": 3000, "error_rate_max": 0.15},
    "queue": {"p95_ms": 900, "p99_ms": 2000, "error_rate_max": 0.01},
    "do": {"p95_ms": 1500, "p99_ms": 3000, "error_rate_max": 0.15},
    "workflow": {"p95_ms": 2000, "p99_ms": 4000, "error_rate_max": 0.10},
    "fetch": {"p95_ms": 2000, "p99_ms": 3000, "error_rate_max": 0.01},
    "cpu": {"p95_ms": 3000, "p99_ms": 6000, "error_rate_max": 0.01},
    "scenario_mega": {"p95_ms": 3000, "p99_ms": 6000, "error_rate_max": 0.05},
}
slo_p0 = {
    "http": {"p95_ms": 800, "p99_ms": 1500, "error_rate_max": 0.01},
    "kv": {"p95_ms": 600, "error_rate_max": 0.01},
    "d1": {"p95_ms": 1200, "error_rate_max": 0.01},
    "r2": {"p95_ms": 1000, "error_rate_max": 0.01},
    "queue": {"p95_ms": 900, "error_rate_max": 0.01},
    "do": {"p95_ms": 1000, "error_rate_max": 0.01},
    "workflow": {"p95_ms": 2000, "error_rate_max": 0.01},
    "fetch": {"p95_ms": 1200, "error_rate_max": 0.01},
    "cpu": {"p95_ms": 3000, "error_rate_max": 0.01},
    "service": {"p95_ms": 800, "error_rate_max": 0.01},
    "scenario_mega": {"p95_ms": 3000, "error_rate_max": 0.01},
}

slo_8c16g = {
    "http": {"p95_ms": 1500, "p99_ms": 2500, "error_rate_max": 0.01},
    "kv": {"p95_ms": 3000, "p99_ms": 5000, "error_rate_max": 0.01},
    "d1": {"p95_ms": 2000, "p99_ms": 8000, "error_rate_max": 0.02},
    "r2": {"p95_ms": 800, "p99_ms": 2000, "error_rate_max": 0.05},
    "queue": {"p95_ms": 700, "p99_ms": 1500, "error_rate_max": 0.01},
    "do": {"p95_ms": 1000, "p99_ms": 2000, "error_rate_max": 0.05},
    "workflow": {"p95_ms": 1500, "p99_ms": 3000, "error_rate_max": 0.05},
    "fetch": {"p95_ms": 1200, "p99_ms": 2000, "error_rate_max": 0.01},
    "cpu": {"p95_ms": 2500, "p99_ms": 5000, "error_rate_max": 0.01},
    "scenario_mega": {"p95_ms": 2500, "p99_ms": 5000, "error_rate_max": 0.03},
}


def summarize(directory, stacks, profile, mode):
    limits = slo_p0 if mode == "p0" else (slo_2c4g if profile == "2c4g" else slo_8c16g)
    results = {}
    for name in stacks:
        threshold = limits[name]
        lat_file = directory / f"lat-{name}.txt"
        err_file = directory / f"err-{name}.txt"
        samples = sorted(int(line) for line in lat_file.read_text().splitlines() if line.strip()) if lat_file.exists() else []
        errors = len(err_file.read_text().splitlines()) if err_file.exists() else 0
        if any(value < 0 for value in samples) or errors > len(samples):
            raise ValueError(f"invalid latency/error counts for {name}")
        count = len(samples)
        rate = errors / count if count else 1.0
        def percentile(percent):
            return samples[math.ceil(count * percent / 100) - 1] if count else 0
        latency = {"p50": percentile(50), "p95": percentile(95), "p99": percentile(99)}
        anomalies = []
        if not count:
            anomalies.append({"type": "no_samples"})
        for field, actual, maximum in (
            ("error_rate", rate, threshold["error_rate_max"]),
            ("p95", latency["p95"], threshold["p95_ms"]),
            ("p99", latency["p99"], threshold.get("p99_ms", math.inf)),
        ):
            if actual > maximum:
                anomalies.append({"type": f"slo_{field}", "actual": actual, "max": maximum})
        results[name] = {"samples": count, "errors": errors, "error_rate": round(rate, 6),
                         "latency_ms": latency, "thresholds": threshold,
                         "anomalies": anomalies, "verdict": "fail" if anomalies else "pass"}
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--mode", choices=("p0", "peak", "soak"), required=True)
    parser.add_argument("--profile", choices=("2c4g", "8c16g"), required=True)
    parser.add_argument("--stacks", nargs="+", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--timestamp", required=True)
    parser.add_argument("--scale", type=float, default=1.0)
    parser.add_argument("--restarts", type=int, default=0)
    args = parser.parse_args()
    if len(args.stacks) != len(set(args.stacks)) or not 0 < args.scale <= 1 or args.restarts < 0:
        parser.error("invalid stack inventory, scale or restart count")
    if args.mode == "p0" and args.profile != "2c4g":
        parser.error("P0 has only a declared 2c4g profile")
    stacks = summarize(args.directory, args.stacks, args.profile, args.mode)
    output = {"schema_version": 2, "profile": (f"p0-{args.profile}" if args.mode == "p0" else f"p1-{args.profile}-{args.mode}") + ("-abbrev" if args.scale < 1 else ""),
              "stress_profile": args.profile, "run_id": args.run_id, "timestamp": args.timestamp,
              "scale": args.scale, "stacks": stacks,
              "verdict": "pass" if all(s["verdict"] == "pass" for s in stacks.values()) else "fail"}
    if args.mode == "soak":
        events = [json.loads(line) for line in (args.directory / "soak-events.jsonl").read_text().splitlines() if line.strip()]
        verified = [event for event in events if event.get("event") == "restart_reconcile_ok"]
        if len(verified) != args.restarts:
            raise ValueError("restart count does not match verified recovery events")
        output["soak"] = {"container_restarts_injected": args.restarts, "events": events}
    print(json.dumps(output, indent=2))


if __name__ == "__main__":
    main()
