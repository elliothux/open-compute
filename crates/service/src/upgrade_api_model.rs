/// Result of a version check against the formal release authority or local cache.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct UpgradeCheckResult {
    /// Schema version for this payload.
    #[schemars(extend("minimum" = 0))]
    pub schema_version: u32,
    /// Currently running release version.
    pub current_version: String,
    /// Newest stable version known from cache or a fresh check, if any.
    #[schemars(extend("minLength" = 1))]
    pub available_version: Option<String>,
    /// Whether `available_version` is strictly newer than `current_version`.
    pub update_available: bool,
    /// Whether this install may be upgraded by the local `ocd upgrade` command.
    pub upgrade_allowed: bool,
    /// Human-safe reason when upgrade is blocked, for example package-manager ownership.
    #[schemars(extend("minLength" = 1))]
    pub blocked_reason: Option<String>,
}
