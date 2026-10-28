//! The optional features of the desktop app: on by default, switchable in Settings, and still
//! overridable by the process environment.
//!
//! Every optional feature is gated on a flag variable that is exactly `1` (see the loaders of the
//! feature modules), and a release app started from the Start menu or Finder has none set. The
//! desktop app therefore composes from an effective environment instead of the raw one: the process
//! environment overlaid with defaults. For each feature flag, a variable the process environment
//! sets, whatever its value, wins (so `0` or any value other than `1` turns the feature off);
//! otherwise the flag is `1` when the Settings switch of the feature is on. The CLI never calls this
//! module: it keeps its environment-only, off-by-default behaviour.
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
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

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
}

impl Feature {
    /// Every feature, in display order.
    pub const ALL: [Self; 5] = [
        Self::Arbeitnow,
        Self::LinkFetch,
        Self::Deletion,
        Self::DocumentArtifacts,
        Self::Generation,
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
        }
    }

    /// The feature of an identifier.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|feature| feature.id() == id)
    }

    /// The flag variable that gates the feature.
    #[must_use]
    pub const fn variable(self) -> &'static str {
        match self {
            Self::Arbeitnow => FLAG_KEY,
            Self::LinkFetch => LINK_FETCH_ENABLED_VARIABLE,
            Self::Deletion => DELETION_ENABLED_VARIABLE,
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
        }
    }
}

/// Whether the process environment sets the flag of `feature` (any value, even an empty or
/// non-Unicode one). A set flag overrides the Settings switch.
#[must_use]
pub fn forced_by_environment(process: &Environment, feature: Feature) -> bool {
    process.contains(feature.variable())
}

/// What the running app made of each feature: whether the environment forced it, and whether it is
/// actually composed. Taken once at startup; a Settings change applies only after a restart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeatureStates {
    forced: [bool; 5],
    effective: [bool; 5],
    started_on: [bool; 5],
}

impl FeatureStates {
    /// The states with every feature unforced and not composed, and every Settings switch on at
    /// startup (the default of a first run).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            forced: [false; 5],
            effective: [false; 5],
            started_on: [true; 5],
        }
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    /// The process pairs followed by the defaults that were added.
    pub pairs: Vec<(OsString, OsString)>,
    /// The features that were left off.
    pub warnings: Vec<FeatureWarning>,
}

/// Overlays the defaults on the process environment. Pure: `keys` is called at most once, only when
/// Arbeitnow is wanted and the environment lacks one of its keys.
///
/// A flag the environment sets is never touched. An unset flag becomes `1` when its Settings switch
/// is on, except Arbeitnow, which also needs a valid key pair: the environment's keys, completed
/// from `keys`. Without a valid pair the flag is left unset (Arbeitnow off) and a warning says so,
/// where an environment-set flag with unusable keys keeps failing startup as it always did.
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
            && !forced_by_environment(&snapshot, feature)
            && features.is_on(feature)
        {
            pairs.push((feature.variable().into(), "1".into()));
        }
    }
    let forced = forced_by_environment(&snapshot, Feature::Arbeitnow);
    let wanted = if forced {
        snapshot.flag_enabled(FLAG_KEY)
    } else {
        features.arbeitnow
    };
    if wanted {
        let usable = complete_arbeitnow_keys(&snapshot, &mut pairs, &mut warnings, keys);
        // A flag of the environment stays as it is; the default flag needs a usable key pair.
        if !forced && usable {
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

/// The environment the desktop app composes from.
#[derive(Debug, Clone)]
pub struct DesktopEnvironment {
    /// The effective variable pairs.
    pub pairs: Vec<(OsString, OsString)>,
    /// The features that were left off, with a fixed warning each.
    pub warnings: Vec<FeatureWarning>,
}

impl DesktopEnvironment {
    /// The snapshot every compose call takes.
    #[must_use]
    pub fn environment(&self) -> Environment {
        Environment::from_os_pairs(self.pairs.iter().cloned(), cfg!(windows))
    }

    /// The pairs the supervised `jobniac-cli http` child is built from. The child does not inherit
    /// `HOME` or `LOCALAPPDATA`, so it could not find the per-user default generated root: with
    /// generation on and no root set, the root the app registered is named explicitly.
    #[must_use]
    pub fn supervisor_pairs(&self) -> Vec<(OsString, OsString)> {
        let env = self.environment();
        let mut pairs = self.pairs.clone();
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

    #[test]
    fn an_empty_environment_turns_every_feature_on() {
        let result = run(Vec::new(), &FeatureSettings::default());
        for feature in Feature::ALL {
            assert!(on(&result, feature), "{feature:?}");
        }
        assert!(result.warnings.is_empty());
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
        let process: Vec<_> = Feature::ALL
            .iter()
            .map(|feature| (feature.variable(), "1"))
            .collect();
        let result = run(os(&process), &features);
        for feature in Feature::ALL {
            assert!(on(&result, feature), "{feature:?}");
        }
    }

    #[test]
    fn a_switch_that_is_off_leaves_its_flag_unset() {
        for off in Feature::ALL {
            let mut features = FeatureSettings::default();
            features.set(off, false);
            let result = run(Vec::new(), &features);
            for feature in Feature::ALL {
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
        let dir = tempfile::tempdir()?;
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
        let dir = tempfile::tempdir()?;
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
            warnings: Vec::new(),
        };
        assert_eq!(managed.supervisor_pairs(), managed.pairs);
        Ok(())
    }

    /// The flag values the HTTP child is started with, built the way the supervisor builds them.
    fn child_flags(process: Pairs, features: &FeatureSettings) -> Vec<(Feature, Option<String>)> {
        let result = run(process, features);
        let prepared = DesktopEnvironment {
            pairs: result.pairs,
            warnings: result.warnings,
        };
        let token = secrecy::SecretString::from("t".repeat(64));
        let child =
            crate::mcp_server::child_environment(&prepared.supervisor_pairs(), 8765, &token, None);
        Feature::ALL
            .iter()
            .map(|feature| {
                let value = child
                    .iter()
                    .find(|(name, _)| name.to_str() == Some(feature.variable()))
                    .map(|(_, value)| value.to_string_lossy().into_owned());
                (*feature, value)
            })
            .collect()
    }

    #[test]
    fn the_http_child_gets_the_same_feature_flags_as_the_app() {
        // Default settings and an empty process environment: every flag is `1` in the child.
        for (feature, value) in child_flags(Vec::new(), &FeatureSettings::default()) {
            assert_eq!(value.as_deref(), Some("1"), "{feature:?}");
        }
        // A flag of the process environment wins and crosses as it is.
        for (feature, value) in child_flags(
            os(&[(DELETION_ENABLED_VARIABLE, "0")]),
            &FeatureSettings::default(),
        ) {
            let expected = if feature == Feature::Deletion {
                "0"
            } else {
                "1"
            };
            assert_eq!(value.as_deref(), Some(expected), "{feature:?}");
        }
        // A Settings switch that is off leaves its flag out of the child.
        let mut features = FeatureSettings::default();
        features.set(Feature::LinkFetch, false);
        for (feature, value) in child_flags(Vec::new(), &features) {
            let expected = (feature != Feature::LinkFetch).then_some("1");
            assert_eq!(value.as_deref(), expected, "{feature:?}");
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
    }
}
