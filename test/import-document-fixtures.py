#!/usr/bin/env python3
"""Explicit maintainer importer for fixed document-parser fixture bytes."""

from __future__ import annotations

import argparse
import hashlib
import json
import ssl
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
FIXTURE_ROOT = REPOSITORY_ROOT / "test/fixtures/document-parser"
MANIFEST_PATH = FIXTURE_ROOT / "manifest.json"
IMPORT_ROOT = REPOSITORY_ROOT / ".temp/document-fixture-import/downloads"
MAX_BYTES = 16 * 1024 * 1024
ALLOWED_HOST = "raw.githubusercontent.com"


class FixedHostRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Reject redirects away from the one audited raw-content host."""

    def redirect_request(self, request: urllib.request.Request, file_pointer: object, code: int, message: str, headers: object, new_url: str) -> urllib.request.Request | None:
        parsed = urllib.parse.urlsplit(new_url)
        if parsed.scheme != "https" or parsed.hostname != ALLOWED_HOST:
            raise urllib.error.HTTPError(new_url, code, "redirect to non-allowlisted host", headers, file_pointer)
        return super().redirect_request(request, file_pointer, code, message, headers, new_url)


def download(entry: dict[str, object], opener: urllib.request.OpenerDirector) -> dict[str, object]:
    fixture_id = str(entry["id"])
    url = str(entry["source_url"])
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != "https" or parsed.hostname != ALLOWED_HOST or parsed.query or parsed.fragment:
        raise ValueError(f"{fixture_id}: source URL is not an exact allowed HTTPS raw URL")
    destination = IMPORT_ROOT / str(entry["path"])
    if destination.exists():
        raise FileExistsError(f"{fixture_id}: refusing to overwrite {destination.relative_to(REPOSITORY_ROOT)}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    request = urllib.request.Request(url, headers={"User-Agent": "open-compute-document-fixture-import/1"})
    with opener.open(request, timeout=30) as response:
        content_type = response.headers.get_content_type()
        if content_type == "text/html":
            raise ValueError(f"{fixture_id}: upstream returned an HTML error page")
        content_length = response.headers.get("Content-Length")
        if content_length is not None and int(content_length) > MAX_BYTES:
            raise ValueError(f"{fixture_id}: upstream content exceeds the importer limit")
        data = response.read(MAX_BYTES + 1)
    if len(data) > MAX_BYTES:
        raise ValueError(f"{fixture_id}: upstream content exceeds the importer limit")
    if data.startswith(b"version https://git-lfs.github.com/spec/"):
        raise ValueError(f"{fixture_id}: Git LFS pointer is forbidden")
    digest = hashlib.sha256(data).hexdigest()
    if len(data) != entry["size_bytes"] or digest != entry["sha256"]:
        raise ValueError(f"{fixture_id}: fixed size or SHA-256 does not match manifest")
    destination.write_bytes(data)
    return {
        "id": fixture_id,
        "path": str(destination.relative_to(REPOSITORY_ROOT)),
        "size_bytes": len(data),
        "sha256": digest,
        "source_url": url,
        "license_reviewed": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--id", action="append", required=True, help="fixture id to download; repeat as needed")
    args = parser.parse_args()
    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    by_id = {entry["id"]: entry for entry in manifest["fixtures"]}
    unknown = sorted(set(args.id) - set(by_id))
    if unknown:
        parser.error(f"unknown fixture ids: {', '.join(unknown)}")
    opener = urllib.request.build_opener(FixedHostRedirectHandler, urllib.request.HTTPSHandler(context=ssl.create_default_context()))
    reports = [download(by_id[fixture_id], opener) for fixture_id in args.id]
    print(json.dumps({"downloads": reports}, indent=2))
    print("Downloaded bytes remain under .temp; review provenance and license before copying any file into the tracked corpus.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
