#!/usr/bin/env python3
"""Install one Linux ocd release binary on the glibc baseline distributions.

Each run serves the binary from a file:// release fixture with the same asset
layout as a GitHub Release. On every supported distribution, scripts/install.sh
installs it and worker.sh deploys and calls a Worker as a non-root user. On the
older distribution, install.sh must refuse the host before it downloads the binary.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[2]
# Multi-arch index digests; Docker selects the binary's architecture.
SUPPORTED = {
    "rockylinux/rockylinux:9": "sha256:8101994123cf3d0a8fee517bee7f39e555c7d92bd2d9eb3303cc988a0eeed00f",
    "ubuntu:22.04": "sha256:076e7bf82319619f28dd0aa9c02ab34b055ad37100395b4f49a2ffba04fb5e11",
}
REFUSED = {
    "rockylinux/rockylinux:8": "sha256:e8a49c5403b687db05d4d67333fa45808fbe74f36e683cec7abb1f7d0f2338c6",
}
MACHINES = {0x3E: ("linux-x64", "linux/amd64"), 0xB7: ("linux-arm64", "linux/arm64")}
# Ubuntu base images omit curl; Rocky images ship curl-minimal.
PREPARE = (
    "set -eu; command -v curl >/dev/null || { apt-get update -qq && "
    "DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "
    "curl ca-certificates >/dev/null; }; useradd --create-home smoke"
)


def docker(*args, check=True, timeout=600):
    result = subprocess.run(["docker", *args], capture_output=True, text=True, timeout=timeout)
    if check and result.returncode != 0:
        raise SystemExit(f"docker {args[0]} failed ({result.returncode}):\n{result.stdout}{result.stderr}")
    return result


def fixture(binary, directory):
    """Write the release directory install.sh reads: release.json, SHA256SUMS, binary."""
    data = binary.read_bytes()
    target, platform = MACHINES[int.from_bytes(data[18:20], "little")]
    cargo = (ROOT / "Cargo.toml").read_text().split("[workspace.package]\n")[1]
    version = re.search(r'^version = "([^"]+)"$', cargo, re.M).group(1)
    baseline = re.search(r'LINUX_GLIBC_BASELINE = "([0-9.]+)"',
                         (ROOT / "scripts/glibc-baseline.ts").read_text()).group(1)
    tag = "v" + version
    release = directory / tag
    release.mkdir(parents=True)
    asset = f"ocd-{tag}-{target}"
    shutil.copyfile(binary, release / asset)
    manifest = json.dumps({"schemaVersion": 1, "version": version, "tag": tag,
                           "artifacts": [{"target": target, "filename": asset, "glibc": baseline}]},
                          indent=2) + "\n"
    (release / "release.json").write_text(manifest)
    sums = {name: hashlib.sha256((release / name).read_bytes()).hexdigest()
            for name in ("release.json", asset)}
    (release / "SHA256SUMS").write_text("".join(f"{digest}  {name}\n" for name, digest in sums.items()))
    shutil.copyfile(ROOT / "scripts/install.sh", directory / "install.sh")
    shutil.copyfile(ROOT / "test/distro-smoke/worker.sh", directory / "worker.sh")
    return {"tag": tag, "target": target, "platform": platform, "glibc": baseline,
            "sha256": sums[asset]}


def exercise(image, digest, release, directory, expect_refusal):
    name = "ocd-distro-smoke-" + uuid.uuid4().hex[:12]
    docker("run", "--detach", "--name", name, "--platform", release["platform"],
           "--volume", f"{directory}:/release:ro", f"{image}@{digest}", "sleep", "infinity")
    try:
        docker("exec", name, "sh", "-c", PREPARE)
        glibc = docker("exec", name, "getconf", "GNU_LIBC_VERSION").stdout.strip()
        install = docker("exec", "--user", "smoke", "--workdir", "/home/smoke",
                         "--env", "OPEN_COMPUTE_RELEASE_TAG=" + release["tag"],
                         "--env", "OPEN_COMPUTE_RELEASE_DOWNLOAD_BASE=file:///release",
                         name, "sh", "/release/install.sh", check=False)
        result = {"image": image, "digest": digest, "glibc": glibc}
        if expect_refusal:
            expected = f"requires glibc {release['glibc']} or newer; this host has {glibc}"
            untouched = docker("exec", name, "sh", "-c",
                               'test ! -e /home/smoke/.local/bin/ocd '
                               '&& test -z "$(ls -A /home/smoke/.open-compute/tmp)"', check=False)
            if install.returncode == 0 or expected not in install.stderr or untouched.returncode != 0:
                raise SystemExit(f"{image} was not refused before download "
                                 f"(exit {install.returncode}):\n{install.stderr}")
            return {**result, "refused": install.stderr.strip().splitlines()[-1]}
        if install.returncode != 0:
            raise SystemExit(f"{image} install failed:\n{install.stderr}")
        version = docker("exec", "--user", "smoke", name, "/home/smoke/.local/bin/ocd",
                         "--no-update-check", "--version").stdout.strip()
        worker = docker("exec", "--user", "smoke", "--workdir", "/home/smoke", name,
                        "sh", "/release/worker.sh", check=False)
        if worker.returncode != 0 or worker.stdout.strip() != "distro-smoke-ok":
            raise SystemExit(f"{image} Worker smoke failed:\n{worker.stdout}{worker.stderr}")
        return {**result, "ocd": version, "workerResponse": worker.stdout.strip()}
    finally:
        docker("rm", "--force", name, check=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", type=Path, required=True, help="Linux ocd release binary")
    parser.add_argument("--evidence", type=Path, help="write the JSON report to this file")
    args = parser.parse_args()
    # Under the checkout, so Docker Desktop and Colima can bind-mount it.
    (ROOT / ".temp").mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="distro-smoke-", dir=ROOT / ".temp") as temporary:
        directory = Path(temporary)
        release = fixture(args.binary.resolve(), directory)
        results = [exercise(image, digest, release, directory, False) for image, digest in SUPPORTED.items()]
        results += [exercise(image, digest, release, directory, True) for image, digest in REFUSED.items()]
    report = {"schemaVersion": 1, **release, "results": results, "passed": True}
    text = json.dumps(report, indent=2) + "\n"
    if args.evidence:
        args.evidence.write_text(text)
    sys.stdout.write(text)


if __name__ == "__main__":
    main()
