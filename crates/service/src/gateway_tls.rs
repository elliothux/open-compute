//! TLS readiness probe for the managed Worker wildcard.

use open_compute_core::{ErrorCode, PlatformError};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

/// Verify the wildcard certificate, SNI route, and private HTTP upstream.
pub(crate) async fn probe_worker_gateway(
    listen: SocketAddr,
    base_domain: &str,
    timeout: Duration,
) -> Result<(), PlatformError> {
    crate::tls::install_default_provider();
    let mut roots = RootCertStore::empty();
    for root in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        roots.add(root.clone()).map_err(|_| unavailable())?;
    }
    probe_with_roots(listen, base_domain, timeout, roots).await
}

async fn probe_with_roots(
    listen: SocketAddr,
    base_domain: &str,
    timeout: Duration,
    roots: RootCertStore,
) -> Result<(), PlatformError> {
    let address = connect_address(listen);
    let hostname = format!("probe.{base_domain}");
    tokio::task::spawn_blocking(move || probe_blocking(address, &hostname, timeout, roots))
        .await
        .map_err(|_| unavailable())?
}

fn probe_blocking(
    address: SocketAddr,
    hostname: &str,
    timeout: Duration,
    roots: RootCertStore,
) -> Result<(), PlatformError> {
    let server_name = ServerName::try_from(hostname.to_owned()).map_err(|_| unavailable())?;
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connection =
        ClientConnection::new(Arc::new(config), server_name).map_err(|_| unavailable())?;
    let stream = TcpStream::connect_timeout(&address, timeout).map_err(|_| unavailable())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|_| unavailable())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|_| unavailable())?;
    let mut tls = StreamOwned::new(connection, stream);
    write!(
        tls,
        "GET /__open_compute_gateway_probe__ HTTP/1.1\r\nHost: {hostname}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|_| unavailable())?;
    tls.flush().map_err(|_| unavailable())?;
    let mut response = Vec::new();
    while response.len() <= 4096
        && !response
            .windows(b"\r\n\r\n".len())
            .any(|window| window == b"\r\n\r\n")
    {
        let mut chunk = [0u8; 512];
        let read = tls.read(&mut chunk).map_err(|_| unavailable())?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read]);
    }
    let valid_status =
        response.starts_with(b"HTTP/1.1 204 ") || response.starts_with(b"HTTP/1.0 204 ");
    response.make_ascii_lowercase();
    if response.len() > 4096
        || !valid_status
        || !response
            .windows(b"\r\nx-open-compute-gateway-probe: 1\r\n".len())
            .any(|window| window == b"\r\nx-open-compute-gateway-probe: 1\r\n")
    {
        return Err(unavailable());
    }
    Ok(())
}

fn connect_address(listen: SocketAddr) -> SocketAddr {
    match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), listen.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), listen.port())
        }
        _ => listen,
    }
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ServiceUnavailable,
        "public gateway TLS qualification failed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use rustls::{ServerConfig, ServerConnection};
    use std::net::TcpListener;

    #[tokio::test]
    async fn tls_probe_verifies_sni_chain_and_http_response() {
        crate::tls::install_default_provider();
        let certificate =
            CertificateDer::from(include_bytes!("../../../test/gateway/probe-cert.der").to_vec());
        let root =
            CertificateDer::from(include_bytes!("../../../test/gateway/probe-root.der").to_vec());
        let private_key =
            PrivateKeyDer::try_from(include_bytes!("../../../test/gateway/probe-key.der").to_vec())
                .unwrap();
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], private_key)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let connection = ServerConnection::new(Arc::new(config)).unwrap();
            let mut tls = StreamOwned::new(connection, stream);
            let mut request = [0u8; 256];
            assert!(tls.read(&mut request).unwrap() > 0);
            tls.write_all(
                b"HTTP/1.1 204 No Content\r\nX-Open-Compute-Gateway-Probe: 1\r\nContent-Length: 0\r\n\r\n",
            )
            .unwrap();
            tls.flush().unwrap();
        });
        let mut roots = RootCertStore::empty();
        roots.add(root).unwrap();
        probe_with_roots(
            address,
            "compute.example.com",
            Duration::from_secs(2),
            roots,
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(
            connect_address("0.0.0.0:443".parse().unwrap()),
            "127.0.0.1:443".parse().unwrap()
        );
    }
}
