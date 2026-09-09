# Fixed Pyodide bundle dependency

`pyodide_314.0.6_2026-08-17_2.capnp.bin.gz` is the platform-independent Python runtime bundle
used by the formally pinned workerd revision. It is a Git LFS build input, not a release sidecar.
The sole authority is [`workerd.lock.json`](../../packages/runtime/workerd.lock.json), which records
the version, file names, compressed SHA-256, decompressed SHA-256, workerd Bazel target, and pinned
gzip compressor identity.

The decompressed bytes are produced by
`//src/pyodide:pyodide.capnp.bin@rule@314.0.6` at workerd revision
`b3e1a27840299f493d9425dc4d9972381d02ef23`. Their SHA-256 is
`c7bae5aa740a62c5ad817d103f404155b248eab3f45cbd52b8c5e5f7d8588441`, matching the integrity
declared by that workerd source. Bun 1.3.14 `node:zlib` gzip level 9 with timestamp zero, no
filename, and OS marker 255 produces the checked-in 6,321,505-byte archive with SHA-256
`5c6c15ece332f7d473cda43343cc9d58e06435a659f38fc9851e67ce89c88b8f`.

Root `bun run build` verifies the LFS object and both digests. Cargo embeds the verified gzip in
the native `ocd` executable. Under the data-directory lock, runtime materialization decompresses
it into the private content-addressed runtime package and passes that directory to workerd through
`--pyodide-bundle-disk-cache-dir`. Production startup and Python execution do not download it.
