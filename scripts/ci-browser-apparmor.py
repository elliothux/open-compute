#!/usr/bin/env python3
"""Grant the CI browser user namespaces without disabling its native sandbox."""

import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys


def browser_policy(executable: str) -> str:
    """Build an AppArmor rule for one canonical executable path, without globs."""
    if (
        not re.fullmatch(r"/[A-Za-z0-9_./-]+", executable)
        or executable.startswith("//")
        or os.path.normpath(executable) != executable
        or len(executable.encode()) > 4096
    ):
        raise ValueError("browser authorization requires a canonical executable path")
    name = "open-compute-browser-" + hashlib.sha256(executable.encode()).hexdigest()
    return (
        f'abi <abi/4.0>,\nprofile {name} "{executable}" flags=(unconfined) {{\n'
        "  userns,\n}\n"
    )


def main(arguments: list[str]) -> None:
    """Add or remove the browser rule; propagate permission and cleanup failures."""
    if len(arguments) != 2 or arguments[0] not in ("allow", "remove"):
        raise ValueError("expected allow|remove and an absolute browser executable")
    operation, executable = arguments
    policy = browser_policy(executable)
    if operation == "allow":
        try:
            restriction = Path(
                "/proc/sys/kernel/apparmor_restrict_unprivileged_userns"
            ).read_text().strip()
        except FileNotFoundError:
            return
        if restriction == "0":
            return
        if restriction != "1":
            raise ValueError("unknown AppArmor user namespace restriction")
        # Record cleanup authority before the privileged command can fail.
        print(f"OPEN_COMPUTE_CI_BROWSER_APPARMOR_EXECUTABLE={executable}", flush=True)
    try:
        subprocess.run(
            [
                "sudo", "-n", "timeout", "--kill-after=1s", "10s",
                "apparmor_parser", "-a" if operation == "allow" else "-R",
            ],
            input=policy,
            text=True,
            capture_output=True,
            check=True,
            timeout=15,
        )
    except subprocess.CalledProcessError as error:
        if error.stderr:
            sys.stderr.write(error.stderr[-16384:])
        raise


if __name__ == "__main__":
    main(sys.argv[1:])
