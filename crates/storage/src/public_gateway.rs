//! Single-domain authority for the optional public gateway.

use crate::ControlDb;
use crate::workers::db_error;
use open_compute_core::{ErrorCode, PlatformError, PublicGatewayConfig};
use rusqlite::{OptionalExtension, params};

/// SQLite owner of the one configured base domain and its qualified namespaces.
pub struct PublicGatewayRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> PublicGatewayRepository<'a> {
    /// Use the existing control database as authority.
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Begin onboarding the configured domain, retaining an already qualified match.
    pub fn provision(
        &self,
        config: &PublicGatewayConfig,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        config.validate()?;
        self.db.with_immediate(|tx| {
            let current: Option<(String, String)> = tx
                .query_row(
                    "SELECT base_domain_ascii, state FROM public_gateway_domains WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            if let Some((domain, _)) = &current
                && PublicGatewayConfig::validate_base_domain(domain).is_err()
            {
                return Err(PlatformError::new(
                    ErrorCode::VersionInvariantViolation,
                    "public gateway domain authority is invalid",
                ));
            }
            match current {
                Some((ref domain, ref state)) if domain == &config.base_domain => {
                    let namespace_state: Option<String> = tx
                        .query_row(
                            "SELECT state FROM public_gateway_namespaces
                             WHERE name = 'worker' AND domain_id = 1",
                            [],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(|_| db_error())?;
                    if namespace_state.is_none()
                        || (state == "disabled") != (namespace_state.as_deref() == Some("disabled"))
                    {
                        return Err(PlatformError::new(
                            ErrorCode::VersionInvariantViolation,
                            "public Worker namespace authority is missing",
                        ));
                    }
                    if state == "disabled" {
                        tx.execute(
                            "UPDATE public_gateway_domains
                             SET state = 'provisioning', generation = generation + 1,
                                 updated_at_ms = ?1 WHERE id = 1",
                            [now_ms],
                        )
                        .map_err(|_| db_error())?;
                        tx.execute(
                            "UPDATE public_gateway_namespaces
                             SET state = 'provisioning', generation = generation + 1,
                                 qualified_at_ms = NULL, updated_at_ms = ?1
                             WHERE name = 'worker' AND domain_id = 1",
                            [now_ms],
                        )
                        .map_err(|_| db_error())?;
                    }
                    return Ok(());
                }
                Some(_) => {
                    let active: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM hostname_claims
                             WHERE exposure = 'public' AND state = 'active')",
                            [],
                            |row| row.get(0),
                        )
                        .map_err(|_| db_error())?;
                    if active {
                        return Err(PlatformError::new(
                            ErrorCode::RouteConflict,
                            "disable public Worker origins before changing the base domain",
                        ));
                    }
                    tx.execute("DELETE FROM public_gateway_namespaces", [])
                        .map_err(|_| db_error())?;
                    tx.execute(
                        "UPDATE public_gateway_domains
                         SET base_domain_ascii = ?1, state = 'provisioning',
                             generation = generation + 1, updated_at_ms = ?2 WHERE id = 1",
                        params![config.base_domain, now_ms],
                    )
                    .map_err(|_| db_error())?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO public_gateway_domains
                         (id, base_domain_ascii, state, generation, updated_at_ms)
                         VALUES(1, ?1, 'provisioning', 1, ?2)",
                        params![config.base_domain, now_ms],
                    )
                    .map_err(|_| db_error())?;
                }
            }
            tx.execute(
                "INSERT INTO public_gateway_namespaces
                 (name, domain_id, state, generation, qualified_at_ms, updated_at_ms)
                 VALUES('worker', 1, 'provisioning', 1, NULL, ?1)",
                [now_ms],
            )
            .map_err(|_| db_error())?;
            Ok(())
        })
    }

    /// Disable the retained domain only when no public Worker binding remains active.
    pub fn disable(&self, now_ms: i64) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let active: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM hostname_claims
                     WHERE exposure = 'public' AND state = 'active')",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if active {
                return Err(PlatformError::new(
                    ErrorCode::RouteConflict,
                    "disable public Worker origins before removing the gateway",
                ));
            }
            let current: Option<(String, String)> = tx
                .query_row(
                    "SELECT base_domain_ascii, state FROM public_gateway_domains WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some((domain, state)) = current else {
                return Ok(());
            };
            if PublicGatewayConfig::validate_base_domain(&domain).is_err() {
                return Err(PlatformError::new(
                    ErrorCode::VersionInvariantViolation,
                    "public gateway domain authority is invalid",
                ));
            }
            let namespace_state: Option<String> = tx
                .query_row(
                    "SELECT state FROM public_gateway_namespaces
                     WHERE name = 'worker' AND domain_id = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| db_error())?;
            if namespace_state.is_none()
                || (state == "disabled") != (namespace_state.as_deref() == Some("disabled"))
            {
                return Err(PlatformError::new(
                    ErrorCode::VersionInvariantViolation,
                    "public Worker namespace authority is missing or inconsistent",
                ));
            }
            if state == "disabled" {
                return Ok(());
            }
            let changed = tx
                .execute(
                    "UPDATE public_gateway_namespaces
                     SET state = 'disabled', generation = generation + 1,
                         qualified_at_ms = NULL, updated_at_ms = ?1
                     WHERE domain_id = 1",
                    [now_ms],
                )
                .map_err(|_| db_error())?;
            if changed == 0 {
                return Err(PlatformError::new(
                    ErrorCode::VersionInvariantViolation,
                    "public Worker namespace authority is missing",
                ));
            }
            tx.execute(
                "UPDATE public_gateway_domains
                 SET state = 'disabled', generation = generation + 1,
                     updated_at_ms = ?1 WHERE id = 1",
                [now_ms],
            )
            .map_err(|_| db_error())?;
            Ok(())
        })
    }

    /// Admit Worker public origins only after independent DNS and TLS qualification.
    pub fn activate_workers(
        &self,
        base_domain: &str,
        qualified_at_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE public_gateway_domains
                     SET state = 'active', generation = generation + 1,
                         updated_at_ms = ?1
                     WHERE id = 1 AND base_domain_ascii = ?2
                       AND state IN ('provisioning', 'degraded', 'active')",
                    params![qualified_at_ms, base_domain],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::ConfigInvalid,
                    "public gateway domain is not provisioned",
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE public_gateway_namespaces
                 SET state = 'active', generation = generation + 1,
                     qualified_at_ms = ?1, updated_at_ms = ?1
                 WHERE name = 'worker' AND domain_id = 1
                   AND state IN ('provisioning', 'degraded', 'active')",
                    [qualified_at_ms],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::VersionInvariantViolation,
                    "public Worker namespace authority is missing",
                ));
            }
            Ok(())
        })
    }
}

impl std::fmt::Debug for PublicGatewayRepository<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("PublicGatewayRepository").finish()
    }
}
