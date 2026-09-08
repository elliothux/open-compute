# Fixed workerd build dependency

These executable files are Git LFS objects, built from the open-compute workerd fork. The first
three are official release inputs; the macOS Intel input is retained for explicit manual builds
and is not published:

| Directory | Target |
| --- | --- |
| `darwin-arm64` | macOS ARM64 |
| `linux-arm64` | Linux GNU ARM64 |
| `linux-x64` | Linux GNU x86-64 |
| `darwin-x64` | macOS x86-64 (manual builds only) |

The sole authoritative pin is [workerd.lock.json](../../packages/runtime/workerd.lock.json).
It records the fork revision, upstream base, build inputs, binary and archive SHA-256,
version, compatibility configuration, and target identity. Do not add a second lock here.
The source is the independent [workerd submodule](../../third_party/workerd).
The workerd [Apache 2.0 license](../workerd-LICENSE) applies; the binaries also
contain upstream third-party components as recorded in the source tree.

## Checkout and build

Install Git LFS, then hydrate the fixed dependencies:

```sh
git lfs install --local
git lfs pull --include="share/workerd/**"
git lfs fsck
bun run build
```

Root `build` verifies the three official binaries and prepares their exact pinned gzip bytes using
Bun 1.3.14, level 9, no filename or timestamp, and gzip OS marker 255. The generated archives
live under `.temp/workerd-build/<target>/<archive-sha256>/`. A pointer, missing binary,
checksum mismatch, or corrupt existing archive fails the build. No runtime is downloaded.
Cargo verifies the selected target again and embeds its archive with the runtime assets.
It does not embed all binaries into one `ocd`. An Intel Mac build must explicitly prepare and pass
the pinned `darwin-x64` archive to Cargo; that path is outside the official release workflow.

For real-runtime tests, set `OPEN_COMPUTE_TEST_WORKERD` to the absolute host binary path.
`bun scripts/prepare-workerd.ts --dest /abs/new-directory` can instead create a verified
host binary/archive pair and print both test/build environment variables. See
[single-binary distribution](../../docs/references/single-binary.md).

## Updating the dependency

1. Implement and commit the fork changes in the submodule, preserving upstream conventions.
2. Build and validate the three official targets from that revision. Record upstream base, exact toolchains,
   flags, target-specific build inputs, and native test results.
3. Replace these binaries together with the authoritative lock. Generate canonical archive
   digests with the pinned compressor; do not reuse gzip digests from another compressor.
4. Run native validation, Cloudflare compatibility review, platform checks, coverage, and the
   final product Gate against the new verified pin. Update the source gitlink and evidence.
5. Stage the binaries through `.gitattributes`, then inspect `git lfs ls-files` and run
   `git lfs fsck`. An authorized Git push must upload the LFS objects as well as their pointers;
   a source-only archive containing pointers is insufficient to build.

Do not edit generated archives or silently fall back to stock workerd. LFS transfer belongs
to dependency acquisition; production startup stays offline. End users receive only the
native `ocd` executable, not this build-dependency directory.
