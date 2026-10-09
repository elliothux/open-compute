import unittest
from unittest.mock import patch

from ci_scope import BASELINE_PATH, changed_files, classify


class CiScopeTests(unittest.TestCase):
    def test_docs_only(self):
        self.assertEqual(classify(["docs/references/testing.md", "README.md"]), ("docs", ""))
        self.assertEqual(
            classify(["docs/references/testing.md", "test/conformance/baseline.json"]),
            ("full", ""),
        )

    def test_release_tooling(self):
        self.assertEqual(
            classify(
                [
                    ".github/workflows/release.yml",
                    "scripts/assemble-release.ts",
                    "scripts/release-test-report.ts",
                    "scripts/release-dry-run.sh",
                    "test/release-dry-run/Dockerfile",
                    "test/release-tools.test.mjs",
                    "test/release-test-report.test.mjs",
                    "test/fixtures/release-evidence.mjs",
                    "docs/references/releasing.md",
                ]
            ),
            ("release-tooling", ""),
        )

    def test_frontend_components(self):
        self.assertEqual(
            classify(["packages/sdk/src/index.ts", "apps/dashboard/src/app.tsx"]),
            ("frontend", "dashboard,sdk"),
        )
        self.assertEqual(
            classify(["apps/website/src/app.tsx", "docs/references/testing.md"]),
            ("frontend", "docs,website"),
        )

    @patch("ci_scope.subprocess.check_output")
    def test_source_digest_only_inherits_owning_changes(self, check_output):
        baseline = '{"schemaVersion":1,"sourceDigest":"%s"}'

        def output(command, **_kwargs):
            args = command[1:]
            values = {
                (
                    "diff",
                    "--name-only",
                    "--diff-filter=ACMRTUXB",
                    "base",
                    "head",
                ): f"{BASELINE_PATH}\n",
                ("show", f"base:{BASELINE_PATH}"): baseline % "before",
                ("show", f"head:{BASELINE_PATH}"): baseline % "after",
                (
                    "log",
                    "--format=%H",
                    "-2",
                    "head",
                    "--",
                    BASELINE_PATH,
                ): "head\nprevious\n",
                ("rev-parse", "head"): "head\n",
                (
                    "diff",
                    "--name-only",
                    "--diff-filter=ACMRTUXB",
                    "previous",
                    "head",
                ): f"apps/website/src/app.tsx\n{BASELINE_PATH}\n",
            }
            return values[tuple(args)]

        check_output.side_effect = output
        paths = changed_files("base", "head")
        self.assertEqual(classify(paths), ("frontend", "website"))

    @patch("ci_scope.subprocess.check_output")
    def test_other_baseline_changes_remain_full(self, check_output):
        values = {
            (
                "diff",
                "--name-only",
                "--diff-filter=ACMRTUXB",
                "base",
                "head",
            ): f"{BASELINE_PATH}\n",
            ("show", f"base:{BASELINE_PATH}"): '{"schemaVersion":1,"sourceDigest":"before"}',
            ("show", f"head:{BASELINE_PATH}"): '{"schemaVersion":2,"sourceDigest":"after"}',
        }
        check_output.side_effect = lambda command, **_kwargs: values[tuple(command[1:])]
        self.assertEqual(classify(changed_files("base", "head")), ("full", ""))

    def test_runtime_and_workflow_changes_are_full(self):
        self.assertEqual(classify(["packages/runtime/src/loader.ts"]), ("full", ""))
        self.assertEqual(classify([".github/workflows/ci.yml"]), ("full", ""))
        self.assertEqual(
            classify([".github/workflows/release.yml", "Cargo.toml"]), ("full", "")
        )
        self.assertEqual(
            classify([".github/actions/setup-open-compute/action.yml"]), ("full", "")
        )


if __name__ == "__main__":
    unittest.main()
