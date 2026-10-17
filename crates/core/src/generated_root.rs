//! Pure resolution and layout rules of the owned generated-artifact root.
//!
//! Nothing here touches the filesystem. The default root is derived from an injected
//! [`Environment`] snapshot only. The
//! Windows default accepts drive-absolute bases only: UNC, device (`\\?\`, `\\.\`) and
//! drive-relative forms are refused. Creation, qualification and
//! file safety live in `jobniac-infra`.

use crate::env::{EnvValue, Environment};
use uuid::Uuid;

/// Longest accepted base or root path, in code points.
pub const MAX_ROOT_CHARS: usize = 4096;
/// Directory name of the application under the Windows and macOS bases.
pub const APP_WINDOWS: &str = "Jobniac";
/// Directory name of the application under the XDG and home bases.
pub const APP_POSIX: &str = "jobniac";
/// Leaf directory of the default root on every platform.
pub const GENERATED_DIR: &str = "generated";

/// Name of the owned marker file of a set-up root.
pub const MARKER_NAME: &str = ".jobniac-generated-root";
/// Name of the empty namespace lock file of a set-up root.
pub const LOCK_NAME: &str = ".jobniac-generated-lock";
/// Name of the root-bound capability file of a set-up root.
pub const CAPABILITY_NAME: &str = ".jobniac-generated-capability";
/// First line of the marker file (ASCII, newline terminated).
pub const MARKER_HEADER: &str = "jobniac-generated-root-v1";
/// Largest marker file accepted, in bytes.
pub const MARKER_LIMIT: usize = 128;
/// First line of every capability file.
pub const CAPABILITY_HEADER: &str = "jobniac-generated-capability-v1";
/// Mechanism tag of a capability published on Linux (anonymous inode, proc link, fsync).
pub const LINUX_CAPABILITY_MECHANISM: &str = "linux-anonymous-proc-link-fsync";
/// Mechanism tag of a capability published on Windows (create-new, no-replace rename).
pub const WINDOWS_CAPABILITY_MECHANISM: &str = "windows-createnew-rename-noreplace";
/// Largest generated PDF, in bytes.
pub const MAX_GENERATED_BYTES: u64 = 10 * 1024 * 1024;

/// Platform classes that decide the default root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootPlatform {
    /// `win32` and any other name starting with `win`.
    Windows,
    /// `darwin`.
    MacOs,
    /// Linux, BSD and every other platform name.
    Other,
}

impl RootPlatform {
    /// Classifies a platform name such as `win32`, `darwin` or `linux`.
    #[must_use]
    pub fn from_platform_name(name: &str) -> Self {
        if name.starts_with("win") {
            Self::Windows
        } else if name == "darwin" {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

/// Returns `value` unchanged when it is a clean absolute base for `platform`, else `None`.
///
/// A usable base is non-empty, at most [`MAX_ROOT_CHARS`] code points, has no leading or
/// trailing whitespace and no code point below 32, is absolute for the platform (drive-absolute
/// on Windows) and has no `.` or `..` component (`\` also separates components on Windows).
fn usable_base(value: &str, platform: RootPlatform) -> Option<&str> {
    if value.is_empty() || value.chars().count() > MAX_ROOT_CHARS {
        return None;
    }
    if value.trim() != value || value.chars().any(|c| (c as u32) < 32) {
        return None;
    }
    if platform == RootPlatform::Windows {
        if !is_drive_absolute(value) {
            return None;
        }
        let segments = value.replace('\\', "/");
        if segments.split('/').any(|s| s == "." || s == "..") {
            return None;
        }
    } else {
        if !value.starts_with('/') {
            return None;
        }
        if value.split('/').any(|s| s == "." || s == "..") {
            return None;
        }
    }
    Some(value)
}

/// Whether `value` starts with a drive letter, a colon and a separator (`C:\` or `C:/`).
fn is_drive_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// Joins path components with the platform separator, as `ntpath.join` and `posixpath.join` do
/// for relative components.
fn join(base: &str, parts: &[&str], separator: char) -> String {
    let mut joined = base.to_owned();
    if !joined.ends_with(separator) && !joined.ends_with('/') && !joined.ends_with('\\') {
        joined.push(separator);
    }
    joined.push_str(&parts.join(&separator.to_string()));
    joined
}

/// Admits an explicit root path (`JOBNIAC_GENERATED_ROOT`) under the same rules as a base.
///
/// Returns the path unchanged when it is usable; any other value yields `None` and must never be
/// replaced by the default.
#[must_use]
pub fn admit_root_path(value: &str, platform: RootPlatform) -> Option<String> {
    usable_base(value, platform).map(str::to_owned)
}

/// Resolves the per-user default generated root from the injected environment.
///
/// - Windows: `%LOCALAPPDATA%\Jobniac\generated` (drive-absolute base only).
/// - macOS: `$HOME/Library/Application Support/Jobniac/generated`.
/// - Other: `$XDG_DATA_HOME/jobniac/generated` when the value is absolute and usable; an empty or
///   relative value is treated as unset and falls back to `$HOME/.local/share/jobniac/generated`.
///   An absolute but malformed value fails closed and never falls back to `HOME`.
///
/// Returns `None` (fail closed) whenever the base in use is missing or unusable.
#[must_use]
pub fn resolve_default_generated_root(
    environment: &Environment,
    platform: RootPlatform,
) -> Option<String> {
    match platform {
        RootPlatform::Windows => {
            let base = usable_base(valid_value(environment, "LOCALAPPDATA")?, platform)?;
            Some(join(base, &[APP_WINDOWS, GENERATED_DIR], '\\'))
        }
        RootPlatform::MacOs => {
            let home = usable_base(valid_value(environment, "HOME")?, platform)?;
            Some(join(
                home,
                &["Library", "Application Support", APP_WINDOWS, GENERATED_DIR],
                '/',
            ))
        }
        RootPlatform::Other => {
            // An invalid-Unicode XDG value is malformed (fail closed), never a silent fallback.
            let raw_xdg = match environment.lookup("XDG_DATA_HOME") {
                EnvValue::Invalid => return None,
                EnvValue::Missing => "",
                EnvValue::Valid(value) => value,
            };
            if raw_xdg.starts_with('/') {
                return usable_base(raw_xdg, platform)
                    .map(|xdg| join(xdg, &[APP_POSIX, GENERATED_DIR], '/'));
            }
            let home = usable_base(valid_value(environment, "HOME")?, platform)?;
            Some(join(
                home,
                &[".local", "share", APP_POSIX, GENERATED_DIR],
                '/',
            ))
        }
    }
}

/// The value of a variable that is present and valid Unicode; `None` when missing or invalid.
fn valid_value<'a>(environment: &'a Environment, key: &str) -> Option<&'a str> {
    match environment.lookup(key) {
        EnvValue::Valid(value) => Some(value),
        EnvValue::Missing | EnvValue::Invalid => None,
    }
}

/// Exact bytes of a root marker: the header line and the canonical lowercase root identity.
#[must_use]
pub fn marker_bytes(root_id: Uuid) -> Vec<u8> {
    format!("{MARKER_HEADER}\n{root_id}\n").into_bytes()
}

/// Parses marker bytes into an *unproven candidate* identity, or `None` when the marker is not
/// well formed.
///
/// The content must be ASCII, at most [`MARKER_LIMIT`] bytes, exactly two LF-terminated lines:
/// the header, then the 36-character canonical lowercase UUID. Trust is not established here
/// (a marker alone is never enough).
#[must_use]
pub fn parse_marker(data: &[u8]) -> Option<Uuid> {
    if data.len() > MARKER_LIMIT || !data.is_ascii() {
        return None;
    }
    let text = std::str::from_utf8(data).ok()?;
    let mut lines = text.split('\n');
    let header = lines.next()?;
    let identifier = lines.next()?;
    if lines.next() != Some("") || lines.next().is_some() {
        return None;
    }
    if header != MARKER_HEADER || identifier.len() != 36 {
        return None;
    }
    let root_id = Uuid::parse_str(identifier).ok()?;
    (root_id.hyphenated().to_string() == identifier).then_some(root_id)
}

/// Exact bytes of a Linux capability file: root identity plus the inode it was published to.
#[must_use]
pub fn linux_capability_bytes(root_id: Uuid, device: u64, inode: u64) -> Vec<u8> {
    format!("{CAPABILITY_HEADER}\n{LINUX_CAPABILITY_MECHANISM}\n{root_id}\n{device}\n{inode}\n")
        .into_bytes()
}

/// Exact bytes of a Windows capability file: root identity plus volume serial and file index.
#[must_use]
pub fn windows_capability_bytes(root_id: Uuid, volume: u64, index: u64) -> Vec<u8> {
    format!("{CAPABILITY_HEADER}\n{WINDOWS_CAPABILITY_MECHANISM}\n{root_id}\n{volume}\n{index}\n")
        .into_bytes()
}

/// Prefix of the temporary names of an interrupted Windows publication.
pub const TEMP_PREFIX: &str = ".jobniac-tmp-";

/// Whether `name` is a stale owned temporary file: the prefix plus 32 lowercase hex digits.
#[must_use]
pub fn is_owned_temp_name(name: &str) -> bool {
    name.strip_prefix(TEMP_PREFIX).is_some_and(|digits| {
        digits.len() == 32
            && digits
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Immutable file name of a generated asset: `<asset-id>.pdf`.
#[must_use]
pub fn asset_file_name(asset_id: Uuid) -> String {
    format!("{asset_id}.pdf")
}

/// Name of the permanent retirement fence of an asset: `.jobniac-retired-<asset-id>`.
#[must_use]
pub fn fence_name(asset_id: Uuid) -> String {
    format!(".jobniac-retired-{asset_id}")
}

/// Exact bytes of a retirement fence; a late writer must find this content before publication.
#[must_use]
pub fn fence_bytes(root_id: Uuid, asset_id: Uuid, sha256: &str, size_bytes: u64) -> Vec<u8> {
    format!("jobniac-retired-asset-v1\n{root_id}\n{asset_id}\n{sha256}\n{size_bytes}\n")
        .into_bytes()
}
