"""CI browser AppArmor authority, failure propagation and cleanup contracts."""

from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / "scripts/ci-browser-apparmor.py"
SPEC = importlib.util.spec_from_file_location("ci_browser_apparmor", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class BrowserAppArmorTests(unittest.TestCase):
    def test_rule_grants_only_one_exact_executable(self):
        path = "/tmp/browser-123/prepared/chrome-headless-shell"
        policy = MODULE.browser_policy(path)
        self.assertIn(f'"{path}" flags=(unconfined)', policy)
        self.assertEqual(policy.count("userns,"), 1)
        self.assertNotIn("*", policy)
        self.assertEqual(policy, MODULE.browser_policy(path))
        self.assertNotEqual(policy, MODULE.browser_policy(path + "-other"))

    def test_rejects_policy_injection_and_noncanonical_paths(self):
        for path in ["", "/", "relative", "/tmp/a/../b", "//tmp/a", "/tmp/a/", "/tmp/a*", '/tmp/a"', "/tmp/a[bc]", "/tmp/a\nuserns,", "/tmp/a b"]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                MODULE.browser_policy(path)

    def test_unknown_arguments_cannot_call_the_privileged_parser(self):
        with patch.object(MODULE.subprocess, "run") as run:
            for arguments in [[], ["allow"], ["replace", "/tmp/browser"], ["allow", "/tmp/browser", "extra"]]:
                with self.subTest(arguments=arguments), self.assertRaises(ValueError):
                    MODULE.main(arguments)
            run.assert_not_called()

    def test_absent_or_disabled_restriction_needs_no_authorization(self):
        for value in ["0\n", FileNotFoundError()]:
            output = io.StringIO()
            with self.subTest(value=value), patch.object(MODULE.Path, "read_text", **({"side_effect": value} if isinstance(value, Exception) else {"return_value": value})), patch.object(MODULE.subprocess, "run") as run, redirect_stdout(output):
                MODULE.main(["allow", "/tmp/browser"])
                run.assert_not_called()
                self.assertEqual(output.getvalue(), "")

    def test_invalid_or_unreadable_restriction_fails_closed(self):
        for value in ["2", "", PermissionError()]:
            output = io.StringIO()
            with self.subTest(value=value), patch.object(MODULE.Path, "read_text", **({"side_effect": value} if isinstance(value, Exception) else {"return_value": value})), patch.object(MODULE.subprocess, "run") as run, redirect_stdout(output), self.assertRaises((ValueError, PermissionError)):
                MODULE.main(["allow", "/tmp/browser"])
            run.assert_not_called()
            self.assertEqual(output.getvalue(), "")

    def test_allow_records_cleanup_before_adding_the_rule(self):
        output = io.StringIO()
        def execute(*args, **kwargs):
            self.assertEqual(output.getvalue(), "OPEN_COMPUTE_CI_BROWSER_APPARMOR_EXECUTABLE=/tmp/browser\n")
        with patch.object(MODULE.Path, "read_text", return_value="1\n"), patch.object(MODULE.subprocess, "run", side_effect=execute) as run, redirect_stdout(output):
            MODULE.main(["allow", "/tmp/browser"])
        self.assertEqual(run.call_args.args[0][-1], "-a")
        self.assertEqual(run.call_args.kwargs["input"], MODULE.browser_policy("/tmp/browser"))
        self.assertTrue(run.call_args.kwargs["check"])
        self.assertEqual(run.call_args.kwargs["timeout"], 15)
        self.assertEqual(run.call_args.args[0][:5], ["sudo", "-n", "timeout", "--kill-after=1s", "10s"])

    def test_remove_does_not_depend_on_the_current_sysctl(self):
        with patch.object(MODULE.Path, "read_text", side_effect=AssertionError("unexpected sysctl read")), patch.object(MODULE.subprocess, "run") as run:
            MODULE.main(["remove", "/tmp/browser"])
        self.assertEqual(run.call_args.args[0][-1], "-R")
        self.assertEqual(run.call_args.kwargs["input"], MODULE.browser_policy("/tmp/browser"))

    def test_add_and_remove_propagate_errors_and_timeouts(self):
        for operation in ["allow", "remove"]:
            for error in [subprocess.CalledProcessError(17, "parser", stderr="x" * 20000 + "parser failure"), subprocess.TimeoutExpired("parser", 15)]:
                output = io.StringIO()
                stderr = io.StringIO()
                with self.subTest(operation=operation, error=type(error).__name__), patch.object(MODULE.Path, "read_text", return_value="1\n"), patch.object(MODULE.subprocess, "run", side_effect=error), redirect_stdout(output), redirect_stderr(stderr), self.assertRaises(type(error)):
                    MODULE.main([operation, "/tmp/browser"])
                self.assertEqual(bool(output.getvalue()), operation == "allow")
                self.assertLessEqual(len(stderr.getvalue()), 16384)
                if isinstance(error, subprocess.CalledProcessError):
                    self.assertTrue(stderr.getvalue().endswith("parser failure"))


if __name__ == "__main__":
    unittest.main()
