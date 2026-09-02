#!/usr/bin/env python3
"""Offline regression test for fixed and hostile document fixture integrity."""

from __future__ import annotations

import os
import subprocess
import sys
import unittest
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]


class DocumentFixtureIntegrityTest(unittest.TestCase):
    """Keep vendored document bytes, provenance, oracles, and seeds frozen."""

    def test_manifest_and_hostile_generator_are_exact(self) -> None:
        """The complete check succeeds without network access or repository writes."""
        environment = os.environ.copy()
        environment["PYTHONDONTWRITEBYTECODE"] = "1"
        result = subprocess.run(
            [sys.executable, "test/check-document-fixtures.py"],
            cwd=REPOSITORY_ROOT,
            env=environment,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("verified 40 fixed document fixtures", result.stdout)
        self.assertIn("verified 15 deterministic hostile fixtures", result.stdout)


if __name__ == "__main__":
    unittest.main()
