#!/usr/bin/env python3
"""Offline integrity and provenance checks for the document parser corpora."""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
FIXTURE_ROOT = REPOSITORY_ROOT / "test/fixtures/document-parser"
MANIFEST_PATH = FIXTURE_ROOT / "manifest.json"
GOLDEN_DIGESTS_PATH = FIXTURE_ROOT / "golden-digests.json"
EXPECTED_REVISIONS = {
    "https://github.com/apache/tika": "1e3d8f888380d8b302ce4787bc7d5fbb513f1867",
    "https://github.com/langchain4j/langchain4j": "b0b3b21e5f5679e86519ef3d979b7fbea0769f13",
    "https://github.com/apache/poi": "6d94ace657249b487959dd654ca9d9b1c6014e4e",
    "https://github.com/apache/pdfbox": "44ae1f5a0371c37128b20fac2beecdfd0c93b503",
}
HEX_40 = re.compile(r"[0-9a-f]{40}\Z")
HEX_64 = re.compile(r"[0-9a-f]{64}\Z")


def fail(message: str) -> None:
    raise ValueError(message)


def load_json(path: Path) -> object:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"cannot read JSON {path.relative_to(REPOSITORY_ROOT)}: {error}")


def check_manifest() -> int:
    manifest = load_json(MANIFEST_PATH)
    if not isinstance(manifest, dict) or manifest.get("schema_version") != 1:
        fail("document fixture manifest has an unsupported schema")
    policy = manifest.get("corpus_policy")
    if not isinstance(policy, dict) or policy.get("network_required_for_tests") is not False or policy.get("git_lfs_allowed") is not False:
        fail("document fixture policy must freeze offline, non-LFS tests")
    fixtures = manifest.get("fixtures")
    if not isinstance(fixtures, list) or len(fixtures) < 30:
        fail("document fixture corpus must contain at least 30 entries")

    ids: set[str] = set()
    paths: set[str] = set()
    oracle_paths: set[str] = set()
    format_counts: Counter[str] = Counter()
    successful_ids: set[str] = set()
    for raw_entry in fixtures:
        if not isinstance(raw_entry, dict):
            fail("document fixture entry is not an object")
        entry = raw_entry
        fixture_id = entry.get("id")
        relative_path = entry.get("path")
        if not isinstance(fixture_id, str) or not fixture_id or fixture_id in ids:
            fail(f"invalid or duplicate fixture id: {fixture_id!r}")
        if not isinstance(relative_path, str) or relative_path in paths or not relative_path.startswith("corpus/"):
            fail(f"invalid or duplicate fixture path for {fixture_id}: {relative_path!r}")
        ids.add(fixture_id)
        paths.add(relative_path)
        path = FIXTURE_ROOT / relative_path
        if not path.is_file() or path.is_symlink():
            fail(f"fixture is missing, not a regular file, or a symlink: {relative_path}")
        data = path.read_bytes()
        if data.startswith(b"version https://git-lfs.github.com/spec/"):
            fail(f"Git LFS pointer is forbidden: {relative_path}")
        if entry.get("size_bytes") != len(data):
            fail(f"size mismatch: {relative_path}")
        digest = hashlib.sha256(data).hexdigest()
        if entry.get("sha256") != digest:
            fail(f"SHA-256 mismatch: {relative_path}")

        document_format = entry.get("format")
        if not isinstance(document_format, str) or path.suffix.lower() != f".{document_format}":
            fail(f"format/extension mismatch in manifest: {fixture_id}")
        format_counts[document_format] += 1
        if not isinstance(entry.get("mime"), str) or not entry["mime"]:
            fail(f"missing MIME: {fixture_id}")
        for key in ("languages", "scripts"):
            value = entry.get(key)
            if not isinstance(value, list) or not value or not all(isinstance(item, str) and item for item in value):
                fail(f"missing reviewed {key}: {fixture_id}")

        repository = entry.get("source_repository")
        revision = entry.get("source_revision")
        source_path = entry.get("source_path")
        if repository not in EXPECTED_REVISIONS or revision != EXPECTED_REVISIONS[repository]:
            fail(f"unapproved source repository or revision: {fixture_id}")
        if not isinstance(source_path, str) or not source_path or source_path.startswith("/") or ".." in Path(source_path).parts:
            fail(f"invalid source path: {fixture_id}")
        slug = repository.removeprefix("https://github.com/")
        expected_url = f"https://raw.githubusercontent.com/{slug}/{revision}/{source_path}"
        if entry.get("source_url") != expected_url:
            fail(f"source URL is not the exact fixed-revision raw URL: {fixture_id}")
        blob = entry.get("source_git_blob_sha1")
        if not isinstance(blob, str) or HEX_40.fullmatch(blob) is None:
            fail(f"missing fixed Git blob identity: {fixture_id}")
        git_blob = hashlib.sha1(f"blob {len(data)}\0".encode("ascii") + data, usedforsecurity=False).hexdigest()
        if git_blob != blob:
            fail(f"fixture bytes do not match the recorded upstream Git blob: {fixture_id}")

        if entry.get("license") != "Apache-2.0":
            fail(f"unapproved fixture license: {fixture_id}")
        audit = entry.get("license_audit")
        if not isinstance(audit, dict) or audit.get("status") != "reviewed" or not audit.get("evidence"):
            fail(f"fixture lacks per-file license audit evidence: {fixture_id}")
        for key in ("license_file", "attribution"):
            license_path = entry.get(key)
            if not isinstance(license_path, str) or not license_path.startswith("licenses/"):
                fail(f"invalid {key}: {fixture_id}")
            tracked_license = FIXTURE_ROOT / license_path
            if not tracked_license.is_file() or tracked_license.is_symlink():
                fail(f"missing {key}: {fixture_id}")

        oracle_path = entry.get("oracle")
        if not isinstance(oracle_path, str) or oracle_path != f"expected/{fixture_id}.json":
            fail(f"oracle path does not match fixture id: {fixture_id}")
        oracle_paths.add(oracle_path)
        oracle = load_json(FIXTURE_ROOT / oracle_path)
        if not isinstance(oracle, dict):
            fail(f"oracle is not an object: {fixture_id}")
        expected_status = entry.get("expected_status")
        if expected_status == "ok":
            if oracle.get("status") != "ok" or oracle.get("error") is not None:
                fail(f"success oracle status mismatch: {fixture_id}")
            successful_ids.add(fixture_id)
        elif not isinstance(expected_status, str) or oracle.get("status") != "error" or oracle.get("error") != expected_status:
            fail(f"error oracle status mismatch: {fixture_id}")
        for key in ("must_contain", "must_not_contain", "retrieval_queries"):
            if not isinstance(oracle.get(key), list):
                fail(f"oracle {key} is not a list: {fixture_id}")
        retrieval_queries = oracle["retrieval_queries"]
        if expected_status == "ok" and not retrieval_queries:
            fail(f"success oracle lacks a retrieval query: {fixture_id}")
        if expected_status != "ok" and retrieval_queries:
            fail(f"error oracle must not advertise retrieval queries: {fixture_id}")
        for probe in retrieval_queries:
            if (
                not isinstance(probe, dict)
                or set(probe) != {"query", "expected_fixture_id"}
                or not isinstance(probe.get("query"), str)
                or not probe["query"]
                or probe.get("expected_fixture_id") != fixture_id
            ):
                fail(f"invalid retrieval query: {fixture_id}")
        if not isinstance(oracle.get("review_notes"), str) or not oracle["review_notes"]:
            fail(f"oracle lacks human review notes: {fixture_id}")
        structure = oracle.get("structure")
        if not isinstance(structure, dict) or not isinstance(structure.get("sheet_names"), list):
            fail(f"oracle structure is incomplete: {fixture_id}")
        if "normalized_markdown_sha256" in oracle:
            fail(f"per-oracle normalized Markdown digest is obsolete: {fixture_id}")

    actual_paths = {
        path.relative_to(FIXTURE_ROOT).as_posix()
        for path in (FIXTURE_ROOT / "corpus").rglob("*")
        if path.is_file()
    }
    if actual_paths != paths:
        fail(f"corpus/manifest path set mismatch: missing={sorted(paths - actual_paths)}, extra={sorted(actual_paths - paths)}")
    actual_oracle_paths = {
        path.relative_to(FIXTURE_ROOT).as_posix()
        for path in (FIXTURE_ROOT / "expected").rglob("*.json")
        if path.is_file()
    }
    if actual_oracle_paths != oracle_paths:
        fail(
            "expected/manifest path set mismatch: "
            f"missing={sorted(oracle_paths - actual_oracle_paths)}, extra={sorted(actual_oracle_paths - oracle_paths)}"
        )
    for document_format, minimum in {"pdf": 2, "docx": 2, "xlsx": 2, "xlsm": 2, "xlsb": 2, "xls": 2, "ods": 2, "odt": 2, "numbers": 2}.items():
        if format_counts[document_format] < minimum:
            fail(f"format {document_format} requires at least {minimum} fixed fixtures")

    golden_digests = load_json(GOLDEN_DIGESTS_PATH)
    if not isinstance(golden_digests, dict):
        fail("document parser golden digests must be an object")
    if set(golden_digests) != successful_ids:
        fail(
            "golden/success fixture id set mismatch: "
            f"missing={sorted(successful_ids - set(golden_digests))}, "
            f"extra={sorted(set(golden_digests) - successful_ids)}"
        )
    for fixture_id, digest in golden_digests.items():
        if not isinstance(digest, str) or HEX_64.fullmatch(digest) is None:
            fail(f"invalid golden Markdown digest: {fixture_id}")
    return len(fixtures)


def main() -> int:
    try:
        count = check_manifest()
        subprocess.run(
            [sys.executable, str(REPOSITORY_ROOT / "test/fuzz/corpus/document-parser/generate.py"), "--check"],
            cwd=REPOSITORY_ROOT,
            check=True,
        )
    except (ValueError, subprocess.CalledProcessError) as error:
        print(f"document fixture integrity check failed: {error}", file=sys.stderr)
        return 1
    digest = hashlib.sha256(MANIFEST_PATH.read_bytes()).hexdigest()
    print(f"verified {count} fixed document fixtures; manifest sha256={digest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
