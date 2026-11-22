#!/usr/bin/env python3
"""Real install/setup/service/data/upgrade qualification in an explicitly disposable OS."""
import argparse
import functools
import http.server
import json
import os
from pathlib import Path
import pwd
import re
import stat
import subprocess
import sys
import threading
import time
import urllib.request

sys.dont_write_bytecode = True
from prepare import digest, verify_release

LABEL = "dev.open-compute.ocd"
BASE = "http://127.0.0.1:8787"


class Qualification:
    def __init__(self, args, mirror):
        self.args = args
        self.mirror = mirror
        self.inputs = json.loads((args.inputs / "inputs.json").read_text())
        self.events = []
        self.home = Path(pwd.getpwuid(os.getuid()).pw_dir)
        self.system = args.scope == "system"
        self.root = Path("/var/lib/open-compute") if self.system else self.home / ".open-compute"
        self.binary = Path("/usr/local/bin/ocd") if self.system else self.home / ".local/bin/ocd"
        self.config = self.root / "instances/default/compute.toml"
        self.prefix = ["sudo", "-n"] if self.system else []
        self.existing_staging = set(self.binary.parent.glob(".ocd-upgrade-*"))
        if sys.platform == "darwin":
            directory = Path("/Library/LaunchDaemons") if self.system else self.home / "Library/LaunchAgents"
            self.unit = directory / (LABEL + ".plist")
        else:
            directory = Path("/etc/systemd/system") if self.system else self.home / ".config/systemd/user"
            self.unit = directory / (LABEL + ".service")
        self.created_parents = []
        parent = directory
        while not parent.exists():
            self.created_parents.append(parent)
            parent = parent.parent
        self.existing_parent = (parent, stat.S_IMODE(parent.stat().st_mode))

    def command(self, label, *args, check=True):
        result = subprocess.run(args, capture_output=True, text=True, timeout=120)
        # Never retain command output: setup/API responses can contain credentials.
        event = {"step": label, "exitCode": result.returncode}
        last_line = result.stderr.splitlines()[-1] if result.stderr.splitlines() else ""
        error = re.match(r"^([A-Z][A-Z_]+):", last_line)
        if error:
            event["errorCode"] = error[1]
        self.events.append(event)
        if check and result.returncode:
            raise AssertionError(f"{label} failed (exit {result.returncode})")
        return result

    def ocd(self, *args):
        scope = ["--system"] if self.system else []
        return self.command("ocd " + args[0], *self.prefix, str(self.binary),
                            "--no-update-check", *scope, *args).stdout

    def pid(self):
        if sys.platform == "darwin":
            domain = "system" if self.system else f"gui/{os.getuid()}"
            output = self.command("launchctl print", *self.prefix, "launchctl", "print",
                                  domain + "/" + LABEL, check=False)
            match = re.search(r"\bpid = (\d+)", output.stdout)
            return int(match[1]) if match else 0
        return int(self.command("systemctl MainPID", *self.prefix, "systemctl",
                                *([] if self.system else ["--user"]), "show", self.unit.name,
                                "--property=MainPID", "--value").stdout.strip())

    def ready(self):
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            try:
                for path in ("/health/live", "/health/ready"):
                    with urllib.request.urlopen(BASE + path, timeout=1) as response:
                        assert response.status == 200
                # The shared daemon health can precede admission of its instance.
                instances = self.api("/operator/api/instances")
                assert len(instances) == 1 and instances[0]["state"] == "running"
                accounts = self.api("/client/v4/accounts")
                assert len(accounts) == 1
                account = accounts[0]["id"]
                assert self.api(f"/client/v4/accounts/{account}")["id"] == account
                pid = self.pid()
                assert pid > 0
                uid = int(self.command("service UID", "ps", "-o", "uid=", "-p", str(pid)).stdout.strip())
                assert uid == os.getuid(), "daemon must run as the operator, including system scope"
                return
            except (OSError, AssertionError, ValueError):
                time.sleep(0.1)
        raise AssertionError("service readiness deadline expired")

    def api(self, path, method="GET", body=None, content_type="application/json", host=None):
        token = (self.root / "keys/admin.token").read_text().strip()
        headers = {"Authorization": "Bearer " + token, "Content-Type": content_type}
        if host:
            headers["Host"] = host
        data = json.dumps(body).encode() if isinstance(body, dict) else body
        request = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
        with urllib.request.urlopen(request, timeout=20) as response:
            value = response.read()
        if host or "/values/" in path:
            return value
        payload = json.loads(value)
        if path == "/operator/api/instances":
            return payload["instances"]
        assert payload["success"], "control API refused qualification operation"
        return payload["result"]

    def seed(self, *, worker=True):
        accounts = self.api("/client/v4/accounts")
        assert len(accounts) == 1
        account = accounts[0]["id"]
        prefix = f"/client/v4/accounts/{account}"
        namespace = self.api(prefix + "/storage/kv/namespaces", "POST", {"title": "installation-state"})["id"]
        self.value_path = prefix + f"/storage/kv/namespaces/{namespace}/values/state"
        self.api(self.value_path, "PUT", b"installation-state", "text/plain")
        databases = prefix + "/d1/database"
        self.d1_uuid = self.api(databases, "POST", {"name": "installation-state"})["uuid"]
        self.d1_path = f"{databases}/{self.d1_uuid}"
        self.d1_table, self.d1_column = "entries", "value"
        # These statements also run on the previous release.
        self.d1_query("CREATE TABLE entries (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
        self.d1_query("INSERT INTO entries (id, value) VALUES (1, 'alpha'), (7, 'beta')")
        self.worker_host = None
        if worker:
            metadata = {"main_module": "index.js", "compatibility_date": "2026-09-08",
                        "bindings": [{"type": "kv_namespace", "name": "DATA", "namespace_id": namespace}]}
            boundary = "installation-boundary"
            source = 'export default {async fetch(request,env){return new Response(await env.DATA.get("state"));}}'
            multipart = (f'--{boundary}\r\nContent-Disposition: form-data; name="metadata"\r\n'
                         f'Content-Type: application/json\r\n\r\n{json.dumps(metadata)}\r\n'
                         f'--{boundary}\r\nContent-Disposition: form-data; name="index.js"; filename="index.js"\r\n'
                         f'Content-Type: application/javascript+module\r\n\r\n{source}\r\n--{boundary}--\r\n').encode()
            self.api(prefix + "/workers/scripts/installation-worker", "PUT", multipart,
                     "multipart/form-data; boundary=" + boundary)
            self.worker_host = f"installation-worker.{account}.localhost"
        self.verify_data()

    def d1_query(self, sql):
        result, = self.api(self.d1_path + "/query", "POST", {"sql": sql})
        assert result["success"], "D1 query failed"
        return result["results"]

    def verify_data(self):
        assert self.api(self.value_path) == b"installation-state"
        if self.worker_host is not None:
            assert self.api("/", host=self.worker_host) == b"installation-state"
        assert self.api(self.d1_path)["uuid"] == self.d1_uuid
        assert self.d1_query(f"SELECT * FROM {self.d1_table} ORDER BY id") == [
            {"id": 1, self.d1_column: "alpha"},
            {"id": 7, self.d1_column: "beta"},
        ]

    def identity(self):
        paths = [self.config, self.root / "ocd.toml", *self.root.glob("**/keys/*")]
        assert any(p.name == "master.key" for p in paths)
        return {str(p.relative_to(self.root)): digest(p) for p in paths if p.is_file()}

    def install(self, tag, mask, *, worker=True):
        assert all(not p.exists() and not p.is_symlink() for p in (self.root, self.binary, self.unit)), "requires empty product scope"
        os.umask(mask)
        directory = self.args.inputs / "download" / tag
        verify_release(directory, tag, self.inputs["target"])
        # HTTP keeps system/user receipts on the same real production download path.
        self.command("install.sh", *self.prefix, "env", f"OPEN_COMPUTE_RELEASE_TAG={tag}",
                     f"OPEN_COMPUTE_RELEASE_DOWNLOAD_BASE={self.mirror}/download", "sh", str(self.args.installer))
        assert tag[1:] == self.command("installed version", str(self.binary), "--no-update-check", "--version").stdout.split()[1]
        if self.system and tag == self.inputs["previous"]:
            # The published previous binary rejects its installer's service-user
            # temp owner under sudo. Bootstrap this historical fixture through
            # manual setup; remove only the empty temp directory we just created.
            temporary = self.root / "tmp"
            assert temporary.is_dir() and not temporary.is_symlink() and not any(temporary.iterdir())
            self.command("previous system manual setup preparation", "rmdir", "--", str(temporary))
        self.ocd("setup", "--yes")
        self.ready()
        assert stat.S_IMODE(self.unit.stat().st_mode) == 0o600
        for parent in self.created_parents:
            mode = stat.S_IMODE(parent.stat().st_mode)
            assert mode == 0o700 if tag == self.inputs["candidate"] else mode & 0o022 == 0
        parent, mode = self.existing_parent
        assert stat.S_IMODE(parent.stat().st_mode) == mode, "setup changed an existing parent"
        if sys.platform != "darwin":
            output = self.command("service enabled", *self.prefix, "systemctl",
                                  *([] if self.system else ["--user"]), "is-enabled", self.unit.name)
            assert output.stdout.strip() == "enabled"
        self.seed(worker=worker)
        self.initial_identity = self.identity()

    def stop(self):
        # Track the actual daemon descendants, including Linux memfd executables.
        tree = self.command("process tree", "ps", "-eo", "pid=,ppid=").stdout
        parents = {int(pid): int(parent) for pid, parent in (line.split() for line in tree.splitlines())}
        daemon_pid = self.pid()
        assert daemon_pid > 0
        descendants = {daemon_pid}
        while True:
            expanded = descendants | {pid for pid, parent in parents.items() if parent in descendants}
            if expanded == descendants:
                break
            descendants = expanded
        self.ocd("stop")
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            reaped = self.pid() == 0
            for pid in descendants:
                result = self.command("daemon descendant reaped", "ps", "-p", str(pid), "-o", "pid=", check=False)
                reaped &= result.returncode == 1 and not result.stdout.strip()
            if reaped:
                return
            time.sleep(0.1)
        raise AssertionError("service manager PID and owned descendants did not disappear after stop")

    def restart(self):
        before = self.pid()
        self.ocd("restart")
        self.ready()
        assert self.pid() != before
        self.verify_data()
        self.stop()
        self.ocd("start")
        self.ready()
        self.verify_data()
        assert self.identity() == self.initial_identity
        self.stop()
        # Storage integrity is checked as its actual owner; only service-manager
        # operations require sudo in system scope.
        self.command("service-owner doctor", str(self.binary), "--no-update-check",
                     *(["--system"] if self.system else []),
                     "--config", str(self.config), "doctor", "--json")
        self.ocd("start")
        self.ready()

    def remove(self):
        # Teardown owns only paths absent at install preflight. Product purge is
        # independently qualified; it is not a prerequisite for the next fixture.
        self.stop()
        if sys.platform != "darwin":
            self.command("unload fixture service", *self.prefix, "systemctl",
                         *([] if self.system else ["--user"]), "disable", "--now", self.unit.name)
        assert self.unit.is_file() and not self.unit.is_symlink()
        assert self.binary.is_file() and not self.binary.is_symlink()
        candidate = self.args.inputs / "download" / self.inputs["candidate"] / f'ocd-{self.inputs["candidate"]}-{self.inputs["target"]}'
        for staged in set(self.binary.parent.glob(".ocd-upgrade-*")) - self.existing_staging:
            assert staged.is_file() and not staged.is_symlink() and digest(staged) == digest(candidate)
            self.command("remove owned staged candidate", *self.prefix, "rm", "--", str(staged))
        self.command("remove owned fixture files", *self.prefix, "rm", "--", str(self.unit), str(self.binary))
        if sys.platform != "darwin":
            self.command("reload service definitions", *self.prefix, "systemctl",
                         *([] if self.system else ["--user"]), "daemon-reload")
        for parent in self.created_parents:
            if parent.exists():
                # systemctl disable can retain empty *.target.wants directories.
                for directory, _children, files in os.walk(parent, topdown=False):
                    assert not files, "fixture service directory contains unowned files"
                    Path(directory).rmdir()
        assert not self.root.is_symlink() and self.root.stat().st_uid == os.getuid()
        self.command("remove owned fixture scope", *self.prefix, "rm", "-r", "--", str(self.root))

    def upgrade(self, label):
        return self.command(label, *self.prefix, str(self.args.driver),
                            "--binary", str(self.binary), "--download-base", self.mirror + "/download",
                            "--version", self.inputs["candidate"][1:],
                            *(["--system"] if self.system else []), check=False)

    def prepare_previous_config(self, check_rejection):
        assert self.config.is_file() and not self.config.is_symlink()
        original = self.config.read_bytes()
        section = re.search(rb"(?ms)^\[durable_objects\][ \t]*\r?\n(.*?)(?=^\[|\Z)", original)
        assert section is not None, "previous setup omitted the Durable Object defaults"
        defaults = rb"(?m)^disk_(?:high_watermark_percent = 85|stop_writes_percent = 95)[ \t]*\r?\n"
        fields = re.findall(rb"(?m)^disk_(?:high_watermark|stop_writes)_percent\s*=", section[1])
        if not fields:
            return
        prepared, replaced = re.subn(defaults, b"", section[1])
        assert replaced == len(fields) == 2, "previous setup changed the historical disk defaults"
        if check_rejection:
            receipt = self.root / "install-receipt.json"
            before = (self.pid(), digest(self.binary), digest(receipt))
            self.verify_upgrade_rejection(
                self.upgrade("unmodified legacy config upgrade"), "INSTANCE_REGISTRY_INVALID")
            self.verify_unchanged_installation(before)
        # Both releases supply their own defaults when the historical keys are absent.
        self.config.write_bytes(original[:section.start(1)] + prepared + original[section.end(1):])
        self.initial_identity[str(self.config.relative_to(self.root))] = digest(self.config)

    def verify_upgrade_rejection(self, result, code):
        assert result.returncode != 0 and result.stderr.rstrip().split("\n")[-1].startswith(code + ":"), f"expected upgrade rejection {code}"

    def verify_unchanged_installation(self, before):
        receipt = self.root / "install-receipt.json"
        assert (self.pid(), digest(self.binary), digest(receipt)) == before
        assert self.identity() == self.initial_identity
        self.verify_data()

    def run(self):
        self.install(self.inputs["candidate"], 0o002)
        self.restart()
        self.remove()

        self.install(self.inputs["previous"], 0o022, worker=not bool(self.args.reject_code))
        receipt = self.root / "install-receipt.json"
        before = (self.pid(), digest(self.binary), digest(receipt))
        self.prepare_previous_config(check_rejection=True)
        self.verify_unchanged_installation(before)
        result = self.upgrade("D1 and KV production upgrade")
        if result.returncode:
            self.verify_data()
            raise AssertionError("D1 and KV production upgrade failed")
        tag, target = self.inputs["candidate"], self.inputs["target"]
        expected = self.args.inputs / "download" / tag / f"ocd-{tag}-{target}"
        assert digest(self.binary) == digest(expected)
        installed = json.loads(self.command("read upgraded receipt", *self.prefix, "cat", str(receipt)).stdout)
        assert installed["version"] == tag[1:] and installed["target"] == target
        assert installed["binary_path"] == str(self.binary.resolve())
        self.ready()
        assert self.pid() != before[0]
        assert self.identity() == self.initial_identity
        self.verify_data()
        self.d1_query("ALTER TABLE entries RENAME COLUMN value TO label")
        self.d1_query("ALTER TABLE entries RENAME TO items")
        self.d1_table, self.d1_column = "items", "label"
        self.verify_data()
        self.restart()
        self.remove()

        if self.args.reject_code:
            self.install(self.inputs["previous"], 0o022)
            self.prepare_previous_config(check_rejection=False)
            before = (self.pid(), digest(self.binary), digest(receipt))
            result = self.upgrade("legacy Worker production upgrade")
            self.verify_upgrade_rejection(result, self.args.reject_code)
            if self.args.reject_code == "MIGRATION_FAILED":
                assert "stored Worker loader schema is not supported by this release" in result.stderr
            self.verify_unchanged_installation(before)
            self.restart()
            self.remove()


class QuietFiles(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_args):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", type=Path, required=True)
    parser.add_argument("--driver", type=Path, required=True)
    parser.add_argument("--installer", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--scope", choices=["user", "system"], required=True)
    parser.add_argument("--reject-code", default="")
    parser.add_argument("--reject-reason", default="")
    args = parser.parse_args()
    assert os.getuid() != 0 and os.environ.get("OPEN_COMPUTE_INSTALL_TEST_DISPOSABLE") == "1", "requires explicit disposable-host acknowledgement"
    assert bool(args.reject_code) == bool(args.reject_reason), "rejection requires a declared code and reason"
    assert not any(name.startswith("OPEN_COMPUTE_TEST_") for name in os.environ), "test root overrides must not bypass production paths"
    args.inputs = args.inputs.resolve()
    args.driver = args.driver.resolve()
    args.installer = args.installer.resolve()
    assert args.driver.is_file() and args.installer.is_file()
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(QuietFiles, directory=str(args.inputs)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    qualification = Qualification(args, f"http://127.0.0.1:{server.server_port}")
    passed = False
    try:
        qualification.run()
        passed = True
    finally:
        # Evidence excludes raw output, databases, credentials and API payloads.
        directory = args.evidence if passed else args.evidence / "failed" / str(time.time_ns())
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "report.json").write_text(json.dumps({
            "passed": passed, "platform": sys.platform, "machine": os.uname().machine,
            "scope": args.scope, **qualification.inputs, "freshUmask": "002",
            "upgradeExpectation": args.reject_code or "success", "rejectionReason": args.reject_reason,
            "events": qualification.events}, indent=2))
        if not passed and qualification.binary.exists():
            qualification.command("failure cleanup stop", *qualification.prefix, str(qualification.binary),
                                  "--no-update-check", *(["--system"] if qualification.system else []), "stop", check=False)
        server.shutdown()
        server.server_close()
    print(f'PASS {sys.platform} {args.scope}: fresh install, service lifecycle, persisted KV/Worker/D1, {qualification.inputs["previous"]} upgrade')


if __name__ == "__main__":
    main()
