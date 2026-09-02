# Fixed document parser corpus

This directory vendors exact bytes for 40 real documents from fixed revisions of
Apache Tika, LangChain4j, Apache POI, and Apache PDFBox. The corpus is independent
of Xberg and covers PDF, DOCX, XLSX, XLSM, XLSB, XLS, ODS, ODT, and Apple Numbers.

`manifest.json` is the authority for every fixture's byte size, SHA-256, upstream
Git blob, exact raw URL, MIME, format, language/script review, license evidence,
and semantic oracle. Each `expected/*.json` records reviewed text or structure,
stable failure classification, and retrieval probes. `golden-digests.json`
freezes the normalized Markdown SHA-256 for every successful fixture as an
independent reviewed authority; parser tests compare every successful output
against it exactly and reject missing or extra entries.

All four source repositories use Apache-2.0. At the pinned revisions, a Git-tree
audit found no fixture-local LICENSE, NOTICE, or README with additional terms in
the selected fixture paths. The exact project license and applicable NOTICE are
retained under `licenses/`. LangChain4j has no NOTICE at the pinned revision, so
its retained LICENSE is also the attribution reference.

Ordinary tests are offline:

```text
python3 test/check-document-fixtures.py
```

The maintainer-only importer requires explicit fixture IDs, downloads only exact
manifest URLs from `raw.githubusercontent.com`, refuses host-changing redirects,
HTML responses, Git LFS pointers, oversize content, hash mismatch, and overwrite,
and leaves verified bytes under `.temp/document-fixture-import/downloads/`:

```text
python3 test/import-document-fixtures.py --id tika-pdf-basic
```

It deliberately does not copy downloaded bytes into this tracked directory and
does not accept license terms automatically.

No real WPS `.et` file was vendored: the investigated design names a SheetJS
candidate, but this corpus has no fixed-revision, per-file redistribution evidence
for it. `.et` must remain a capability deviation. Renaming an XLS file would test
magic/extension mismatch but would not prove `.et` support, so it is not counted
as a real-format fixture.
