//! Process-wide TLS cryptography selection.

/// Select the repository's AWS-LC rustls provider before constructing clients.
pub(crate) fn install_default_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}
