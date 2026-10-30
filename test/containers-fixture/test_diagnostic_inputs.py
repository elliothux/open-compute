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


class DiagnosticInputsTest(unittest.TestCase):
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
