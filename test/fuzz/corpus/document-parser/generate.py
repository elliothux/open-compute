#!/usr/bin/env python3
"""Generate and verify deterministic hostile document-parser corpus entries."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import struct
import tempfile
import warnings
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent
SEED_PATH = ROOT / "seed.json"
MANIFEST_PATH = ROOT / "manifest.json"
ZIP_TIMESTAMP = (1980, 1, 1, 0, 0, 0)


def zip_bytes(entries: list[tuple[str, bytes]], *, compression: int = zipfile.ZIP_DEFLATED) -> bytes:
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=compression, compresslevel=9) as archive:
        for name, data in entries:
            info = zipfile.ZipInfo(name, ZIP_TIMESTAMP)
            info.compress_type = compression
            info.create_system = 3
            info.external_attr = 0o100644 << 16
            archive.writestr(info, data)
    return output.getvalue()


def minimal_pdf(body: bytes) -> bytes:
    return b"%PDF-1.7\n% hostile deterministic fixture\n" + body + b"\n%%EOF\n"


def build_cases(seed: dict[str, object]) -> dict[str, bytes]:
    repeat_bytes = int(seed["repeat_bytes"])
    nesting_depth = int(seed["xml_nesting_depth"])
    huge = b"A" * repeat_bytes
    content_types = (
        b'<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        b'<Default Extension="xml" ContentType="application/xml"/></Types>'
    )

    duplicate = io.BytesIO()
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", UserWarning)
        with zipfile.ZipFile(duplicate, "w", compression=zipfile.ZIP_STORED) as archive:
            for payload in (b"first", b"second"):
                info = zipfile.ZipInfo("word/document.xml", ZIP_TIMESTAMP)
                info.create_system = 3
                info.external_attr = 0o100644 << 16
                archive.writestr(info, payload)

    deep_xml = b"<document>" + b"<node>" * nesting_depth + b"x" + b"</node>" * nesting_depth + b"</document>"
    entity_xml = b'<!DOCTYPE x [<!ENTITY a "ENTITY_EXPANSION_MUST_NOT_APPEAR">]><document>&a;</document>'
    rel_cycle = (
        b'<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        b'<Relationship Id="r1" Type="hostile" Target="../word/_rels/document.xml.rels"/></Relationships>'
    )
    oversized_shared = b'<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>' + huge + b"</t></si></sst>"
    spreadsheet_limits = (
        b'<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>'
        b'<row r="1048577"><c r="XFE1048577"><f>SUM(A1:A1048576)</f><v>1</v></c></row>'
        b"</sheetData></worksheet>"
    )

    ole = bytearray(1024)
    ole[:8] = bytes.fromhex("d0cf11e0a1b11ae1")
    ole[24:26] = struct.pack("<H", 0x003E)
    ole[26:28] = struct.pack("<H", 0x0003)
    ole[28:30] = struct.pack("<H", 0xFFFE)
    ole[30:32] = struct.pack("<H", 9)
    ole[44:48] = struct.pack("<I", 1)
    ole[48:52] = struct.pack("<I", 0xFFFFFF00)
    ole[76:80] = struct.pack("<I", 0xFFFFFF00)

    cases = {
        "zip-extreme-compression.docx": zip_bytes([
            ("[Content_Types].xml", content_types),
            ("word/document.xml", b"<document><text>" + huge + b"</text></document>"),
        ]),
        "zip-duplicate-entry.docx": duplicate.getvalue(),
        "xml-deep-nesting.odt": zip_bytes([("mimetype", b"application/vnd.oasis.opendocument.text"), ("content.xml", deep_xml)]),
        "xml-entity.odt": zip_bytes([("mimetype", b"application/vnd.oasis.opendocument.text"), ("content.xml", entity_xml)]),
        "ooxml-oversized-shared-strings.xlsx": zip_bytes([("[Content_Types].xml", content_types), ("xl/sharedStrings.xml", oversized_shared)]),
        "ooxml-relationship-cycle.docx": zip_bytes([("[Content_Types].xml", content_types), ("word/_rels/document.xml.rels", rel_cycle)]),
        "spreadsheet-limit-overflow.xlsx": zip_bytes([("[Content_Types].xml", content_types), ("xl/worksheets/sheet1.xml", spreadsheet_limits)]),
        "ole-truncated-sector-chain.xls": bytes(ole),
        "pdf-object-cycle.pdf": minimal_pdf(b"1 0 obj << /Type /Pages /Parent 1 0 R /Kids [1 0 R] /Count 1 >> endobj"),
        "pdf-huge-dimensions.pdf": minimal_pdf(b"1 0 obj << /Type /Page /MediaBox [0 0 2147483647 2147483647] >> endobj"),
        "pdf-nested-stream.pdf": minimal_pdf(b"1 0 obj << /Length 34 >> stream\nstream\nAAAAAAAA\nendstream\nendstream\nendobj"),
        "frame-length-overflow.bin": b"OCDP" + bytes([1, 0, 0, 0]) + struct.pack(">Q", 0xFFFFFFFFFFFFFFFF),
        "frame-invalid-utf8.bin": b"OCDP" + bytes([1, 0, 0, 0]) + struct.pack(">Q", 2) + b"\xff\xfe",
        "frame-trailing-bytes.bin": b"OCDP" + bytes([1, 0, 0, 0]) + struct.pack(">Q", 2) + b"{}TRAILING",
        "frame-wrong-digest.bin": b"OCDP" + bytes([1, 0, 0, 0]) + struct.pack(">Q", 34) + b"{}" + bytes(32),
    }
    return cases


def manifest_for(cases: dict[str, bytes], seed: dict[str, object]) -> dict[str, object]:
    expected_errors = seed["expected_errors"]
    assert isinstance(expected_errors, dict)
    return {
        "schema_version": 1,
        "generator": "generate.py",
        "seed": "seed.json",
        "cases": [
            {
                "path": name,
                "size_bytes": len(data),
                "sha256": hashlib.sha256(data).hexdigest(),
                "expected_error": expected_errors[name],
            }
            for name, data in sorted(cases.items())
        ],
    }


def write_corpus(cases: dict[str, bytes], manifest: dict[str, object]) -> None:
    tracked = {entry["path"] for entry in manifest["cases"]}  # type: ignore[index]
    for existing in ROOT.iterdir():
        if existing.is_file() and existing.name not in {"README.md", "generate.py", "manifest.json", "seed.json"} and existing.name not in tracked:
            raise SystemExit(f"refusing to leave untracked hostile fixture: {existing.name}")
    for name, data in cases.items():
        (ROOT / name).write_bytes(data)
    MANIFEST_PATH.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def check_corpus(cases: dict[str, bytes], manifest: dict[str, object]) -> None:
    expected_manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    if expected_manifest != manifest:
        raise SystemExit("hostile manifest does not match deterministic generator output")
    with tempfile.TemporaryDirectory(prefix="document-parser-hostile-") as directory:
        temp = Path(directory)
        for name, data in cases.items():
            generated = temp / name
            generated.write_bytes(data)
            tracked = ROOT / name
            if not tracked.is_file() or tracked.read_bytes() != generated.read_bytes():
                raise SystemExit(f"hostile fixture differs from deterministic output: {name}")
    print(f"verified {len(cases)} deterministic hostile fixtures")


def main() -> None:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true", help="regenerate tracked fixtures and manifest")
    mode.add_argument("--check", action="store_true", help="verify tracked fixtures without modifying them")
    args = parser.parse_args()
    seed = json.loads(SEED_PATH.read_text(encoding="utf-8"))
    cases = build_cases(seed)
    manifest = manifest_for(cases, seed)
    if args.write:
        write_corpus(cases, manifest)
    else:
        check_corpus(cases, manifest)


if __name__ == "__main__":
    main()
