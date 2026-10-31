# Contributing to open-compute

Bug reports, documentation fixes, compatibility reproductions, and focused code changes are welcome. Follow the [code of conduct](CODE_OF_CONDUCT.md). Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

## Issues and proposals

Search existing issues and check the [compatibility guide](https://open-compute.dev/docs/platform/compatibility/) before opening an issue. Use the Bug report or Feature request template. Include a minimal reproduction for bugs and describe the user problem for proposals.

Discuss substantial features, dependency additions, architecture changes, or runtime pin changes in an issue before implementing them. Keep proposals within the project's self-hosted, single-machine scope.

## Development setup

Read [AGENTS.md](AGENTS.md) and any applicable nested `AGENTS.md` before changing code. It lists the build and test commands, the invariants, and how changes are judged. Security changes follow the [threat model](docs/security.md).

Use the Rust toolchain in [rust-toolchain.toml](rust-toolchain.toml), the Bun version and workspace dependencies in [package.json](package.json), and Git LFS. The test runner requires Python 3.11 or later; JavaScript tests use Node.js.

Initialize the build submodule with `git submodule update --init --depth 1 third_party/gitserver`, as in the [CI setup action](.github/actions/setup-open-compute/action.yml). Follow the [source build guide](docs/references/single-binary.md) to obtain Git LFS assets, install locked dependencies, and explicitly prepare workerd and Caddy inputs before building. Runtime inputs must match the formal locks; generated runtime assets are not committed.

## Changes and validation

Keep each pull request focused on one problem. Update affected consumers, documentation, and focused success and failure tests together. Preserve unrelated changes and retained failure evidence. Never include credentials, local data, or generated build output.

Follow the [testing guide](docs/references/testing.md) and run the tests for what you changed. Fix failures before rerunning. Real-runtime tests require the verified, formally pinned workerd binary. Cloudflare and other external mutation tests require explicit authorization and dedicated resources.

For documentation-only changes, run `git diff --check` and verify links, commands, and claims against the repository. Rust checks and runtime Gates are not required for documentation-only changes.

Published platform database migrations are immutable. Runtime pin changes must follow the coordinated update process in the [workerd guide](docs/workerd/README.md).

## Pull requests

Use the pull request template to describe the problem, resulting behavior, linked issue, and validation commands and results. State checks you could not run and why. Call out changes to security boundaries, persistence, Cloudflare compatibility, or runtime pins when applicable.

Write commit messages, issues, pull requests, and reviews in English. Small documentation and typo fixes can be submitted directly. Contributions are provided under the repository's [Apache-2.0 license](LICENSE).
