# Security policy

## Supported versions

Security fixes target the latest published [open-compute release](https://github.com/elliothux/open-compute/releases/latest). Older release lines do not receive security backports. Development builds are not supported releases.

If you found a vulnerability in an older version, report it even if you cannot verify it on the current release.

## Report a vulnerability

Email [elliothu.my@gmail.com](mailto:elliothu.my@gmail.com) with the subject `open-compute security report`. Do not disclose vulnerabilities in public issues, pull requests, or discussions.

Include:

- The affected release or commit, operating system, and CPU architecture.
- The affected component, required privileges, and deployment conditions.
- A minimal reproduction using synthetic data, expected and actual behavior, and potential impact.
- Sanitized logs or a proof of concept, if available.

Do not send real credentials, private keys, production databases, or other users' data. Test only systems you own or have permission to assess.

The maintainer will review the report and coordinate a fix and public disclosure with the reporter. Please allow time for investigation and a fix before publishing exploit details. Response and resolution times depend on maintainer availability; there is no guaranteed response deadline.

## Security boundaries

open-compute is a self-hosted platform for one machine. Reports may concern `ocd`, the Dashboard, runtime assets, or the pinned runtime components shipped in a release. Security properties include:

- Authorization before control-plane access or mutation, and authenticated internal listeners.
- Isolation of tenant bindings, credentials, deployment identity, and instance data.
- Protection of secrets in storage, responses, logs, process arguments, and artifacts.
- Verified runtime inputs, immutable deployments, and persistence integrity across restart and crash recovery.
- Bounded processing of untrusted requests, documents, and images.

Tenant outbound connections can reach public, private, loopback, link-local, and metadata IP addresses allowed by the host. Operators own destination filtering through the host firewall, network namespace, container, or VM. open-compute does not provide per-Worker or per-instance network isolation. Platform-owned listeners must authenticate independently of source address.

See the [threat model](docs/security.md) for the adversaries open-compute defends against, and the [compatibility guide](https://open-compute.dev/docs/platform/compatibility/) for documented platform limits. A documented limit does not exclude a report about a separate authorization, isolation, secret-handling, or integrity failure.
