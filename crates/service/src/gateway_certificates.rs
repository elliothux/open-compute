//! Persistent evidence that a managed Gateway domain has served TLS successfully.

use open_compute_core::{ErrorCode, PlatformError, PublicGatewayConfig};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(crate) fn initialize_registry(gateway_dir: &Path) -> Result<(), PlatformError> {
    open_compute_storage::ensure_dir_secure(&registry(gateway_dir))?;
    open_compute_storage::ensure_dir_secure(&attempted_registry(gateway_dir))
}

pub(crate) fn check_registry(gateway_dir: &Path) -> Result<(), PlatformError> {
    validate_dir(&registry(gateway_dir))?;
    validate_dir(&attempted_registry(gateway_dir))
}

pub(crate) fn check_certified_domains(
    gateway_dir: &Path,
    domains: &[String],
) -> Result<(), PlatformError> {
    check_registry(gateway_dir)?;
    for domain in domains {
        PublicGatewayConfig::validate_base_domain(domain)?;
        let certified = check_marker(&marker(gateway_dir, domain), domain)?;
        let attempted = check_marker(&attempted_marker(gateway_dir, domain), domain)?;
        if (certified || attempted) && !has_certificate_site(gateway_dir, domain)? {
            return Err(incomplete());
        }
    }
    Ok(())
}

pub(crate) fn record_certified_domain(
    gateway_dir: &Path,
    domain: &str,
) -> Result<(), PlatformError> {
    check_registry(gateway_dir)?;
    PublicGatewayConfig::validate_base_domain(domain)?;
    let marker = marker(gateway_dir, domain);
    match fs::symlink_metadata(&marker) {
        Ok(_) => return check_certified_domains(gateway_dir, &[domain.to_owned()]),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(incomplete()),
    }
    open_compute_storage::atomic_write(&marker, domain.as_bytes()).map_err(|_| incomplete())?;
    check_certified_domains(gateway_dir, &[domain.to_owned()])
}

pub(crate) fn record_issuance_attempt(
    gateway_dir: &Path,
    domain: &str,
) -> Result<(), PlatformError> {
    check_registry(gateway_dir)?;
    PublicGatewayConfig::validate_base_domain(domain)?;
    let path = attempted_marker(gateway_dir, domain);
    if check_marker(&path, domain)? {
        return Ok(());
    }
    open_compute_storage::atomic_write(&path, domain.as_bytes()).map_err(|_| incomplete())
}

fn check_marker(path: &Path, domain: &str) -> Result<bool, PlatformError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            open_compute_storage::validate_owned_file(path, true).map_err(|_| incomplete())?;
            if fs::read(path).map_err(|_| incomplete())? != domain.as_bytes() {
                return Err(incomplete());
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(incomplete()),
    }
}

pub(crate) fn check_present_certificates(storage: &Path) -> Result<(), PlatformError> {
    let certificates = storage.join("certificates");
    if !exists(&certificates)? {
        return Ok(());
    }
    validate_dir(&certificates)?;
    for issuer in fs::read_dir(&certificates).map_err(|_| incomplete())? {
        let issuer = issuer.map_err(|_| incomplete())?.path();
        validate_dir(&issuer)?;
        for site in fs::read_dir(&issuer).map_err(|_| incomplete())? {
            validate_site(&site.map_err(|_| incomplete())?.path())?;
        }
    }
    Ok(())
}

fn has_certificate_site(gateway_dir: &Path, domain: &str) -> Result<bool, PlatformError> {
    let certificates = gateway_dir.join("storage/certificates");
    if !exists(&certificates)? {
        return Ok(false);
    }
    validate_dir(&certificates)?;
    // CertMagic v0.25.3 StorageKeys.Safe("*.example.com") is "wildcard_.example.com".
    let site_name = format!("wildcard_.{domain}");
    for issuer in fs::read_dir(&certificates).map_err(|_| incomplete())? {
        let issuer = issuer.map_err(|_| incomplete())?.path();
        validate_dir(&issuer)?;
        let site = issuer.join(&site_name);
        if exists(&site)? {
            validate_site(&site)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn validate_site(site: &Path) -> Result<(), PlatformError> {
    validate_dir(site)?;
    let name = site
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(incomplete)?;
    for suffix in ["crt", "key", "json"] {
        let file = site.join(format!("{name}.{suffix}"));
        open_compute_storage::validate_owned_file(&file, true).map_err(|_| incomplete())?;
        if fs::metadata(&file).map_err(|_| incomplete())?.len() == 0 {
            return Err(incomplete());
        }
    }
    Ok(())
}

fn validate_dir(path: &Path) -> Result<(), PlatformError> {
    let meta = fs::symlink_metadata(path).map_err(|_| incomplete())?;
    if !meta.is_dir()
        || meta.uid() != rustix::process::getuid().as_raw()
        || meta.permissions().mode() & 0o077 != 0
    {
        return Err(incomplete());
    }
    Ok(())
}

fn exists(path: &Path) -> Result<bool, PlatformError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(incomplete()),
    }
}

fn registry(gateway_dir: &Path) -> PathBuf {
    gateway_dir.join("config-state/certified")
}

fn attempted_registry(gateway_dir: &Path) -> PathBuf {
    gateway_dir.join("config-state/attempted")
}

fn marker(gateway_dir: &Path, domain: &str) -> PathBuf {
    registry(gateway_dir).join(hex::encode(Sha256::digest(domain.as_bytes())))
}

fn attempted_marker(gateway_dir: &Path, domain: &str) -> PathBuf {
    attempted_registry(gateway_dir).join(hex::encode(Sha256::digest(domain.as_bytes())))
}

fn incomplete() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "Gateway certificate storage is incomplete",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn certified_site_loss_is_rejected_without_issuing_again() {
        let temp = tempfile::tempdir().unwrap();
        let gateway = temp.path().join("gateway");
        let domain = "example.com".to_owned();
        let site = gateway.join("storage/certificates/issuer/wildcard_.example.com");
        for dir in [
            gateway.clone(),
            gateway.join("config-state"),
            gateway.join("storage"),
            gateway.join("storage/certificates"),
            gateway.join("storage/certificates/issuer"),
            site.clone(),
        ] {
            open_compute_storage::ensure_dir_secure(&dir).unwrap();
        }
        initialize_registry(&gateway).unwrap();
        check_certified_domains(&gateway, std::slice::from_ref(&domain)).unwrap();
        record_issuance_attempt(&gateway, &domain).unwrap();
        assert_eq!(
            check_certified_domains(&gateway, std::slice::from_ref(&domain))
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
        for suffix in ["crt", "key", "json"] {
            open_compute_storage::atomic_write(
                &site.join(format!("wildcard_.example.com.{suffix}")),
                b"asset",
            )
            .unwrap();
        }
        check_certified_domains(&gateway, std::slice::from_ref(&domain)).unwrap();
        record_certified_domain(&gateway, &domain).unwrap();
        check_certified_domains(&gateway, std::slice::from_ref(&domain)).unwrap();
        fs::remove_dir_all(&site).unwrap();
        assert_eq!(
            check_certified_domains(&gateway, std::slice::from_ref(&domain))
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
        assert_eq!(
            record_certified_domain(&gateway, &domain)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
        assert_eq!(
            fs::read(marker(&gateway, &domain)).unwrap(),
            domain.as_bytes()
        );
    }

    #[test]
    fn certified_marker_permissions_and_symlink_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let gateway = temp.path().join("gateway");
        open_compute_storage::ensure_dir_secure(&gateway).unwrap();
        open_compute_storage::ensure_dir_secure(&gateway.join("config-state")).unwrap();
        initialize_registry(&gateway).unwrap();
        assert_eq!(
            check_certified_domains(&gateway, &["../escape".to_owned()])
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
        let domain = "example.com".to_owned();
        let marker = marker(&gateway, &domain);
        symlink(gateway.join("config-state"), &marker).unwrap();
        assert_eq!(
            check_certified_domains(&gateway, &[domain])
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
        fs::remove_file(&marker).unwrap();
        let attempted = attempted_marker(&gateway, "example.com");
        symlink(gateway.join("config-state"), &attempted).unwrap();
        assert_eq!(
            check_certified_domains(&gateway, &["example.com".to_owned()])
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
        fs::remove_file(&attempted).unwrap();
        fs::remove_dir(registry(&gateway)).unwrap();
        assert_eq!(
            check_registry(&gateway).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
    }
}
