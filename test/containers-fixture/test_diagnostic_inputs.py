"""Input retention must detect replacement even while the original file stays open."""

import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('diagnostic_inputs',
                                             Path(__file__).with_name('diagnostic-inputs.py'))
diagnostic = importlib.util.module_from_spec(spec)
spec.loader.exec_module(diagnostic)
native_spec = importlib.util.spec_from_file_location('native_http_diagnostic',
    Path(__file__).with_name('native-http-diagnostic.py'))
native = importlib.util.module_from_spec(native_spec)
native_spec.loader.exec_module(native)


class DiagnosticInputsTest(unittest.TestCase):
    def test_native_dependency_rejects_unpinned_or_foreign_archives(self):
        source = (Path(__file__).resolve().parents[2] /
            'third_party/workerd/build/deps/gen/deps.MODULE.bazel').read_text()
        pinned = native.dependency(source)
        self.assertRegex(pinned['sha256'], r'^[0-9a-f]{64}$')
        for invalid in (source.replace(pinned['url'], 'https://example.invalid/archive'),
                        source.replace(pinned['sha256'], 'missing'),
                        source.replace(pinned['strip_prefix'], '../outside')):
            with self.subTest(source=invalid[:32]):
                with self.assertRaisesRegex(ValueError, 'CAPNP_DEPENDENCY_INVALID'):
                    native.dependency(invalid)

    def test_replacement_and_byte_drift_are_rejected_and_descriptors_close(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'input'
            path.write_bytes(b'original')
            held = diagnostic.HeldInputs([path])
            fd = held.entries[0][1]
            held.verify(content=True)
            replacement = path.with_name('replacement')
            replacement.write_bytes(b'original')
            replacement.replace(path)
            with self.assertRaisesRegex(RuntimeError, 'IDENTITY_CHANGED'):
                held.verify(content=True)
            held.close()
            with self.assertRaises(OSError):
                os.fstat(fd)
            held = diagnostic.HeldInputs([path])
            try:
                path.write_bytes(b'modified')
                with self.assertRaisesRegex(RuntimeError, 'IDENTITY_CHANGED'):
                    held.verify(content=True)
            finally:
                held.close()


if __name__ == '__main__':
    unittest.main()
