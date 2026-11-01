"""Check image preparation dependencies without starting CI or Docker."""
from pathlib import Path
import json
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
PREPARE = 'uses: ./.github/actions/prepare-container-fixture-images'


def job(source, name):
    match = re.search(r'^  ' + re.escape(name) + r':\n(.*?)(?=^  [\w-]+:\n|\Z)',
                      source, re.MULTILINE | re.DOTALL)
    if match is None:
        raise AssertionError('Missing job: ' + name)
    return match.group(1)


class ImageWorkflowTests(unittest.TestCase):
    def test_each_build_consumer_waits_for_current_run_images(self):
        for workflow, names in [('ci.yml', ('checks', 'product-gates')),
                                ('release.yml', ('coverage', 'package', 'ai-search-qualification',
                                                 's3-provider-qualification'))]:
            source = (ROOT / '.github/workflows' / workflow).read_text()
            for name in names:
                with self.subTest(workflow=workflow, job=name):
                    body = job(source, name)
                    self.assertRegex(body, r'needs: [^\n]*fixture-images')
                    self.assertEqual(body.count(PREPARE), 1)
                    build = body.index('bun run build' if name != 'package'
                                       else './scripts/package-release.sh')
                    self.assertLess(body.index('uses: ./.github/actions/setup-open-compute'),
                                    body.index(PREPARE))
                    self.assertLess(body.index(PREPARE), build)
            producer = job(source, 'fixture-images')
            self.assertIn('needs: failfast', producer)
            self.assertIn('uses: ./.github/workflows/container-images.yml', producer)
        ci = (ROOT / '.github/workflows/ci.yml').read_text()
        self.assertIn('arm64: false', job(ci, 'fixture-images'))
        self.assertIn("needs.failfast.outputs.scope == 'full'", job(ci, 'fixture-images'))
        self.assertIn("github.ref == 'refs/heads/main'", job(ci, 'fixture-images'))
        self.assertIn('needs.fixture-images.result', job(ci, 'ci'))

    def test_native_producer_and_consumer_use_the_same_artifact_names(self):
        source = (ROOT / '.github/workflows/container-images.yml').read_text()
        self.assertIn('workflow_call:', source)
        self.assertIn('type: boolean', source)
        self.assertIn('default: true', source)
        self.assertIn('"os":"ubuntu-24.04","architecture":"amd64"', source)
        self.assertIn('"os":"ubuntu-24.04-arm","architecture":"arm64"', source)
        self.assertNotIn('--image ', source)
        self.assertLess(source.index('./scripts/build-container-sidecar-source.sh'),
                        source.index('test/containers-fixture/build-images.py'))
        self.assertLess(source.index('test/containers-fixture/consume-images.py'),
                        source.index('uses: actions/upload-artifact@'))
        self.assertIn('name: p23-images-${{ matrix.architecture }}', source)
        self.assertIn('if-no-files-found: error', source)
        consumer = (ROOT / '.github/actions/prepare-container-fixture-images/action.yml').read_text()
        self.assertIn('X64) architecture=amd64', consumer)
        self.assertIn('ARM64) architecture=arm64', consumer)
        self.assertIn('name: p23-images-${{ steps.platform.outputs.architecture }}', consumer)
        self.assertNotIn('run-id:', consumer)
        self.assertNotIn('github-token:', consumer)
        self.assertLess(consumer.index('test/containers-fixture/consume-images.py'),
                        consumer.index('echo "OPEN_COMPUTE_TEST_CONTAINERS_INPUT='))
        self.assertNotIn('"containers":', consumer)
        self.assertNotIn('qualified-input', consumer)

    def test_builds_use_cargo_without_wrapper_policy(self):
        for name in ('.github/actions/setup-open-compute/action.yml',
                     '.github/workflows/container-images.yml',
                     '.github/workflows/container-diagnostic.yml',
                     '.github/workflows/ci.yml', '.github/workflows/release.yml',
                     'test/gateway/runtime.Dockerfile', 'test/release-dry-run/Dockerfile',
                     'test/containers-fixture/Dockerfile.base'):
            with self.subTest(file=name):
                source = (ROOT / name).read_text()
                self.assertNotRegex(source, r'\bmbx\b|MBX_|mr-boxington|rust-cache-backend')
        setup = (ROOT / '.github/actions/setup-open-compute/action.yml').read_text()
        self.assertNotIn('Intel', setup)
        self.assertIn('toolchain: 1.98.0', setup)
        self.assertIn('Cache Cargo downloads', setup)
        self.assertIn('cargo build --locked', (ROOT / 'test/gateway/runtime.Dockerfile').read_text())

    def test_release_requires_full_linux_workspace_and_coverage(self):
        source = (ROOT / '.github/workflows/release.yml').read_text()
        coverage = job(source, 'coverage')
        self.assertIn('./test/coverage.sh --jobs 2', coverage)
        self.assertNotIn('\n  integration:', source)
        self.assertEqual(source.count('./test/coverage.sh --jobs 2'), 1)
        self.assertIn('OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO=1', coverage)
        self.assertIn('runs-on: ubuntu-24.04', coverage)
        self.assertNotIn('intel', source.lower())
        for body in (coverage,):
            self.assertEqual(body.count('test/containers-fixture/with-ci-linux-engine.sh'), 1)
            verify = body.index('bun scripts/verify-release-package.ts "$RUNNER_TEMP/release-broker"')
            self.assertLess(verify, body.index('test/containers-fixture/with-ci-linux-engine.sh'))
            self.assertIn('--binary "$RUNNER_TEMP/release-broker/ocd-${{ needs.failfast.outputs.tag }}-linux-x64"', body)
            self.assertIn('.temp/containers-ci-engine/', body)
        matrix = re.search(r"^        include: (.+)$", job(source, 'package'), re.MULTILINE)
        self.assertIsNotNone(matrix)
        alternatives = re.findall(r"'(\[.*?\])'", matrix.group(1))
        self.assertEqual(json.loads(alternatives[-1]), [
            {"target": "darwin-arm64", "os": "macos-15"},
            {"target": "linux-arm64", "os": "ubuntu-24.04-arm"},
            {"target": "linux-x64", "os": "ubuntu-24.04"},
        ])

if __name__ == '__main__':
    unittest.main()
