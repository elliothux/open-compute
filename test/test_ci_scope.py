import unittest
from pathlib import Path
import subprocess
import tempfile
from unittest.mock import patch

from ci_scope import BASELINE_PATH, PRODUCT_TARGETS, affected_targets, changed_files, classify


class CiScopeTests(unittest.TestCase):
    def test_product_target_unions_and_shared_fallback(self):
        self.assertEqual(affected_targets(['crates/service/src/r2_api.rs']), ['r2', 'recorded', 'contract'])
        self.assertEqual(affected_targets([
            'crates/service/src/r2_api.rs', 'crates/storage/src/d1.rs',
            'crates/service/src/r2_backend/tests/multipart.rs',
        ]), ['r2', 'd1', 'recorded', 'contract'])
        self.assertEqual(affected_targets(['crates/service/tests/p0_5_r2_support/uploads.rs']),
                         ['r2', 'recorded', 'contract'])
        self.assertEqual(affected_targets(['test/applications/python-queues/src/entry.py']),
                         ['recorded', 'python-queues', 'contract'])
        for path in ['crates/core/src/ids.rs', 'Cargo.lock', 'packages/runtime/src/loader.ts']:
            self.assertEqual(affected_targets([path]), list(PRODUCT_TARGETS), path)
        self.assertEqual(affected_targets(['test/conformance/fixtures/kv/portable-namespace/contract.json']), ['recorded', 'contract'])
        self.assertIn('recorded', PRODUCT_TARGETS)
        self.assertNotIn('single-binary', PRODUCT_TARGETS)
        self.assertNotIn('cf-diff', PRODUCT_TARGETS)
        self.assertNotIn('ai-provider-qualification', PRODUCT_TARGETS)
        self.assertEqual(affected_targets(['docs/references/testing.md', 'AGENTS.md',
                                          'apps/website/src/app.tsx']), [])

    def test_deleted_and_renamed_products_keep_both_sides(self):
        with tempfile.TemporaryDirectory() as directory:
            def git(*args):
                return subprocess.check_output(['git', *args], cwd=directory, text=True).strip()

            git('init', '--quiet')
            source = Path(directory, 'crates/service/src/r2_api.rs')
            source.parent.mkdir(parents=True)
            source.write_text('same source\n')
            git('add', '.')
            base = git('write-tree')
            for destination, expected in [
                    (None, ['r2', 'recorded', 'contract']),
                    ('d1.rs', ['r2', 'd1', 'recorded', 'contract']),
                    ('shared.rs', list(PRODUCT_TARGETS))]:
                git('read-tree', '--reset', '-u', base)
                if destination:
                    source.rename(source.with_name(destination))
                else:
                    source.unlink()
                git('add', '-A')
                head = git('write-tree')
                with patch('ci_scope.git_output', side_effect=lambda args: git(*args)):
                    self.assertEqual(affected_targets(changed_files(base, head)), expected)

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
                    "--no-renames",
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
                    "--no-renames",
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
                "--no-renames",
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
