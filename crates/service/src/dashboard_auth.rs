//! One-time Dashboard login codes and short-lived browser sessions.

use open_compute_core::{ErrorCode, PlatformError, StartupId};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LOGIN_CODE_BYTES: usize = 32;
const SESSION_TOKEN_BYTES: usize = 32;
const LOGIN_CODE_TTL: Duration = Duration::from_secs(30);
const SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// Shared Dashboard authentication state for one process generation.
#[derive(Debug)]
pub struct DashboardAuth {
    startup_id: StartupId,
    inner: Mutex<DashboardAuthInner>,
}

#[derive(Debug, Default)]
struct DashboardAuthInner {
    login_codes: HashMap<String, LoginCodeRecord>,
    sessions: HashMap<String, SessionRecord>,
}

#[derive(Debug)]
struct LoginCodeRecord {
    expires_at_ms: u64,
    consumed: bool,
}

#[derive(Debug)]
struct SessionRecord {
    expires_at_ms: u64,
}

/// Response returned when minting a one-time login code.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginCodeIssue {
    /// One-time code placed in the Dashboard URL fragment.
    pub code: String,
    /// Absolute expiry in Unix milliseconds.
    pub expires_at_ms: u64,
}

/// Response returned when exchanging a code or admin token for a browser session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionIssue {
    /// Short-lived browser session token (not the long-lived admin token).
    pub session_token: String,
    /// Absolute expiry in Unix milliseconds.
    pub expires_at_ms: u64,
}

impl DashboardAuth {
    /// Create auth state bound to the current startup generation.
    #[must_use]
    pub fn new(startup_id: StartupId) -> Self {
        Self {
            startup_id,
            inner: Mutex::new(DashboardAuthInner::default()),
        }
    }

    /// Startup generation this store belongs to.
    #[must_use]
    pub fn startup_id(&self) -> StartupId {
        self.startup_id
    }

    /// Mint a one-time login code valid for about 30 seconds.
    pub fn issue_login_code(&self, now: SystemTime) -> Result<LoginCodeIssue, PlatformError> {
        let expires_at_ms = deadline_ms(now, LOGIN_CODE_TTL)?;
        let code = random_token(LOGIN_CODE_BYTES)?;
        let mut inner = self.lock()?;
        inner.purge(now_ms(now)?);
        inner.login_codes.insert(
            code.clone(),
            LoginCodeRecord {
                expires_at_ms,
                consumed: false,
            },
        );
        Ok(LoginCodeIssue {
            code,
            expires_at_ms,
        })
    }

    /// Consume a one-time login code and mint a browser session.
    pub fn exchange_login_code(
        &self,
        code: &str,
        now: SystemTime,
    ) -> Result<SessionIssue, PlatformError> {
        let now_ms = now_ms(now)?;
        let mut inner = self.lock()?;
        inner.purge(now_ms);
        let Some(record) = inner.login_codes.get(code) else {
            return Err(PlatformError::new(
                ErrorCode::AdminAuthRequired,
                "dashboard login code is invalid",
            ));
        };
        if record.consumed || record.expires_at_ms <= now_ms {
            return Err(PlatformError::new(
                ErrorCode::AdminAuthRequired,
                "dashboard login code is expired or already used",
            ));
        }
        inner.login_codes.remove(code);
        Self::mint_session_locked(&mut inner, now)
    }

    /// Mint a browser session after verifying a long-lived admin token elsewhere.
    pub fn issue_session_from_admin(&self, now: SystemTime) -> Result<SessionIssue, PlatformError> {
        let mut inner = self.lock()?;
        inner.purge(now_ms(now)?);
        Self::mint_session_locked(&mut inner, now)
    }

    /// Return true when `token` is a live browser session for this generation.
    pub fn session_valid(&self, token: &str, now: SystemTime) -> bool {
        let Ok(now_ms) = now_ms(now) else {
            return false;
        };
        let Ok(mut inner) = self.lock() else {
            return false;
        };
        inner.purge(now_ms);
        inner
            .sessions
            .get(token)
            .is_some_and(|session| session.expires_at_ms > now_ms)
    }

    /// Drop one browser session.
    pub fn revoke_session(&self, token: &str) -> Result<(), PlatformError> {
        let mut inner = self.lock()?;
        inner.sessions.remove(token);
        Ok(())
    }

    fn mint_session_locked(
        inner: &mut DashboardAuthInner,
        now: SystemTime,
    ) -> Result<SessionIssue, PlatformError> {
        let expires_at_ms = deadline_ms(now, SESSION_TTL)?;
        let session_token = random_token(SESSION_TOKEN_BYTES)?;
        inner
            .sessions
            .insert(session_token.clone(), SessionRecord { expires_at_ms });
        Ok(SessionIssue {
            session_token,
            expires_at_ms,
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, DashboardAuthInner>, PlatformError> {
        self.inner
            .lock()
            .map_err(|_| PlatformError::new(ErrorCode::Internal, "dashboard auth lock is poisoned"))
    }
}

impl DashboardAuthInner {
    fn purge(&mut self, now_ms: u64) {
        self.login_codes
            .retain(|_, record| !record.consumed && record.expires_at_ms > now_ms);
        self.sessions
            .retain(|_, session| session.expires_at_ms > now_ms);
    }
}

fn random_token(bytes: usize) -> Result<String, PlatformError> {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.try_fill_bytes(&mut buf).map_err(|_| {
        PlatformError::new(
            ErrorCode::Internal,
            "failed to generate dashboard auth material",
        )
    })?;
    Ok(hex::encode(buf))
}

fn now_ms(now: SystemTime) -> Result<u64, PlatformError> {
    let ms = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            PlatformError::new(ErrorCode::Internal, "system clock is before the Unix epoch")
        })?
        .as_millis();
    u64::try_from(ms).map_err(|_| PlatformError::new(ErrorCode::Internal, "timestamp overflow"))
}

fn deadline_ms(now: SystemTime, ttl: Duration) -> Result<u64, PlatformError> {
    let start = now_ms(now)?;
    Ok(start.saturating_add(ttl.as_millis() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_code_is_single_use_and_expires() {
        let auth = DashboardAuth::new(StartupId::generate());
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let issued = auth.issue_login_code(now).unwrap();
        let session = auth.exchange_login_code(&issued.code, now).unwrap();
        assert!(auth.session_valid(&session.session_token, now));
        assert!(auth.exchange_login_code(&issued.code, now).is_err());
        let expired = auth.issue_login_code(now).unwrap();
        let later = now + LOGIN_CODE_TTL + Duration::from_secs(1);
        assert!(auth.exchange_login_code(&expired.code, later).is_err());
    }

    #[test]
    fn admin_minted_session_can_be_revoked() {
        let auth = DashboardAuth::new(StartupId::generate());
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let session = auth.issue_session_from_admin(now).unwrap();
        assert!(auth.session_valid(&session.session_token, now));
        auth.revoke_session(&session.session_token).unwrap();
        assert!(!auth.session_valid(&session.session_token, now));
    }

    #[test]
    fn startup_id_and_pre_epoch_paths() {
        let startup = StartupId::generate();
        let auth = DashboardAuth::new(startup);
        assert_eq!(auth.startup_id(), startup);
        let pre = UNIX_EPOCH - Duration::from_secs(1);
        assert!(auth.issue_login_code(pre).is_err());
        assert!(!auth.session_valid("missing", pre));
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let session = auth.issue_session_from_admin(now).unwrap();
        let later = now + SESSION_TTL + Duration::from_secs(1);
        assert!(!auth.session_valid(&session.session_token, later));
        assert!(auth.exchange_login_code("no-such-code", now).is_err());
    }
}
