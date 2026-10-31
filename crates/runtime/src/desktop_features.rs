//! The optional features of the desktop app: switchable in Settings, and still overridable by the
//! process environment. Five are on by default (the in-app features); the three MCP capabilities
//! are off until the user opts in.
//!
//! Every optional feature is gated on a flag variable that is exactly `1` (see the loaders of the
//! feature modules), and a release app started from the Start menu or Finder has none set. The
//! desktop app therefore composes from an effective environment instead of the raw one: the process
//! environment overlaid with defaults. For each feature flag, a variable the process environment
//! sets, whatever its value, wins (so `0` or any value other than `1` turns the feature off);
//! otherwise the flag is `1` when the Settings switch of the feature is on. The CLI never calls this
//! module: it keeps its environment-only, off-by-default behaviour.
//!
//! **MCP capabilities.** The tools of the `jobniac-cli http` child that the app supervises and that
//! delete data or reach the network are `delete_profile`/`delete_job`, `add_job_from_link` and
//! `scan_arbeitnow_jobs` with its pair `import_arbeitnow_job` (the other tools read and write the
//! local database only). They are gated on the same flags as the in-app features
//! (`JOBNIAC_DELETION_ENABLED`, `JOBNIAC_LINK_FETCH_ENABLED`, `JOBNIAC_ARBEITNOW_ENABLED`), but the
//! child never inherits those flags: neither the in-app switches nor a flag of `1` in the process
//! environment (a general choice to enable the feature in the app or the CLI) reaches it. The child
//! gets such a flag only from its own Settings switch ([`Feature::McpDeletion`],
//! [`Feature::McpLinkFetch`], [`Feature::McpArbeitnow`]), which is off by default. A flag the
//! process sets to anything other than `1` stays an explicit refusal and wins over the switch
//! ([`DesktopEnvironment::supervisor_base_pairs`]).
//!
//! The capabilities that are on at startup form the [`McpGrants`] the supervisor holds. Turning a
//! switch off revokes its grant at once (the supervisor kills a child that holds it and every later
//! start leaves it out); turning one on takes effect at the next start of the app, because a
//! grant can be given only from the startup work (the keys of Arbeitnow, the refusals of the
//! process environment). The Arbeitnow keys cross to the child only with the Arbeitnow grant.
//!
//! Arbeitnow also needs two signing keys. A key the process environment does not set comes from
//! `feature-keys.json` in the per-user data directory, which is generated on first use from the OS
//! random source ([`load_or_create_keys`]). The keys never reach a log, a message or IPC.
//!
//! Generation needs a registered root. With generation on and no `JOBNIAC_GENERATED_ROOT_ID`, the
//! per-user default root is registered here at startup, the same work as `setup-generated-root`
//! ([`prepare_environment`]), before the services are composed so that composition finds the owned
//! marker. A feature that cannot be made ready stays off with one fixed warning; nothing blocks
//! startup.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use jobniac_core::env::Environment;
use jobniac_core::generated_root::RootPlatform;
use serde::{Deserialize, Serialize};

use crate::app_settings::FeatureSettings;
use crate::arbeitnow::{CANDIDATE_KEY, CURSOR_SECRET_KEY, FLAG_KEY, load_arbeitnow_config};
use crate::artifacts::DOCUMENT_ARTIFACTS_VARIABLE;
use crate::database::{database_path, prepare};
use crate::deletion::DELETION_ENABLED_VARIABLE;
use crate::generation::{
    GENERATED_ENABLED_VARIABLE, GENERATED_ROOT_ID_VARIABLE, GENERATED_ROOT_VARIABLE,
    compose_root_setup, load_generated_root_location, platform_name,
};
use crate::job_links::LINK_FETCH_ENABLED_VARIABLE;
use crate::mcp_server::{McpSupervisor, Spawner};

/// The file of the generated Arbeitnow keys, inside the per-user data directory.
pub const KEYS_FILE: &str = "feature-keys.json";

/// The temporary file of the atomic keys write.
const KEYS_TEMP_FILE: &str = "feature-keys.json.tmp";

/// The largest keys file that is read. The real file is about 150 bytes.
const MAX_KEYS_BYTES: u64 = 4 * 1024;

/// Random bytes behind each generated key.
const KEY_BYTES: usize = 32;

/// How often a generated key pair is redrawn when the validation refuses it (a draw with too few
/// distinct bytes, which is practically impossible).
const GENERATION_ATTEMPTS: usize = 8;

/// An optional feature of the desktop app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// Arbeitnow board scan and import.
    Arbeitnow,
    /// Job posting capture from a link.
    LinkFetch,
    /// Hard deletion of profiles and jobs.
    Deletion,
    /// Document artifacts and their export.
    DocumentArtifacts,
    /// Resume and cover-letter PDF generation.
    Generation,
    /// MCP clients may scan the Arbeitnow board and import a candidate (`scan_arbeitnow_jobs`,
    /// `import_arbeitnow_job`). The scan reaches the network. Off until the user opts in.
    McpArbeitnow,
    /// MCP clients may add a job from a link (`add_job_from_link`, which fetches the page). Off
    /// until the user opts in.
    McpLinkFetch,
    /// MCP clients may delete profiles and jobs (`delete_profile`, `delete_job`). Off until the
    /// user opts in.
    McpDeletion,
}

/// The number of optional features.
const FEATURE_COUNT: usize = 8;

impl Feature {
    /// Every feature, in display order.
    pub const ALL: [Self; FEATURE_COUNT] = [
        Self::Arbeitnow,
        Self::LinkFetch,
        Self::Deletion,
        Self::DocumentArtifacts,
        Self::Generation,
        Self::McpArbeitnow,
        Self::McpLinkFetch,
        Self::McpDeletion,
    ];

    /// The stable identifier shared with the settings file and the screen.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Arbeitnow => "arbeitnow",
            Self::LinkFetch => "linkFetch",
            Self::Deletion => "deletion",
            Self::DocumentArtifacts => "documentArtifacts",
            Self::Generation => "generation",
            Self::McpArbeitnow => "mcpArbeitnow",
            Self::McpLinkFetch => "mcpLinkFetch",
            Self::McpDeletion => "mcpDeletion",
        }
    }

    /// Whether the feature is a capability of the MCP server. Its switch never sets a flag in the
    /// environment of the app itself, only in the environment of the supervised child.
    #[must_use]
    pub const fn is_mcp_capability(self) -> bool {
        matches!(
            self,
            Self::McpArbeitnow | Self::McpLinkFetch | Self::McpDeletion
        )
    }

    /// The feature of an identifier.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|feature| feature.id() == id)
    }

    /// The flag variable that gates the feature. An MCP capability shares the variable of the
    /// in-app feature it exposes, because the child reads the same loaders.
    #[must_use]
    pub const fn variable(self) -> &'static str {
        match self {
            Self::Arbeitnow | Self::McpArbeitnow => FLAG_KEY,
            Self::LinkFetch | Self::McpLinkFetch => LINK_FETCH_ENABLED_VARIABLE,
            Self::Deletion | Self::McpDeletion => DELETION_ENABLED_VARIABLE,
            Self::DocumentArtifacts => DOCUMENT_ARTIFACTS_VARIABLE,
            Self::Generation => GENERATED_ENABLED_VARIABLE,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Arbeitnow => 0,
            Self::LinkFetch => 1,
            Self::Deletion => 2,
            Self::DocumentArtifacts => 3,
            Self::Generation => 4,
            Self::McpArbeitnow => 5,
            Self::McpLinkFetch => 6,
            Self::McpDeletion => 7,
        }
    }
}

impl FeatureSettings {
    /// Whether the Settings switch of `feature` is on.
    #[must_use]
    pub const fn is_on(&self, feature: Feature) -> bool {
        match feature {
            Feature::Arbeitnow => self.arbeitnow,
            Feature::LinkFetch => self.link_fetch,
            Feature::Deletion => self.deletion,
            Feature::DocumentArtifacts => self.document_artifacts,
            Feature::Generation => self.generation,
            Feature::McpArbeitnow => self.mcp_arbeitnow,
            Feature::McpLinkFetch => self.mcp_link_fetch,
            Feature::McpDeletion => self.mcp_deletion,
        }
    }

    /// Sets the Settings switch of `feature`.
    pub const fn set(&mut self, feature: Feature, on: bool) {
        match feature {
            Feature::Arbeitnow => self.arbeitnow = on,
            Feature::LinkFetch => self.link_fetch = on,
            Feature::Deletion => self.deletion = on,
            Feature::DocumentArtifacts => self.document_artifacts = on,
            Feature::Generation => self.generation = on,
            Feature::McpArbeitnow => self.mcp_arbeitnow = on,
            Feature::McpLinkFetch => self.mcp_link_fetch = on,
            Feature::McpDeletion => self.mcp_deletion = on,
        }
    }
}

/// Whether the process environment forces `feature`, so that its Settings switch has no effect.
///
/// For an in-app feature this is a flag the environment sets (any value, even an empty or
/// non-Unicode one): `1` turns it on and anything else off. For an MCP capability only a flag set
/// to something other than exactly `1` forces it (off). A flag of `1` is a general-purpose choice
/// to enable the feature in the app or the CLI, not an opt-in to hand the capability to an MCP
/// client, so it forces nothing there and the switch stays the user's.
#[must_use]
pub fn forced_by_environment(process: &Environment, feature: Feature) -> bool {
    if feature.is_mcp_capability() {
        process.contains(feature.variable()) && !process.flag_enabled(feature.variable())
    } else {
        process.contains(feature.variable())
    }
}

/// What the running app made of each feature: whether the environment forced it, and whether it is
/// actually composed. Taken once at startup; a Settings change applies only after a restart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeatureStates {
    forced: [bool; FEATURE_COUNT],
    effective: [bool; FEATURE_COUNT],
    started_on: [bool; FEATURE_COUNT],
    /// An MCP capability that was granted at startup and has been revoked since. Turning it on
    /// again waits for the next start of the app: grants never come back in the same run.
    revoked: [bool; FEATURE_COUNT],
}

impl FeatureStates {
    /// The states with every feature unforced and not composed, and every Settings switch at its
    /// default at startup (the in-app features on, the MCP capabilities off: a first run).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            forced: [false; FEATURE_COUNT],
            effective: [false; FEATURE_COUNT],
            // The order of `Feature::ALL`; a test pins it to `FeatureSettings::default()`.
            started_on: [true, true, true, true, true, false, false, false],
            revoked: [false; FEATURE_COUNT],
        }
    }

    /// Records whether the MCP capability `feature` was revoked after it was granted at startup.
    #[must_use]
    pub const fn with_revoked(mut self, feature: Feature, revoked: bool) -> Self {
        self.revoked[feature.index()] = revoked;
        self
    }

    /// Whether the MCP capability `feature` was granted at startup and revoked since.
    #[must_use]
    pub const fn revoked(&self, feature: Feature) -> bool {
        self.revoked[feature.index()]
    }

    /// Records whether the environment forces `feature`.
    #[must_use]
    pub const fn with_forced(mut self, feature: Feature, forced: bool) -> Self {
        self.forced[feature.index()] = forced;
        self
    }

    /// Records whether `feature` is composed and usable.
    #[must_use]
    pub const fn with_effective(mut self, feature: Feature, effective: bool) -> Self {
        self.effective[feature.index()] = effective;
        self
    }

    /// Records whether the Settings switch of `feature` was on when the app started.
    #[must_use]
    pub const fn with_started_on(mut self, feature: Feature, on: bool) -> Self {
        self.started_on[feature.index()] = on;
        self
    }

    /// Whether the environment forces `feature`.
    #[must_use]
    pub const fn forced(&self, feature: Feature) -> bool {
        self.forced[feature.index()]
    }

    /// Whether `feature` is composed and usable in this run.
    #[must_use]
    pub const fn effective(&self, feature: Feature) -> bool {
        self.effective[feature.index()]
    }

    /// Whether the Settings switch of `feature` was on when the app started.
    #[must_use]
    pub const fn started_on(&self, feature: Feature) -> bool {
        self.started_on[feature.index()]
    }
}

/// A feature that was left off at startup. The texts are fixed: they name no path, key or value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureWarning {
    /// The generated Arbeitnow keys could not be read or created.
    KeysUnavailable,
    /// The keys of the environment and the file do not form a valid Arbeitnow configuration.
    KeysInvalid,
    /// The default generated root could not be registered.
    RootNotRegistered,
}

impl FeatureWarning {
    /// The fixed warning text.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::KeysUnavailable => {
                "Arbeitnow stays off: its signing keys could not be read or created."
            }
            Self::KeysInvalid => "Arbeitnow stays off: its signing keys are not valid.",
            Self::RootNotRegistered => {
                "Document generation stays off: its output folder could not be set up."
            }
        }
    }
}

/// Writes one fixed warning line per left-off feature to the diagnostics.
pub fn report_warnings(warnings: &[FeatureWarning]) {
    for warning in warnings {
        tracing::warn!("{}", warning.text());
    }
}

/// The generated Arbeitnow keys. The values never leave this module's callers' environment.
#[derive(Clone, PartialEq, Eq)]
pub struct FeatureKeys {
    candidate_key: String,
    cursor_secret: String,
}

impl std::fmt::Debug for FeatureKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FeatureKeys(<redacted>)")
    }
}

/// The keys file as stored.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredKeys {
    arbeitnow_candidate_key: String,
    cursor_hmac_secret: String,
}

impl FeatureKeys {
    /// Whether the two keys form a configuration `load_arbeitnow_config` accepts.
    fn is_valid(&self) -> bool {
        let env = Environment::from_pairs([
            (FLAG_KEY, "1"),
            (CANDIDATE_KEY, self.candidate_key.as_str()),
            (CURSOR_SECRET_KEY, self.cursor_secret.as_str()),
        ]);
        matches!(load_arbeitnow_config(&env), Ok(Some(_)))
    }

    /// A fresh valid pair from the OS random source; `None` when it does not answer.
    fn generate() -> Option<Self> {
        for _ in 0..GENERATION_ATTEMPTS {
            let keys = Self {
                candidate_key: random_text()?,
                cursor_secret: random_text()?,
            };
            if keys.is_valid() {
                return Some(keys);
            }
        }
        None
    }
}

/// 32 random bytes as unpadded base64url: a valid candidate key and a valid cursor secret (43 ASCII
/// characters).
fn random_text() -> Option<String> {
    let mut bytes = [0_u8; KEY_BYTES];
    getrandom::fill(&mut bytes).ok()?;
    Some(encode_base64url(&bytes))
}

/// Unpadded base64url (RFC 4648 section 5).
fn encode_base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block = [0_u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let value = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        // One output character per started 6 bits: 2, 3 or 4 for a 1, 2 or 3 byte chunk.
        for position in 0..=chunk.len() {
            let sextet = (value >> (18 - 6 * position)) & 0x3f;
            text.push(char::from(ALPHABET[sextet as usize]));
        }
    }
    text
}

/// Reads the keys from `directory`, or generates and stores a new pair. A missing, unreadable,
/// oversized, malformed or invalid file is replaced; `None` only when no valid pair can be read
/// back or written (a pair that cannot be stored would change on every start, so it is not used).
///
/// The file is written atomically (a temporary file, then a rename) and, on Unix, readable by the
/// user only. Nothing is logged and no path or value is returned.
#[must_use]
pub fn load_or_create_keys(directory: &Path) -> Option<FeatureKeys> {
    if let Some(keys) = read_keys(&directory.join(KEYS_FILE)) {
        return Some(keys);
    }
    let keys = FeatureKeys::generate()?;
    write_keys(directory, &keys).ok()?;
    Some(keys)
}

fn read_keys(path: &Path) -> Option<FeatureKeys> {
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_KEYS_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() as u64 > MAX_KEYS_BYTES {
        return None;
    }
    let stored: StoredKeys = serde_json::from_str(&text).ok()?;
    let keys = FeatureKeys {
        candidate_key: stored.arbeitnow_candidate_key,
        cursor_secret: stored.cursor_hmac_secret,
    };
    keys.is_valid().then_some(keys)
}

fn write_keys(directory: &Path, keys: &FeatureKeys) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(&StoredKeys {
        arbeitnow_candidate_key: keys.candidate_key.clone(),
        cursor_hmac_secret: keys.cursor_secret.clone(),
    })
    .map_err(std::io::Error::other)?;
    fs::create_dir_all(directory)?;
    let temporary = directory.join(KEYS_TEMP_FILE);
    // A leftover file may carry looser permissions than the one created below.
    drop(fs::remove_file(&temporary));
    let written = write_private(&temporary, text.as_bytes())
        .and_then(|()| fs::rename(&temporary, directory.join(KEYS_FILE)));
    if written.is_err() {
        drop(fs::remove_file(&temporary));
    }
    written
}

/// Writes `bytes` to a new file that only the user can read (mode 0600 on Unix; on Windows the
/// per-user data directory is already private to the user's account) and flushes it to disk.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The result of [`overlay`]: the effective variable pairs and the features left off.
#[derive(Clone, PartialEq, Eq)]
pub struct Overlay {
    /// The process pairs followed by the defaults that were added.
    pub pairs: Vec<(OsString, OsString)>,
    /// The features that were left off.
    pub warnings: Vec<FeatureWarning>,
}

impl fmt::Debug for Overlay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The pairs hold the process environment and the generated keys: a count only.
        f.debug_struct("Overlay")
            .field("variables", &self.pairs.len())
            .field("warnings", &self.warnings)
            .finish()
    }
}

/// Overlays the defaults on the process environment. Pure: `keys` is called at most once, only when
/// Arbeitnow is wanted and the environment lacks one of its keys.
///
/// A flag the environment sets is never touched. An unset flag becomes `1` when its Settings switch
/// is on, except Arbeitnow, which also needs a valid key pair: the environment's keys, completed
/// from `keys`. Without a valid pair the flag is left unset (Arbeitnow off) and a warning says so,
/// where an environment-set flag with unusable keys keeps failing startup as it always did.
///
/// The MCP capability switches add no flag here: this is the environment of the app, and the MCP
/// flags belong to the child only ([`DesktopEnvironment::supervisor_base_pairs`]). The Arbeitnow keys
/// are prepared when either the in-app feature or the MCP opt-in wants them, so that the child can
/// use them without the in-app feature being on.
pub fn overlay(
    process: Vec<(OsString, OsString)>,
    features: &FeatureSettings,
    keys: impl FnOnce() -> Option<FeatureKeys>,
) -> Overlay {
    let snapshot = Environment::from_os_pairs(process.iter().cloned(), cfg!(windows));
    let mut pairs = process;
    let mut warnings = Vec::new();
    for feature in Feature::ALL {
        if feature != Feature::Arbeitnow
            && !feature.is_mcp_capability()
            && !forced_by_environment(&snapshot, feature)
            && features.is_on(feature)
        {
            pairs.push((feature.variable().into(), "1".into()));
        }
    }
    let forced = forced_by_environment(&snapshot, Feature::Arbeitnow);
    let in_app = if forced {
        snapshot.flag_enabled(FLAG_KEY)
    } else {
        features.arbeitnow
    };
    let for_mcp =
        features.mcp_arbeitnow && !forced_by_environment(&snapshot, Feature::McpArbeitnow);
    if in_app || for_mcp {
        let usable = complete_arbeitnow_keys(&snapshot, &mut pairs, &mut warnings, keys);
        // A flag of the environment stays as it is; the default flag needs a usable key pair, and
        // only the in-app switch sets it.
        if !forced && in_app && usable {
            pairs.push((FLAG_KEY.into(), "1".into()));
        }
    }
    Overlay { pairs, warnings }
}

/// Adds the keys the environment lacks from `keys`. With the default flag (`forced` false) the
/// resulting pair is checked, so a stale key of the environment leaves Arbeitnow off instead of
/// stopping startup. Returns whether the pair is usable (or, for an environment flag, was left to
/// the loader).
fn complete_arbeitnow_keys(
    snapshot: &Environment,
    pairs: &mut Vec<(OsString, OsString)>,
    warnings: &mut Vec<FeatureWarning>,
    keys: impl FnOnce() -> Option<FeatureKeys>,
) -> bool {
    let forced = forced_by_environment(snapshot, Feature::Arbeitnow);
    let has_candidate = snapshot.contains(CANDIDATE_KEY);
    let has_secret = snapshot.contains(CURSOR_SECRET_KEY);
    if !has_candidate || !has_secret {
        let Some(keys) = keys() else {
            warnings.push(FeatureWarning::KeysUnavailable);
            return false;
        };
        if !has_candidate {
            pairs.push((CANDIDATE_KEY.into(), keys.candidate_key.into()));
        }
        if !has_secret {
            pairs.push((CURSOR_SECRET_KEY.into(), keys.cursor_secret.into()));
        }
    }
    if forced {
        return true;
    }
    let candidate = Environment::from_os_pairs(
        pairs
            .iter()
            .cloned()
            .chain([(OsString::from(FLAG_KEY), OsString::from("1"))]),
        cfg!(windows),
    );
    let valid = matches!(load_arbeitnow_config(&candidate), Ok(Some(_)));
    if !valid {
        warnings.push(FeatureWarning::KeysInvalid);
    }
    valid
}

/// Whether `pairs` plus the Arbeitnow flag form a configuration `load_arbeitnow_config` accepts.
/// The child fails to start on a flag with unusable keys, so it only gets the flag with a valid pair.
fn arbeitnow_keys_are_valid(pairs: &[(OsString, OsString)]) -> bool {
    let candidate = Environment::from_os_pairs(
        pairs
            .iter()
            .filter(|(name, _)| {
                !name
                    .to_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(FLAG_KEY))
            })
            .cloned()
            .chain([(OsString::from(FLAG_KEY), OsString::from("1"))]),
        cfg!(windows),
    );
    matches!(load_arbeitnow_config(&candidate), Ok(Some(_)))
}

/// What the root registration did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootRegistration {
    /// Nothing to do: generation is off, or the root is managed by the environment.
    NotNeeded,
    /// The per-user default root is registered (now or before).
    Registered,
    /// The root could not be registered.
    Failed,
}

/// Registers the per-user default generated root when generation is on in `env` and the
/// environment names neither a root nor its identity. This is `setup-generated-root`: the schema is
/// brought up to date first (`prepare` migrates, as every start does), then the root is created,
/// marked and registered; a root that is already set up is confirmed unchanged, so the call is
/// idempotent. No path or cause is reported.
pub async fn register_default_generated_root(env: &Environment) -> RootRegistration {
    if !env.flag_enabled(GENERATED_ENABLED_VARIABLE)
        || env.contains(GENERATED_ROOT_ID_VARIABLE)
        || env.contains(GENERATED_ROOT_VARIABLE)
    {
        return RootRegistration::NotNeeded;
    }
    let platform = RootPlatform::from_platform_name(platform_name());
    let (Some(location), Some(path)) = (
        load_generated_root_location(env, platform),
        database_path(env),
    ) else {
        return RootRegistration::Failed;
    };
    let Some(pool) = prepare(&path) else {
        return RootRegistration::Failed;
    };
    match compose_root_setup(&pool, &location, None).run().await {
        Ok(_) => RootRegistration::Registered,
        Err(_) => RootRegistration::Failed,
    }
}

/// The environment the desktop app composes from. Its `Debug` text shows counts only: the pairs
/// hold the process environment and the generated Arbeitnow keys.
#[derive(Clone)]
pub struct DesktopEnvironment {
    /// The effective variable pairs: the process pairs first, then what the defaults added.
    pub pairs: Vec<(OsString, OsString)>,
    /// How many of the leading `pairs` come from the process environment.
    pub process_count: usize,
    /// The Settings switches the environment was built from.
    pub features: FeatureSettings,
    /// The features that were left off, with a fixed warning each.
    pub warnings: Vec<FeatureWarning>,
}

impl fmt::Debug for DesktopEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DesktopEnvironment")
            .field("variables", &self.pairs.len())
            .field("process_count", &self.process_count)
            .field("features", &self.features)
            .field("warnings", &self.warnings)
            .finish()
    }
}

impl DesktopEnvironment {
    /// The snapshot every compose call takes.
    #[must_use]
    pub fn environment(&self) -> Environment {
        Environment::from_os_pairs(self.pairs.iter().cloned(), cfg!(windows))
    }

    /// The pairs the supervised `jobniac-cli http` child is built from, before any grant: the
    /// process environment and what the defaults added, without the three MCP capabilities.
    ///
    /// The flags of the capabilities (`JOBNIAC_ARBEITNOW_ENABLED`, `JOBNIAC_LINK_FETCH_ENABLED`,
    /// `JOBNIAC_DELETION_ENABLED`) are never inherited. Neither the app's own flags (on by default
    /// for its screens) nor a flag of exactly `1` in the process environment reaches the child: such
    /// a flag says the app or the CLI may use the feature, not that an MCP client may. A flag the
    /// process sets to anything else (`0`, `true`, an empty value, a value that is not Unicode) is
    /// passed on as the explicit refusal it is. The names match without regard to case, as the
    /// Windows environment does. The capabilities come back only as grants ([`McpGrants`]).
    ///
    /// The Arbeitnow candidate key never crosses here, and neither does a cursor secret the app
    /// generated: they cross only with the Arbeitnow grant. A cursor secret the process sets is kept
    /// (it is the documented setting for stable list cursors).
    ///
    /// The child does not inherit `HOME` or `LOCALAPPDATA`, so it could not find the per-user
    /// default generated root: with generation on and no root set, the root the app registered is
    /// named explicitly.
    #[must_use]
    pub fn supervisor_base_pairs(&self) -> Vec<(OsString, OsString)> {
        let env = self.environment();
        let named = |name: &OsString, variable: &str| {
            name.to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(variable))
        };
        let gated = |name: &OsString| {
            Feature::ALL
                .into_iter()
                .filter(|feature| feature.is_mcp_capability())
                .any(|feature| named(name, feature.variable()))
        };
        let mut pairs: Vec<(OsString, OsString)> = self
            .pairs
            .iter()
            .enumerate()
            .filter(|(index, (name, value))| {
                let process = *index < self.process_count;
                if named(name, CANDIDATE_KEY) {
                    return false;
                }
                if named(name, CURSOR_SECRET_KEY) {
                    return process;
                }
                // A gated flag crosses only as a refusal of the process environment.
                !gated(name) || (process && value.to_str() != Some("1"))
            })
            .map(|(_, pair)| pair.clone())
            .collect();
        if env.flag_enabled(GENERATED_ENABLED_VARIABLE)
            && !env.contains(GENERATED_ROOT_VARIABLE)
            && let Some(location) = load_generated_root_location(
                &env,
                RootPlatform::from_platform_name(platform_name()),
            )
        {
            pairs.push((GENERATED_ROOT_VARIABLE.into(), location.root.into()));
        }
        pairs
    }

    /// What the user has granted the MCP server at startup, from the Settings switches this
    /// environment was built from: the capability flags that are on, not refused by the process
    /// environment, and (for Arbeitnow) backed by a valid key pair, each with the pairs it needs.
    /// The supervisor holds these and can only narrow them ([`McpGrants::pairs_for`]).
    #[must_use]
    pub fn mcp_grants(&self) -> McpGrants {
        let env = self.environment();
        let process = Environment::from_os_pairs(
            self.pairs.iter().take(self.process_count).cloned(),
            cfg!(windows),
        );
        let mut entries = Vec::new();
        for feature in Feature::ALL {
            if !feature.is_mcp_capability()
                || forced_by_environment(&process, feature)
                || !self.features.is_on(feature)
            {
                continue;
            }
            let mut grant = vec![(OsString::from(feature.variable()), OsString::from("1"))];
            if feature == Feature::McpArbeitnow {
                if !arbeitnow_keys_are_valid(&self.pairs) {
                    continue;
                }
                let (Some(candidate), Some(secret)) =
                    (env.get(CANDIDATE_KEY), env.get(CURSOR_SECRET_KEY))
                else {
                    continue;
                };
                grant.push((CANDIDATE_KEY.into(), candidate.into()));
                // A secret of the process is already in the base pairs.
                if !process.contains(CURSOR_SECRET_KEY) {
                    grant.push((CURSOR_SECRET_KEY.into(), secret.into()));
                }
            }
            entries.push((feature, grant));
        }
        McpGrants { entries }
    }

    /// The supervisor of the MCP server as the desktop shell builds it, with the grants it starts
    /// with (also returned, as the most it can ever hold in this run). This is the one way to wire
    /// the environment into a supervisor: the base pairs go to [`McpSupervisor::new`] and the
    /// capabilities go in as grants, which a later revocation can take away. Passing the base and
    /// the grants together as one source environment would make them impossible to revoke.
    #[must_use]
    pub fn supervisor(&self, spawner: Arc<dyn Spawner>) -> (McpSupervisor, McpGrants) {
        let grants = self.mcp_grants();
        let supervisor =
            McpSupervisor::new(spawner, self.supervisor_base_pairs()).with_grants(grants.pairs());
        (supervisor, grants)
    }
}

/// The capabilities of the MCP server the user granted at startup, each with the variable pairs the
/// child needs for it. This is the most the supervisor can ever hold in this run: a revocation
/// removes a capability, and nothing adds one back before the next start of the app.
///
/// The pairs include the Arbeitnow signing keys, so `Debug` shows which capabilities were granted
/// and never a variable value.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct McpGrants {
    entries: Vec<(Feature, Vec<(OsString, OsString)>)>,
}

impl fmt::Debug for McpGrants {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let granted: Vec<&str> = self
            .entries
            .iter()
            .map(|(feature, _)| feature.id())
            .collect();
        f.debug_struct("McpGrants")
            .field("granted", &granted)
            .finish_non_exhaustive()
    }
}

impl McpGrants {
    /// Every pair of every grant, for the supervisor's startup grants.
    #[must_use]
    pub fn pairs(&self) -> Vec<(OsString, OsString)> {
        self.entries
            .iter()
            .flat_map(|(_, pairs)| pairs.iter().cloned())
            .collect()
    }

    /// The pairs of the grants whose Settings switch is still on in `features`. Passed to
    /// `McpSupervisor::restrict_grants` after a switch was turned off.
    #[must_use]
    pub fn pairs_for(&self, features: &FeatureSettings) -> Vec<(OsString, OsString)> {
        self.entries
            .iter()
            .filter(|(feature, _)| features.is_on(*feature))
            .flat_map(|(_, pairs)| pairs.iter().cloned())
            .collect()
    }

    /// Whether `pair` belongs to a grant whose Settings switch is still on in `features`. For
    /// `McpSupervisor::retain_grants`: the decision copies no value.
    #[must_use]
    pub fn allows(&self, features: &FeatureSettings, pair: &(OsString, OsString)) -> bool {
        self.entries
            .iter()
            .any(|(feature, pairs)| features.is_on(*feature) && pairs.contains(pair))
    }

    /// Whether `feature` was granted at startup.
    #[must_use]
    pub fn granted_at_startup(&self, feature: Feature) -> bool {
        self.entries.iter().any(|(granted, _)| *granted == feature)
    }
}

/// Builds the effective environment of the desktop app: the defaults overlaid on `process`, the
/// Arbeitnow keys from `keys_directory` where the environment has none, and the default generated
/// root registered when generation is on. A feature that cannot be made ready stays off with a
/// warning. Never fails.
pub async fn prepare_environment(
    process: Vec<(OsString, OsString)>,
    features: &FeatureSettings,
    keys_directory: Option<&Path>,
) -> DesktopEnvironment {
    let process_count = process.len();
    let mut result = overlay(process, features, || {
        keys_directory.and_then(load_or_create_keys)
    });
    let env = Environment::from_os_pairs(result.pairs.iter().cloned(), cfg!(windows));
    if register_default_generated_root(&env).await == RootRegistration::Failed {
        result.warnings.push(FeatureWarning::RootNotRegistered);
        // The default flag is withdrawn. A flag of the environment stays: composition then finds
        // no marker and refuses generation on its own.
        let added = result.pairs.split_off(process_count);
        let generation = Feature::Generation.variable();
        result.pairs.extend(added.into_iter().filter(|(name, _)| {
            !name
                .to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(generation))
        }));
    }
    DesktopEnvironment {
        pairs: result.pairs,
        process_count,
        features: *features,
        warnings: result.warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const CANDIDATE: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
    const SECRET: &str = "cursor-secret-0123456789abcdefghijklmnop";

    fn os(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    fn fixed_keys() -> FeatureKeys {
        FeatureKeys {
            candidate_key: CANDIDATE.to_owned(),
            cursor_secret: SECRET.to_owned(),
        }
    }

    fn effective(overlay: &Overlay) -> Environment {
        Environment::from_os_pairs(overlay.pairs.iter().cloned(), false)
    }

    fn on(overlay: &Overlay, feature: Feature) -> bool {
        effective(overlay).flag_enabled(feature.variable())
    }

    fn run(process: Vec<(OsString, OsString)>, features: &FeatureSettings) -> Overlay {
        overlay(process, features, || Some(fixed_keys()))
    }

    /// The features with a flag of their own in the environment of the app.
    fn in_app_features() -> impl Iterator<Item = Feature> {
        Feature::ALL
            .into_iter()
            .filter(|feature| !feature.is_mcp_capability())
    }

    #[test]
    fn an_empty_environment_turns_every_in_app_feature_on() {
        let result = run(Vec::new(), &FeatureSettings::default());
        for feature in in_app_features() {
            assert!(on(&result, feature), "{feature:?}");
        }
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn the_mcp_capabilities_are_off_by_default_and_are_not_features_of_the_app_environment() {
        let defaults = FeatureSettings::default();
        for feature in Feature::ALL {
            assert_eq!(
                defaults.is_on(feature),
                !feature.is_mcp_capability(),
                "{feature:?}"
            );
        }
        // Even switched on, they add no flag to the environment of the app: the in-app switches of
        // Arbeitnow, link capture and deletion alone decide its flags.
        let mut features = FeatureSettings::default();
        for feature in Feature::ALL {
            features.set(feature, feature.is_mcp_capability());
        }
        let result = run(Vec::new(), &features);
        let env = effective(&result);
        assert!(!env.contains(DELETION_ENABLED_VARIABLE));
        assert!(!env.contains(LINK_FETCH_ENABLED_VARIABLE));
        assert!(!env.contains(FLAG_KEY));
    }

    #[test]
    fn the_start_states_match_the_default_switches() {
        let states = FeatureStates::new();
        let defaults = FeatureSettings::default();
        for feature in Feature::ALL {
            assert_eq!(
                states.started_on(feature),
                defaults.is_on(feature),
                "{feature:?}"
            );
        }
        // A first run has nothing to restart for, whatever the default of a switch is.
        assert!(!states.started_on(Feature::McpDeletion));
        assert!(!states.started_on(Feature::McpLinkFetch));
    }

    #[test]
    fn a_set_flag_wins_whatever_its_value() {
        for feature in Feature::ALL {
            for value in ["0", "true", "", "01", " 1"] {
                let result = run(
                    os(&[(feature.variable(), value)]),
                    &FeatureSettings::default(),
                );
                assert!(!on(&result, feature), "{feature:?} {value:?}");
                assert!(forced_by_environment(&effective(&result), feature));
            }
        }
    }

    #[test]
    fn a_flag_of_one_is_on_even_with_the_switch_off() {
        let mut features = FeatureSettings::default();
        for feature in Feature::ALL {
            features.set(feature, false);
        }
        let process: Vec<_> = in_app_features()
            .map(|feature| (feature.variable(), "1"))
            .collect();
        let result = run(os(&process), &features);
        for feature in Feature::ALL {
            assert!(on(&result, feature), "{feature:?}");
        }
    }

    #[test]
    fn a_switch_that_is_off_leaves_its_flag_unset() {
        for off in in_app_features() {
            let mut features = FeatureSettings::default();
            features.set(off, false);
            let result = run(Vec::new(), &features);
            for feature in in_app_features() {
                assert_eq!(
                    on(&result, feature),
                    feature != off,
                    "{feature:?} off={off:?}"
                );
            }
            assert!(!effective(&result).contains(off.variable()));
        }
    }

    #[test]
    fn the_keys_are_only_asked_for_when_arbeitnow_needs_them() {
        let features = FeatureSettings {
            arbeitnow: false,
            ..FeatureSettings::default()
        };
        let result = overlay(Vec::new(), &features, || {
            panic!("the keys must not be read while Arbeitnow is off")
        });
        assert!(!effective(&result).contains(CANDIDATE_KEY));
        let process = os(&[(CANDIDATE_KEY, CANDIDATE), (CURSOR_SECRET_KEY, SECRET)]);
        let result = overlay(process, &FeatureSettings::default(), || {
            panic!("the keys of the environment are used as they are")
        });
        assert!(on(&result, Feature::Arbeitnow));
    }

    #[test]
    fn the_environment_keys_win_and_the_missing_one_comes_from_the_file() {
        let from_environment = "environment-secret-0123456789abcdefghij";
        let result = run(
            os(&[(CURSOR_SECRET_KEY, from_environment)]),
            &FeatureSettings::default(),
        );
        let env = effective(&result);
        assert_eq!(env.get(CURSOR_SECRET_KEY), Some(from_environment));
        assert_eq!(env.get(CANDIDATE_KEY), Some(CANDIDATE));
        assert!(on(&result, Feature::Arbeitnow));
    }

    #[test]
    fn missing_keys_leave_arbeitnow_off_with_a_fixed_warning() {
        let result = overlay(Vec::new(), &FeatureSettings::default(), || None);
        assert!(!on(&result, Feature::Arbeitnow));
        assert_eq!(result.warnings, vec![FeatureWarning::KeysUnavailable]);
        assert!(on(&result, Feature::Deletion));
    }

    #[test]
    fn a_stale_environment_key_leaves_arbeitnow_off_instead_of_stopping_startup() {
        // A blank cursor secret is fine for the list cursors, but not for Arbeitnow.
        let result = run(
            os(&[(CURSOR_SECRET_KEY, "   ")]),
            &FeatureSettings::default(),
        );
        assert!(!on(&result, Feature::Arbeitnow));
        assert_eq!(result.warnings, vec![FeatureWarning::KeysInvalid]);
    }

    #[test]
    fn a_flag_of_one_with_unusable_keys_is_left_to_the_loader() {
        let result = run(
            os(&[(FLAG_KEY, "1"), (CURSOR_SECRET_KEY, "short")]),
            &FeatureSettings::default(),
        );
        assert!(load_arbeitnow_config(&effective(&result)).is_err());
    }

    #[test]
    fn the_overlay_never_changes_the_process_pairs() {
        let process = os(&[("PATH", "/usr/bin"), ("JOBNIAC_DB_PATH", "/x")]);
        let result = overlay(process.clone(), &FeatureSettings::default(), || None);
        assert_eq!(&result.pairs[..process.len()], process.as_slice());
    }

    #[test]
    fn the_feature_identifiers_round_trip() {
        for feature in Feature::ALL {
            assert_eq!(Feature::from_id(feature.id()), Some(feature));
        }
        assert_eq!(Feature::from_id("mcp"), None);
    }

    #[test]
    fn base64url_matches_the_known_vector() {
        let bytes: Vec<u8> = (0..32).collect();
        assert_eq!(encode_base64url(&bytes), CANDIDATE);
        assert_eq!(encode_base64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(encode_base64url(&[]), "");
    }

    #[test]
    fn generated_keys_pass_the_arbeitnow_validation_and_stay_stable() -> TestResult {
        let dir = tempfile::tempdir()?;
        let first = load_or_create_keys(dir.path()).ok_or("keys must be created")?;
        let env = Environment::from_pairs([
            (FLAG_KEY, "1"),
            (CANDIDATE_KEY, first.candidate_key.as_str()),
            (CURSOR_SECRET_KEY, first.cursor_secret.as_str()),
        ]);
        assert!(load_arbeitnow_config(&env)?.is_some());
        assert_ne!(first.candidate_key, first.cursor_secret);
        let second = load_or_create_keys(dir.path()).ok_or("keys must reload")?;
        assert_eq!(first, second);
        assert!(!dir.path().join(KEYS_TEMP_FILE).exists());
        Ok(())
    }

    #[test]
    fn a_broken_keys_file_is_regenerated() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(KEYS_FILE);
        let first = load_or_create_keys(dir.path()).ok_or("keys must be created")?;
        let oversized = " ".repeat(usize::try_from(MAX_KEYS_BYTES)? + 1);
        for broken in [
            "",
            "not json",
            "[1]",
            r#"{"arbeitnowCandidateKey":"AAAA","cursorHmacSecret":"short"}"#,
            r#"{"arbeitnowCandidateKey":1}"#,
            oversized.as_str(),
        ] {
            fs::write(&path, broken)?;
            let keys = load_or_create_keys(dir.path()).ok_or("keys must be regenerated")?;
            assert!(keys.is_valid());
            assert_ne!(keys, first, "{broken:?}");
            // The repaired file is stable again.
            assert_eq!(load_or_create_keys(dir.path()), Some(keys));
        }
        Ok(())
    }

    #[test]
    fn keys_that_cannot_be_stored_are_not_used() -> TestResult {
        let dir = tempfile::tempdir()?;
        let blocker = dir.path().join("blocked");
        fs::write(&blocker, "x")?;
        assert!(load_or_create_keys(&blocker.join("inner")).is_none());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn the_keys_file_is_private_to_the_user() -> TestResult {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        load_or_create_keys(dir.path()).ok_or("keys must be created")?;
        let mode = fs::metadata(dir.path().join(KEYS_FILE))?
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        Ok(())
    }

    #[test]
    fn the_debug_form_and_the_warnings_show_no_key() {
        let keys = fixed_keys();
        let text = format!("{keys:?}");
        assert!(!text.contains(CANDIDATE) && !text.contains(SECRET));
    }

    type Pairs = Vec<(OsString, OsString)>;

    /// A home for a test that registers the default root. The generated root is refused when its
    /// text differs from its final path, and on Windows `%TEMP%` can be an 8.3 alias that the final
    /// path expands, so the home is built from the canonical temporary folder.
    fn home_dir() -> std::io::Result<tempfile::TempDir> {
        jobniac_testkit::scratch::scratch_dir("jobniac-home-")
    }

    /// An environment with a database and a user directory for every platform's default root.
    fn home_environment(root: &Path) -> Result<Pairs, Box<dyn std::error::Error>> {
        let base = root.to_str().ok_or("temporary path is not Unicode")?;
        let database = root.join("jobniac.sqlite3");
        let database = database.to_str().ok_or("temporary path is not Unicode")?;
        Ok(os(&[
            ("JOBNIAC_DB_PATH", database),
            ("LOCALAPPDATA", base),
            ("HOME", base),
            ("XDG_DATA_HOME", base),
        ]))
    }

    fn platform() -> RootPlatform {
        RootPlatform::from_platform_name(platform_name())
    }

    #[tokio::test]
    async fn generation_registers_the_default_root_so_that_composition_finds_it() -> TestResult {
        let dir = home_dir()?;
        let keys = dir.path().join("keys");
        let prepared = prepare_environment(
            home_environment(dir.path())?,
            &FeatureSettings::default(),
            Some(&keys),
        )
        .await;
        assert!(prepared.warnings.is_empty(), "{:?}", prepared.warnings);
        let env = prepared.environment();
        assert!(env.flag_enabled(GENERATED_ENABLED_VARIABLE));
        assert!(env.flag_enabled(FLAG_KEY));
        let services = crate::services::compose(&env)?;
        assert!(services.generation.facade.is_available().await);
        // Again: idempotent, and the root stays registered.
        let again = prepare_environment(
            home_environment(dir.path())?,
            &FeatureSettings::default(),
            Some(&keys),
        )
        .await;
        assert!(again.warnings.is_empty());
        let services = crate::services::compose(&again.environment())?;
        assert!(services.generation.facade.is_available().await);
        Ok(())
    }

    /// A home reached through a link (macOS `/var` temporary directories, a relocated home,
    /// `/home` pointing at `/var/home`): opening the root never follows a link, so the default
    /// root is registered, and found again, under the real location of the home.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_home_behind_a_link_still_registers_the_default_root() -> TestResult {
        let dir = tempfile::tempdir()?;
        let real = std::fs::canonicalize(dir.path())?.join("real-home");
        std::fs::create_dir(&real)?;
        let link = std::fs::canonicalize(dir.path())?.join("linked-home");
        std::os::unix::fs::symlink(&real, &link)?;
        let keys = dir.path().join("keys");
        let database = real.join("jobniac.sqlite3");
        let linked = link.to_str().ok_or("temporary path is not Unicode")?;
        let database = database.to_str().ok_or("temporary path is not Unicode")?;
        let pairs = os(&[
            ("JOBNIAC_DB_PATH", database),
            ("LOCALAPPDATA", linked),
            ("HOME", linked),
            ("XDG_DATA_HOME", linked),
        ]);
        let prepared = prepare_environment(pairs, &FeatureSettings::default(), Some(&keys)).await;
        assert!(prepared.warnings.is_empty(), "{:?}", prepared.warnings);
        let location = load_generated_root_location(&prepared.environment(), platform())
            .ok_or("the app must resolve the root")?;
        assert!(
            location
                .root
                .starts_with(real.to_str().ok_or("not Unicode")?)
        );
        let services = crate::services::compose(&prepared.environment())?;
        assert!(services.generation.facade.is_available().await);
        Ok(())
    }

    #[tokio::test]
    async fn generation_switched_off_registers_nothing() -> TestResult {
        let dir = tempfile::tempdir()?;
        let features = FeatureSettings {
            generation: false,
            ..FeatureSettings::default()
        };
        let prepared = prepare_environment(home_environment(dir.path())?, &features, None).await;
        let env = prepared.environment();
        assert!(!env.flag_enabled(GENERATED_ENABLED_VARIABLE));
        assert_eq!(
            register_default_generated_root(&env).await,
            RootRegistration::NotNeeded
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_environment_managed_root_is_not_registered_by_the_app() {
        for managed in [
            (GENERATED_ROOT_VARIABLE, "/srv/jobniac-synthetic/generated"),
            (
                GENERATED_ROOT_ID_VARIABLE,
                "6f1c2d3e-4a5b-4c6d-8e7f-900112233445",
            ),
        ] {
            let env = Environment::from_pairs([(GENERATED_ENABLED_VARIABLE, "1"), managed]);
            assert_eq!(
                register_default_generated_root(&env).await,
                RootRegistration::NotNeeded
            );
        }
    }

    #[tokio::test]
    async fn a_failed_registration_withdraws_the_default_flag_and_warns() -> TestResult {
        let dir = tempfile::tempdir()?;
        // No user directory in the environment: the default root cannot be resolved.
        let database = dir.path().join("jobniac.sqlite3");
        let database = database.to_str().ok_or("temporary path is not Unicode")?;
        let prepared = prepare_environment(
            os(&[("JOBNIAC_DB_PATH", database)]),
            &FeatureSettings::default(),
            None,
        )
        .await;
        let env = prepared.environment();
        assert!(!env.flag_enabled(GENERATED_ENABLED_VARIABLE));
        assert!(
            prepared
                .warnings
                .contains(&FeatureWarning::RootNotRegistered)
        );
        assert!(prepared.warnings.contains(&FeatureWarning::KeysUnavailable));
        // The other features are unaffected.
        assert!(env.flag_enabled(DELETION_ENABLED_VARIABLE));
        Ok(())
    }

    #[tokio::test]
    async fn the_supervised_child_is_told_the_registered_root() -> TestResult {
        let dir = home_dir()?;
        let prepared = prepare_environment(
            home_environment(dir.path())?,
            &FeatureSettings::default(),
            Some(&dir.path().join("keys")),
        )
        .await;
        let child = Environment::from_os_pairs(prepared.supervisor_pairs(), false);
        let location = load_generated_root_location(&child, platform())
            .ok_or("the child must resolve the root")?;
        assert!(!location.is_default);
        let own = load_generated_root_location(&prepared.environment(), platform())
            .ok_or("the app must resolve the root")?;
        assert_eq!(location.root, own.root);
        // A root of the environment is passed on unchanged.
        let managed = DesktopEnvironment {
            pairs: os(&[
                (GENERATED_ENABLED_VARIABLE, "1"),
                (GENERATED_ROOT_VARIABLE, "/srv/jobniac-synthetic/generated"),
            ]),
            process_count: 2,
            features: FeatureSettings::default(),
            warnings: Vec::new(),
        };
        assert_eq!(managed.supervisor_pairs(), managed.pairs);
        Ok(())
    }

    impl DesktopEnvironment {
        /// The whole environment of a child started with the startup grants: the base pairs and
        /// every grant. Test-only: the app wires the two apart (`DesktopEnvironment::supervisor`),
        /// so that a grant can be revoked.
        fn supervisor_pairs(&self) -> Vec<(OsString, OsString)> {
            let mut pairs = self.supervisor_base_pairs();
            pairs.extend(self.mcp_grants().pairs());
            pairs
        }
    }

    /// The desktop environment built from `process` and `features`, as `prepare_environment`
    /// builds it (without a database or a root).
    fn desktop(process: Pairs, features: &FeatureSettings) -> DesktopEnvironment {
        let process_count = process.len();
        let result = run(process, features);
        DesktopEnvironment {
            pairs: result.pairs,
            process_count,
            features: *features,
            warnings: result.warnings,
        }
    }

    /// The desktop environment built from `process` and `features` with no key source, as
    /// `prepare_environment` builds it when the keys cannot be read (and without a database or a
    /// root).
    fn desktop_without_keys(process: Pairs, features: &FeatureSettings) -> DesktopEnvironment {
        let process_count = process.len();
        let result = overlay(process, features, || None);
        DesktopEnvironment {
            pairs: result.pairs,
            process_count,
            features: *features,
            warnings: result.warnings,
        }
    }

    /// The flag values the HTTP child is started with, built the way the supervisor builds them:
    /// one entry per distinct flag variable.
    fn child_flags_of(prepared: &DesktopEnvironment) -> Vec<(&'static str, Option<String>)> {
        let token = secrecy::SecretString::from("t".repeat(64));
        let child =
            crate::mcp_server::child_environment(&prepared.supervisor_pairs(), 8765, &token, None);
        in_app_features()
            .map(|feature| {
                let value = child
                    .iter()
                    .find(|(name, _)| name.to_str() == Some(feature.variable()))
                    .map(|(_, value)| value.to_string_lossy().into_owned());
                (feature.variable(), value)
            })
            .collect()
    }

    fn child_flags(
        process: Pairs,
        features: &FeatureSettings,
    ) -> Vec<(&'static str, Option<String>)> {
        child_flags_of(&desktop(process, features))
    }

    /// The flag of `variable` in the list of `child_flags`.
    fn flag<'a>(flags: &'a [(&str, Option<String>)], variable: &str) -> Option<&'a str> {
        flags
            .iter()
            .find(|(name, _)| *name == variable)
            .and_then(|(_, value)| value.as_deref())
    }

    /// The variables of the three MCP capabilities.
    const GATED: [&str; 3] = [
        FLAG_KEY,
        LINK_FETCH_ENABLED_VARIABLE,
        DELETION_ENABLED_VARIABLE,
    ];

    fn only_gated(flags: &[(&str, Option<String>)], on: &[&str]) {
        for variable in GATED {
            let expected = on.contains(&variable).then_some("1");
            assert_eq!(flag(flags, variable), expected, "{variable}");
        }
    }

    #[test]
    fn the_http_child_gets_no_network_or_deletion_flag_by_default() {
        // Default settings and an empty process environment: the in-app features are on, and the
        // child still has neither the deletion tools, the link tool nor the Arbeitnow tools.
        let flags = child_flags(Vec::new(), &FeatureSettings::default());
        only_gated(&flags, &[]);
        // The other features cross as before.
        assert_eq!(flag(&flags, DOCUMENT_ARTIFACTS_VARIABLE), Some("1"));
        assert_eq!(flag(&flags, GENERATED_ENABLED_VARIABLE), Some("1"));
        // The app itself has the in-app features.
        let app = desktop(Vec::new(), &FeatureSettings::default()).environment();
        for variable in GATED {
            assert!(app.flag_enabled(variable), "{variable}");
        }
    }

    #[test]
    fn each_mcp_opt_in_adds_only_its_own_flag_to_the_child() {
        for (feature, variable) in [
            (Feature::McpDeletion, DELETION_ENABLED_VARIABLE),
            (Feature::McpLinkFetch, LINK_FETCH_ENABLED_VARIABLE),
            (Feature::McpArbeitnow, FLAG_KEY),
        ] {
            let mut features = FeatureSettings::default();
            features.set(feature, true);
            let flags = child_flags(Vec::new(), &features);
            only_gated(&flags, &[variable]);
        }
        let mut features = FeatureSettings::default();
        for feature in Feature::ALL {
            if feature.is_mcp_capability() {
                features.set(feature, true);
            }
        }
        only_gated(&child_flags(Vec::new(), &features), &GATED);
    }

    #[test]
    fn the_mcp_opt_in_does_not_depend_on_the_in_app_switch() {
        // The in-app switch off leaves the app without the feature and the child with it.
        for (in_app, mcp) in [
            (Feature::Deletion, Feature::McpDeletion),
            (Feature::LinkFetch, Feature::McpLinkFetch),
            (Feature::Arbeitnow, Feature::McpArbeitnow),
        ] {
            let mut features = FeatureSettings::default();
            features.set(in_app, false);
            features.set(mcp, true);
            let prepared = desktop(Vec::new(), &features);
            assert!(
                !prepared.environment().flag_enabled(in_app.variable()),
                "{in_app:?}"
            );
            let child = Environment::from_os_pairs(prepared.supervisor_pairs(), false);
            assert!(child.flag_enabled(in_app.variable()), "{mcp:?}");
            // And the other way round: the in-app switch on never reaches the child.
            let mut features = FeatureSettings::default();
            features.set(mcp, false);
            let prepared = desktop(Vec::new(), &features);
            assert!(prepared.environment().flag_enabled(in_app.variable()));
            let child = Environment::from_os_pairs(prepared.supervisor_pairs(), false);
            assert!(!child.contains(in_app.variable()), "{in_app:?}");
        }
    }

    #[test]
    fn a_general_flag_of_one_in_the_process_environment_is_no_mcp_opt_in() {
        // `JOBNIAC_*_ENABLED=1` enables the feature for the app (and the CLI); it is not a consent
        // to give an MCP client the capability, so the child does not inherit it.
        for variable in GATED {
            let flags = child_flags(os(&[(variable, "1")]), &FeatureSettings::default());
            assert_eq!(flag(&flags, variable), None, "{variable}");
            // The app still follows the variable.
            let app = desktop(os(&[(variable, "1")]), &FeatureSettings::default()).environment();
            assert!(app.flag_enabled(variable), "{variable}");
        }
        // Even with the in-app switches off the variable only enables the app's feature.
        let mut off = FeatureSettings::default();
        for feature in Feature::ALL {
            off.set(feature, false);
        }
        let process: Vec<_> = GATED.iter().map(|variable| (*variable, "1")).collect();
        let prepared = desktop(os(&process), &off);
        only_gated(&child_flags_of(&prepared), &[]);
        for variable in GATED {
            assert!(prepared.environment().flag_enabled(variable), "{variable}");
        }
        // The same holds when other variables are set around it.
        let flags = child_flags(
            os(&[
                (DELETION_ENABLED_VARIABLE, "1"),
                (GENERATED_ENABLED_VARIABLE, "1"),
            ]),
            &FeatureSettings::default(),
        );
        assert_eq!(flag(&flags, DELETION_ENABLED_VARIABLE), None);
        assert_eq!(flag(&flags, GENERATED_ENABLED_VARIABLE), Some("1"));
    }

    #[test]
    fn a_general_flag_of_one_and_the_opt_in_together_give_one_flag_to_the_child() {
        let mut features = FeatureSettings::default();
        features.set(Feature::McpDeletion, true);
        let prepared = desktop(os(&[(DELETION_ENABLED_VARIABLE, "1")]), &features);
        let pairs = prepared.supervisor_pairs();
        let flags: Vec<_> = pairs
            .iter()
            .filter(|(name, _)| name == DELETION_ENABLED_VARIABLE)
            .collect();
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].1, "1");
    }

    #[test]
    fn a_process_flag_other_than_one_is_an_explicit_refusal_the_opt_in_cannot_override() {
        for (variable, feature) in [
            (DELETION_ENABLED_VARIABLE, Feature::McpDeletion),
            (LINK_FETCH_ENABLED_VARIABLE, Feature::McpLinkFetch),
            (FLAG_KEY, Feature::McpArbeitnow),
        ] {
            let mut on = FeatureSettings::default();
            on.set(feature, true);
            for value in ["0", "true", "yes", "01", " 1", "", "2", "ON"] {
                let process = os(&[(variable, value)]);
                let flags = child_flags(process.clone(), &on);
                // Passed on as it is, so the child reads the refusal too.
                assert_eq!(flag(&flags, variable), Some(value), "{variable}={value:?}");
                assert!(
                    forced_by_environment(&Environment::from_os_pairs(process, false), feature),
                    "{variable}={value:?}"
                );
            }
        }
        // The unrelated flags are untouched by one refusal.
        let flags = child_flags(
            os(&[(DELETION_ENABLED_VARIABLE, "0")]),
            &FeatureSettings::default(),
        );
        assert_eq!(flag(&flags, GENERATED_ENABLED_VARIABLE), Some("1"));
        assert_eq!(flag(&flags, LINK_FETCH_ENABLED_VARIABLE), None);
    }

    #[test]
    fn the_environment_forces_an_mcp_capability_only_when_it_refuses_it() {
        let set = |value: &str| {
            Environment::from_os_pairs(os(&[(DELETION_ENABLED_VARIABLE, value)]), false)
        };
        // An in-app feature is forced by any set value; an MCP capability only by a refusal.
        assert!(forced_by_environment(&set("1"), Feature::Deletion));
        assert!(!forced_by_environment(&set("1"), Feature::McpDeletion));
        assert!(forced_by_environment(&set("0"), Feature::Deletion));
        assert!(forced_by_environment(&set("0"), Feature::McpDeletion));
        let unset = Environment::from_os_pairs(Vec::<(OsString, OsString)>::new(), false);
        assert!(!forced_by_environment(&unset, Feature::Deletion));
        assert!(!forced_by_environment(&unset, Feature::McpDeletion));
    }

    #[test]
    fn a_switch_that_is_off_leaves_its_flag_out_of_the_child() {
        let mut features = FeatureSettings::default();
        features.set(Feature::DocumentArtifacts, false);
        features.set(Feature::McpLinkFetch, true);
        let flags = child_flags(Vec::new(), &features);
        assert_eq!(flag(&flags, DOCUMENT_ARTIFACTS_VARIABLE), None);
        only_gated(&flags, &[LINK_FETCH_ENABLED_VARIABLE]);
    }

    #[test]
    fn the_arbeitnow_opt_in_needs_valid_keys_and_leaves_the_app_flag_alone() {
        // Without a key source the child gets no Arbeitnow flag (it would fail to start on it),
        // with a warning, and the app has none either.
        let mut features = FeatureSettings::default();
        features.set(Feature::Arbeitnow, false);
        features.set(Feature::McpArbeitnow, true);
        let prepared = desktop_without_keys(Vec::new(), &features);
        assert_eq!(prepared.warnings, vec![FeatureWarning::KeysUnavailable]);
        only_gated(&child_flags_of(&prepared), &[]);
        // With keys, the child gets the flag and the keys; the app has the keys and no flag.
        let prepared = desktop(Vec::new(), &features);
        assert!(prepared.warnings.is_empty());
        let app = prepared.environment();
        assert!(!app.flag_enabled(FLAG_KEY));
        assert!(app.contains(CANDIDATE_KEY) && app.contains(CURSOR_SECRET_KEY));
        let child = Environment::from_os_pairs(prepared.supervisor_pairs(), false);
        assert!(child.flag_enabled(FLAG_KEY));
        assert!(matches!(load_arbeitnow_config(&child), Ok(Some(_))));
        // A stale key of the environment keeps the flag from a child that would not start.
        let prepared = desktop(os(&[(CURSOR_SECRET_KEY, "   ")]), &features);
        assert_eq!(prepared.warnings, vec![FeatureWarning::KeysInvalid]);
        only_gated(&child_flags_of(&prepared), &[]);
    }

    #[test]
    fn no_keys_are_made_for_arbeitnow_when_neither_the_app_nor_mcp_wants_it() {
        let mut features = FeatureSettings::default();
        features.set(Feature::Arbeitnow, false);
        let result = overlay(Vec::new(), &features, || {
            panic!("the keys must not be read while Arbeitnow is off for the app and for MCP")
        });
        assert!(!effective(&result).contains(CANDIDATE_KEY));
        // An environment refusal of the flag also keeps the MCP opt-in from asking for keys.
        features.set(Feature::McpArbeitnow, true);
        let result = overlay(os(&[(FLAG_KEY, "0")]), &features, || {
            panic!("the keys must not be read while the environment refuses Arbeitnow")
        });
        assert!(!effective(&result).contains(CANDIDATE_KEY));
    }

    #[test]
    fn the_supervisor_pairs_keep_the_process_pairs_and_the_added_order() {
        let process = os(&[("PATH", "/usr/bin"), ("JOBNIAC_DB_PATH", "/x")]);
        let mut features = FeatureSettings::default();
        features.set(Feature::McpDeletion, true);
        let prepared = desktop(process.clone(), &features);
        let pairs = prepared.supervisor_pairs();
        assert_eq!(&pairs[..process.len()], process.as_slice());
        assert!(pairs.contains(&(
            OsString::from(DELETION_ENABLED_VARIABLE),
            OsString::from("1")
        )));
        // The app's own copy of the in-app deletion flag is dropped, so the flag appears once.
        let count = pairs
            .iter()
            .filter(|(name, _)| name == DELETION_ENABLED_VARIABLE)
            .count();
        assert_eq!(count, 1);
    }

    /// A value that starts with `1` and is not Unicode, where the platform can build one.
    fn non_unicode_one() -> Option<OsString> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Some(OsString::from_vec(vec![0x31, 0xff]))
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            // A lone surrogate is not valid UTF-16.
            Some(OsString::from_wide(&[0x31, 0xD800]))
        }
        #[cfg(not(any(unix, windows)))]
        {
            None
        }
    }

    fn every_mcp_switch_on() -> FeatureSettings {
        let mut features = FeatureSettings::default();
        for feature in Feature::ALL {
            if feature.is_mcp_capability() {
                features.set(feature, true);
            }
        }
        features
    }

    fn has_pair(pairs: &[(OsString, OsString)], name: &str) -> bool {
        pairs.iter().any(|(pair, _)| {
            pair.to_str()
                .is_some_and(|pair| pair.eq_ignore_ascii_case(name))
        })
    }

    #[test]
    fn the_base_pairs_carry_no_capability_flag_and_no_arbeitnow_key() {
        // Everything is granted, and the process sets general flags of `1` too.
        let process = os(&[
            (DELETION_ENABLED_VARIABLE, "1"),
            (LINK_FETCH_ENABLED_VARIABLE, "1"),
            (FLAG_KEY, "1"),
            (CANDIDATE_KEY, CANDIDATE),
            (CURSOR_SECRET_KEY, SECRET),
        ]);
        let prepared = desktop(process, &every_mcp_switch_on());
        let base = prepared.supervisor_base_pairs();
        for name in GATED {
            assert!(!has_pair(&base, name), "{name}");
        }
        // The candidate key of the process stays out; its cursor secret (a documented setting) stays.
        assert!(!has_pair(&base, CANDIDATE_KEY));
        assert!(base.contains(&(OsString::from(CURSOR_SECRET_KEY), OsString::from(SECRET))));
        // The grants bring the flags and the candidate key back, exactly once each.
        let grants = prepared.mcp_grants().pairs();
        for name in GATED {
            assert_eq!(
                grants.iter().filter(|(pair, _)| pair == name).count(),
                1,
                "{name}"
            );
        }
        assert!(grants.contains(&(OsString::from(CANDIDATE_KEY), OsString::from(CANDIDATE))));
        // The secret of the process is not duplicated into the grant.
        assert!(!has_pair(&grants, CURSOR_SECRET_KEY));
    }

    #[test]
    fn a_generated_cursor_secret_and_the_candidate_key_cross_only_with_the_arbeitnow_grant() {
        // The app generated the keys (the in-app Arbeitnow is on) but MCP Arbeitnow is off.
        let prepared = desktop(Vec::new(), &FeatureSettings::default());
        let app = prepared.environment();
        assert!(app.contains(CANDIDATE_KEY) && app.contains(CURSOR_SECRET_KEY));
        let child = prepared.supervisor_pairs();
        assert!(!has_pair(&child, CANDIDATE_KEY));
        assert!(!has_pair(&child, CURSOR_SECRET_KEY));
        assert!(!has_pair(&child, FLAG_KEY));
        // With the grant, the child gets both keys and the flag, and no more.
        let mut on = FeatureSettings::default();
        on.set(Feature::McpArbeitnow, true);
        let prepared = desktop(Vec::new(), &on);
        let grants = prepared.mcp_grants();
        assert!(grants.granted_at_startup(Feature::McpArbeitnow));
        let pairs = grants.pairs();
        assert!(has_pair(&pairs, FLAG_KEY));
        assert!(has_pair(&pairs, CANDIDATE_KEY));
        assert!(has_pair(&pairs, CURSOR_SECRET_KEY));
        // Narrowing the switch drops all three pairs from the allowed set.
        assert!(grants.pairs_for(&FeatureSettings::default()).is_empty());
    }

    #[test]
    fn the_grants_at_startup_follow_the_switches_and_the_refusals() {
        assert_eq!(
            desktop(Vec::new(), &FeatureSettings::default()).mcp_grants(),
            McpGrants::default()
        );
        let prepared = desktop(Vec::new(), &every_mcp_switch_on());
        let grants = prepared.mcp_grants();
        for feature in Feature::ALL {
            assert_eq!(
                grants.granted_at_startup(feature),
                feature.is_mcp_capability(),
                "{feature:?}"
            );
        }
        // A refusal of the process environment leaves the grant out.
        let refused = desktop(
            os(&[(DELETION_ENABLED_VARIABLE, "0")]),
            &every_mcp_switch_on(),
        );
        let grants = refused.mcp_grants();
        assert!(!grants.granted_at_startup(Feature::McpDeletion));
        assert!(grants.granted_at_startup(Feature::McpLinkFetch));
        // The allowed pairs follow the current switches, one feature at a time.
        let all = prepared.mcp_grants();
        let mut features = every_mcp_switch_on();
        features.set(Feature::McpLinkFetch, false);
        let allowed = all.pairs_for(&features);
        assert!(has_pair(&allowed, DELETION_ENABLED_VARIABLE));
        assert!(!has_pair(&allowed, LINK_FETCH_ENABLED_VARIABLE));
        assert!(has_pair(&allowed, FLAG_KEY));
        // Arbeitnow without a usable key pair is not granted.
        let mut arbeitnow = FeatureSettings::default();
        arbeitnow.set(Feature::McpArbeitnow, true);
        let unusable = desktop_without_keys(Vec::new(), &arbeitnow);
        assert!(
            !unusable
                .mcp_grants()
                .granted_at_startup(Feature::McpArbeitnow)
        );
    }

    #[test]
    fn capability_flags_match_without_regard_to_case_in_the_base_pairs() {
        // The Windows environment is case-insensitive, so `Jobniac_Deletion_Enabled=1` is the
        // general flag. It never reaches the child, on any platform.
        let process = os(&[
            ("Jobniac_Deletion_Enabled", "1"),
            ("jobniac_link_fetch_enabled", "1"),
            ("JOBNIAC_arbeitnow_ENABLED", "1"),
        ]);
        let prepared = desktop(process, &FeatureSettings::default());
        let base = prepared.supervisor_base_pairs();
        for name in GATED {
            assert!(!has_pair(&base, name), "{name}");
        }
        // With the opt-in the child gets the canonical flag, once.
        let prepared = desktop(
            os(&[("Jobniac_Deletion_Enabled", "1")]),
            &every_mcp_switch_on(),
        );
        let pairs = prepared.supervisor_pairs();
        assert_eq!(
            pairs
                .iter()
                .filter(|(name, _)| {
                    name.to_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case(DELETION_ENABLED_VARIABLE))
                })
                .count(),
            1
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_mixed_case_refusal_wins_over_the_opt_in_on_windows() {
        let process = os(&[
            ("Jobniac_Deletion_Enabled", "0"),
            ("jobniac_link_fetch_enabled", "false"),
            ("JOBNIAC_Arbeitnow_Enabled", ""),
        ]);
        let prepared = desktop(process, &every_mcp_switch_on());
        let grants = prepared.mcp_grants();
        for feature in [
            Feature::McpDeletion,
            Feature::McpLinkFetch,
            Feature::McpArbeitnow,
        ] {
            assert!(!grants.granted_at_startup(feature), "{feature:?}");
        }
        // The refusals cross as they are, under the case the process used.
        let child = Environment::from_os_pairs(prepared.supervisor_pairs(), true);
        for name in GATED {
            assert!(!child.flag_enabled(name), "{name}");
        }
        assert!(child.contains(DELETION_ENABLED_VARIABLE));
    }

    #[test]
    fn a_non_unicode_flag_value_is_a_refusal_and_never_a_grant() {
        let Some(bad) = non_unicode_one() else {
            return;
        };
        for variable in GATED {
            let process = vec![(OsString::from(variable), bad.clone())];
            let prepared = desktop(process, &every_mcp_switch_on());
            let grants = prepared.mcp_grants();
            let feature = Feature::ALL
                .into_iter()
                .find(|feature| feature.is_mcp_capability() && feature.variable() == variable);
            assert!(
                feature.is_some_and(|feature| !grants.granted_at_startup(feature)),
                "{variable}"
            );
            // The value crosses as it is (a refusal), and the child cannot read it as `1`.
            let child = Environment::from_os_pairs(prepared.supervisor_pairs(), cfg!(windows));
            assert!(!child.flag_enabled(variable), "{variable}");
        }
    }

    #[test]
    fn the_debug_text_of_the_grants_and_the_environment_shows_no_secret_value() {
        let process = os(&[
            (CANDIDATE_KEY, CANDIDATE),
            (CURSOR_SECRET_KEY, SECRET),
            ("JOBNIAC_DB_PATH", "/srv/jobniac-synthetic/db"),
        ]);
        let prepared = desktop(process.clone(), &every_mcp_switch_on());
        let grants = prepared.mcp_grants();
        assert!(grants.granted_at_startup(Feature::McpArbeitnow));
        let overlaid = run(process, &every_mcp_switch_on());
        // The keys the app generated are the fixed pair of the test, so they are covered too.
        let generated = desktop(Vec::new(), &every_mcp_switch_on());
        for text in [
            format!("{grants:?}"),
            format!("{grants:#?}"),
            format!("{prepared:?}"),
            format!("{prepared:#?}"),
            format!("{overlaid:?}"),
            format!("{generated:?}"),
            format!("{:?}", generated.mcp_grants()),
        ] {
            assert!(!text.contains(CANDIDATE), "{text}");
            assert!(!text.contains(SECRET), "{text}");
            assert!(!text.contains("srv/jobniac-synthetic"), "{text}");
        }
        // It still says what was granted, and for the environment how much it holds.
        let text = format!("{grants:?}");
        assert!(
            text.contains("McpGrants") && text.contains("mcpArbeitnow"),
            "{text}"
        );
        assert!(format!("{prepared:?}").contains("variables"));
        // The default grants show nothing granted.
        assert!(format!("{:?}", McpGrants::default()).contains("[]"));
    }

    #[test]
    fn a_grant_is_allowed_only_while_its_own_switch_is_on() {
        let prepared = desktop(Vec::new(), &every_mcp_switch_on());
        let grants = prepared.mcp_grants();
        let deletion = (
            OsString::from(DELETION_ENABLED_VARIABLE),
            OsString::from("1"),
        );
        let candidate = (OsString::from(CANDIDATE_KEY), OsString::from(CANDIDATE));
        let unknown = (OsString::from("JOBNIAC_UNRELATED"), OsString::from("1"));
        let on = every_mcp_switch_on();
        assert!(grants.allows(&on, &deletion));
        assert!(grants.allows(&on, &candidate));
        assert!(!grants.allows(&on, &unknown));
        let mut off = every_mcp_switch_on();
        off.set(Feature::McpDeletion, false);
        assert!(!grants.allows(&off, &deletion));
        assert!(grants.allows(&off, &candidate));
        off.set(Feature::McpArbeitnow, false);
        assert!(!grants.allows(&off, &candidate));
        // The decision agrees with the list the other API builds.
        for pair in grants.pairs() {
            assert_eq!(
                grants.allows(&off, &pair),
                grants.pairs_for(&off).contains(&pair)
            );
        }
    }

    #[test]
    fn feature_states_record_forced_and_effective_per_feature() {
        let states = FeatureStates::new()
            .with_forced(Feature::Deletion, true)
            .with_effective(Feature::Generation, true);
        assert!(states.forced(Feature::Deletion) && !states.forced(Feature::Generation));
        assert!(states.effective(Feature::Generation) && !states.effective(Feature::Deletion));
        let states = states.with_started_on(Feature::Deletion, false);
        assert!(!states.started_on(Feature::Deletion) && states.started_on(Feature::Arbeitnow));
        let states = states.with_revoked(Feature::McpDeletion, true);
        assert!(states.revoked(Feature::McpDeletion) && !states.revoked(Feature::McpLinkFetch));
    }
}
