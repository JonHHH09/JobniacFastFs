//! Opt-in Arbeitnow signing keys.
//!
//! Errors never echo a setting value or a key.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use jobniac_core::arbeitnow::board::Page;
use jobniac_core::arbeitnow::candidates::CandidateCodec;
use jobniac_core::arbeitnow::cursor::CursorKey;
use jobniac_core::arbeitnow::failure::BoardFailure;
use jobniac_core::arbeitnow::import::{ArbeitnowJobImportService, ImportOutcome, ImportRequest};
use jobniac_core::arbeitnow::instant::ArbeitnowInstant;
use jobniac_core::arbeitnow::scan::{
    ArbeitnowBoard, ArbeitnowScanService, ScanRequest, ScanResult,
};
use jobniac_core::clock::SystemClock;
use jobniac_core::env::Environment;
use jobniac_core::errors::{ApplicationError, ErrorCode};
use jobniac_infra::arbeitnow::board::admit_board_result;
use jobniac_infra::arbeitnow::sqlite_repository::SqliteArbeitnowImportRepository;
use jobniac_infra::sqlite::pool::SqlitePool;
use jobniac_infra::sqlite::unit_of_work::SqliteUnitOfWork;
use jobniac_infra::workers::WorkerSupervisor;
use secrecy::{ExposeSecret, SecretSlice};

use crate::worker_fetch::WorkerBoardTransport;

/// Opt-in flag of the Arbeitnow services: admitted only as exactly `1`.
pub const FLAG_KEY: &str = "JOBNIAC_ARBEITNOW_ENABLED";
/// The base64url candidate signing key.
pub const CANDIDATE_KEY: &str = "JOBNIAC_ARBEITNOW_CANDIDATE_KEY";
/// The cursor HMAC secret.
pub const CURSOR_SECRET_KEY: &str = "JOBNIAC_CURSOR_HMAC_SECRET";
/// Maximum number of base64url data characters, before the optional padding.
const MAX_ENCODED_CHARS: usize = 1024;
const MIN_SECRET_LEN: usize = 32;
const MAX_SECRET_LEN: usize = 1024;
const MIN_KEY_BYTES: usize = 32;
/// Rejects trivially guessable keys such as repeated characters.
const MIN_DISTINCT_BYTES: usize = 16;
/// Scans that may be in flight at once, matching the two worker children of the fetch kind. A
/// further scan is refused at once instead of parking another thread behind the supervisor.
pub const MAX_CONCURRENT_SCANS: usize = 2;

/// An explicitly enabled optional feature has unusable companion settings.
///
/// The message is fixed on purpose: it never carries a setting value or a key.
#[derive(Debug, thiserror::Error)]
#[error("Optional feature configuration is invalid")]
pub struct OptInConfigError;

/// Private signing keys for the opt-in Arbeitnow scan and import services.
pub struct ArbeitnowConfig {
    candidate_key: SecretSlice<u8>,
    cursor_key: SecretSlice<u8>,
}

impl ArbeitnowConfig {
    /// The decoded candidate signing key, at least 32 bytes.
    pub fn candidate_key(&self) -> &[u8] {
        self.candidate_key.expose_secret()
    }

    /// The cursor HMAC secret, 32 to 1024 ASCII bytes.
    pub fn cursor_key(&self) -> &[u8] {
        self.cursor_key.expose_secret()
    }
}

impl fmt::Debug for ArbeitnowConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArbeitnowConfig")
            .field("candidate_key", &"<redacted>")
            .field("cursor_key", &"<redacted>")
            .finish()
    }
}

/// Load the Arbeitnow keys when `JOBNIAC_ARBEITNOW_ENABLED` is exactly `1`.
///
/// Any other flag value, including absence, returns `Ok(None)` whatever the key settings contain.
/// When the flag is `1`, a missing, malformed, short or non-distinct key fails closed.
pub fn load_arbeitnow_config(
    env: &Environment,
) -> Result<Option<ArbeitnowConfig>, OptInConfigError> {
    if !env.flag_enabled(FLAG_KEY) {
        return Ok(None);
    }
    let encoded = env.get(CANDIDATE_KEY).ok_or(OptInConfigError)?;
    let secret = env.get(CURSOR_SECRET_KEY).ok_or(OptInConfigError)?;
    let data = base64url_data(encoded).ok_or(OptInConfigError)?;
    if !is_valid_cursor_secret(secret) {
        return Err(OptInConfigError);
    }
    let candidate = decode_base64url(data).ok_or(OptInConfigError)?;
    let cursor = secret.as_bytes();
    if candidate.len() < MIN_KEY_BYTES
        || candidate == cursor
        || data.as_bytes() == cursor
        || distinct_bytes(&candidate) < MIN_DISTINCT_BYTES
        || distinct_bytes(cursor) < MIN_DISTINCT_BYTES
    {
        return Err(OptInConfigError);
    }
    Ok(Some(ArbeitnowConfig {
        candidate_key: SecretSlice::from(candidate),
        cursor_key: SecretSlice::from(cursor.to_vec()),
    }))
}

/// The scan use case over the sandboxed board transport.
pub type ArbeitnowScanUseCase = ArbeitnowScanService<WorkerArbeitnowBoard>;

/// The trusted import use case over the SQLite import repository.
pub type ArbeitnowImportUseCase = ArbeitnowJobImportService<SqliteArbeitnowImportRepository>;

/// The opt-in Arbeitnow use cases, composed only behind the flag, valid keys and a database.
#[derive(Debug, Clone)]
pub struct ArbeitnowServices {
    scan: Arc<ArbeitnowScanUseCase>,
    import: Arc<ArbeitnowImportUseCase>,
    gate: Arc<ScanGate>,
}

/// A counting gate over the scans in flight. Slots are taken without blocking.
#[derive(Debug)]
struct ScanGate {
    active: AtomicUsize,
    limit: usize,
}

impl ScanGate {
    const fn new(limit: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            limit,
        }
    }

    /// Takes a slot, or `None` when all `limit` slots are in use.
    fn try_enter(self: &Arc<Self>) -> Option<ScanSlot> {
        let mut current = self.active.load(Ordering::Acquire);
        loop {
            if current >= self.limit {
                return None;
            }
            match self.active.compare_exchange_weak(
                current,
                current.saturating_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(ScanSlot {
                        gate: Arc::clone(self),
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }
}

/// One slot of a [`ScanGate`]; releasing it on drop makes the slot available again.
#[derive(Debug)]
struct ScanSlot {
    gate: Arc<ScanGate>,
}

impl Drop for ScanSlot {
    fn drop(&mut self) {
        self.gate.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Runs `work` on a plain thread inside one slot of `gate` and awaits its result.
///
/// The slot is released before the result is sent, so a caller that has its result can start
/// another run at once without seeing its own finished run still counted.
async fn run_gated<T, F>(gate: &Arc<ScanGate>, work: F) -> Result<T, ApplicationError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let slot = gate
        .try_enter()
        .ok_or_else(|| ApplicationError::of(ErrorCode::UpstreamUnavailable))?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let result = work();
        // The slot is held until the work ends, even when the caller has gone away.
        drop(slot);
        // A closed receiver means the caller went away; the result is then not needed.
        let _ = sender.send(result);
    });
    receiver
        .await
        .map_err(|_| ApplicationError::of(ErrorCode::InternalError))
}

impl ArbeitnowServices {
    /// Scan the board. The scan is synchronous and its transport refuses to run inside a Tokio
    /// runtime, so it runs on a plain thread and the result is awaited.
    ///
    /// # Errors
    ///
    /// The scan service errors unchanged; a panicked scan thread is an internal error. When
    /// [`MAX_CONCURRENT_SCANS`] scans are already running the call is refused at once as
    /// upstream unavailable, without starting a thread.
    pub async fn scan(&self, request: ScanRequest) -> Result<ScanResult, ApplicationError> {
        let service = Arc::clone(&self.scan);
        run_gated(&self.gate, move || service.scan(&request)).await?
    }

    /// Import the candidate named by `request` without refetching it.
    ///
    /// # Errors
    ///
    /// The import service errors unchanged.
    pub async fn import(&self, request: ImportRequest) -> Result<ImportOutcome, ApplicationError> {
        self.import.import(request).await
    }
}

/// The Arbeitnow board over the sandboxed `fetch-arbeitnow` child.
///
/// Without a supervisor every fetch is a transport failure, which is what a failed child start
/// reports. It must be called from a plain thread (see [`ArbeitnowServices::scan`]).
#[derive(Debug, Clone)]
pub struct WorkerArbeitnowBoard {
    transport: Option<WorkerBoardTransport>,
}

impl WorkerArbeitnowBoard {
    /// A board over `supervisor`; `None` fails every fetch closed.
    #[must_use]
    pub fn new(supervisor: Option<Arc<WorkerSupervisor>>) -> Self {
        Self {
            transport: supervisor.map(WorkerBoardTransport::new),
        }
    }
}

impl ArbeitnowBoard for WorkerArbeitnowBoard {
    fn fetch(&self, page: u32, visa_sponsorship: Option<bool>) -> Result<Page, BoardFailure> {
        let transport = self.transport.as_ref().ok_or(BoardFailure::Transport)?;
        let result = transport.fetch(i64::from(page), visa_sponsorship);
        let now = system_instant().ok_or(BoardFailure::Transport)?;
        admit_board_result(result, page, visa_sponsorship, now)
    }
}

/// The wall clock as a nanosecond instant; `None` for a clock before the epoch or out of range.
fn system_instant() -> Option<ArbeitnowInstant> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    ArbeitnowInstant::new(
        i64::try_from(elapsed.as_secs()).ok()?,
        elapsed.subsec_nanos(),
    )
    .ok()
}

/// Whether the platform admits the owned child transports (`linux`, `darwin` and `win32`).
const fn platform_supported() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    ))
}

/// Composes the use cases from validated keys, one unit of work and the worker supervisor.
///
/// # Errors
///
/// [`OptInConfigError`] when a key is refused by the codec or the cursor authority.
pub fn compose_arbeitnow_services(
    config: &ArbeitnowConfig,
    uow: SqliteUnitOfWork,
    supervisor: Option<Arc<WorkerSupervisor>>,
) -> Result<ArbeitnowServices, OptInConfigError> {
    let clock = Arc::new(SystemClock);
    let scan_codec =
        CandidateCodec::new(config.candidate_key(), clock.clone()).map_err(|_| OptInConfigError)?;
    let import_codec =
        CandidateCodec::new(config.candidate_key(), clock.clone()).map_err(|_| OptInConfigError)?;
    let cursor_key = CursorKey::new(config.cursor_key()).map_err(|_| OptInConfigError)?;
    let scan = ArbeitnowScanService::new(
        WorkerArbeitnowBoard::new(supervisor),
        cursor_key,
        scan_codec,
    );
    let import = ArbeitnowJobImportService::new(
        import_codec,
        SqliteArbeitnowImportRepository::new(uow),
        clock,
    );
    Ok(ArbeitnowServices {
        scan: Arc::new(scan),
        import: Arc::new(import),
        gate: Arc::new(ScanGate::new(MAX_CONCURRENT_SCANS)),
    })
}

/// Composes the Arbeitnow use cases from the snapshot `env` and the database configuration.
///
/// `Ok(None)` unless the flag is exactly `1`. The platform gate and the key validation run before
/// the database check, so an enabled flag on an unsupported platform or with bad keys stops
/// startup; a valid enabled flag without a database configuration composes nothing. Composition
/// connects, spawns and fetches nothing.
///
/// # Errors
///
/// [`OptInConfigError`] for an unsupported platform or unusable keys.
pub fn compose_arbeitnow(
    env: &Environment,
    database: Option<&SqlitePool>,
) -> Result<Option<ArbeitnowServices>, OptInConfigError> {
    compose_arbeitnow_on(env, database, platform_supported())
}

/// [`compose_arbeitnow`] with the platform decision passed in, so the gate can be tested on every host.
fn compose_arbeitnow_on(
    env: &Environment,
    database: Option<&SqlitePool>,
    platform_supported: bool,
) -> Result<Option<ArbeitnowServices>, OptInConfigError> {
    if !env.flag_enabled(FLAG_KEY) {
        return Ok(None);
    }
    if !platform_supported {
        return Err(OptInConfigError);
    }
    let Some(config) = load_arbeitnow_config(env)? else {
        return Ok(None);
    };
    let Some(database) = database else {
        return Ok(None);
    };
    let uow = SqliteUnitOfWork::new(database.clone());
    let supervisor = crate::diagnostics::current_process_supervisor();
    compose_arbeitnow_services(&config, uow, supervisor).map(Some)
}

/// The cursor secret: ASCII, 32 to 1024 bytes, and unchanged by trimming ASCII whitespace.
fn is_valid_cursor_secret(secret: &str) -> bool {
    secret.is_ascii()
        && (MIN_SECRET_LEN..=MAX_SECRET_LEN).contains(&secret.len())
        && secret.trim_matches(is_ascii_space).len() == secret.len()
}

/// The ASCII whitespace characters: tab to carriage return, 0x1C to 0x1F, and space.
fn is_ascii_space(c: char) -> bool {
    matches!(c, '\t'..='\r' | '\u{1c}'..='\u{1f}' | ' ')
}

/// The data part of the candidate text: 1 to 1024 base64url characters, then at most two `=`.
fn base64url_data(text: &str) -> Option<&str> {
    let data = text.trim_end_matches('=');
    let padding = text.len() - data.len();
    let valid = (1..=MAX_ENCODED_CHARS).contains(&data.len())
        && padding <= 2
        && data.bytes().all(|byte| sextet(byte).is_some());
    valid.then_some(data)
}

/// The 6-bit value of one base64url character (RFC 4648 section 5), or `None`.
fn sextet(byte: u8) -> Option<u32> {
    let value = match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'-' => 62,
        b'_' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

/// Decode unpadded base64url leniently: unused bits of the final group are ignored, and a single
/// leftover character is an error.
fn decode_base64url(data: &str) -> Option<Vec<u8>> {
    if data.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for byte in data.bytes() {
        accumulator = (accumulator << 6) | sextet(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((accumulator >> bits) & 0xFF) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The number of distinct byte values.
fn distinct_bytes(bytes: &[u8]) -> usize {
    let mut seen = [false; 256];
    for &byte in bytes {
        seen[usize::from(byte)] = true;
    }
    seen.iter().filter(|present| **present).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURSOR_SECRET: &str = "cursor-secret-0123456789abcdefghijklmnop";
    /// The bytes 0 to 31 in unpadded base64url.
    const KEY32: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
    /// `KEY32` with its last character changed in the two unused bits; it decodes to the same
    /// bytes.
    const KEY32_UNUSED_BITS: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh9";
    /// The bytes 0 to 33 with the two-`=` padding.
    const KEY34_PADDED: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gIQ==";
    /// Forty-four characters, all in the base64url alphabet.
    const ALPHABET_SECRET: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGH";

    #[tokio::test]
    async fn a_gated_run_is_refused_when_full_and_releases_before_its_result() {
        let gate = Arc::new(ScanGate::new(1));
        let held = gate.try_enter().expect("slot");
        let refused = run_gated(&gate, || 1_u8).await;
        assert_eq!(
            refused.expect_err("full gate").code(),
            ErrorCode::UpstreamUnavailable
        );
        drop(held);
        for _ in 0..200 {
            // Each result is back only after its slot was released, so the next run is admitted.
            assert_eq!(run_gated(&gate, || 7_u8).await.expect("admitted"), 7);
        }
        let panicked = run_gated(&gate, || -> u8 { panic!("scan thread panic") }).await;
        assert_eq!(
            panicked.expect_err("panic").code(),
            ErrorCode::InternalError
        );
        assert!(gate.try_enter().is_some(), "a panicking run frees its slot");
    }

    #[test]
    fn the_scan_gate_refuses_beyond_its_limit_and_frees_a_slot_on_drop() {
        let gate = Arc::new(ScanGate::new(MAX_CONCURRENT_SCANS));
        let held: Vec<ScanSlot> = (0..MAX_CONCURRENT_SCANS)
            .map(|_| gate.try_enter().expect("slot within the limit"))
            .collect();
        assert!(gate.try_enter().is_none(), "a full gate refuses at once");
        drop(held);
        assert!(gate.try_enter().is_some(), "released slots are reusable");
    }

    fn env(pairs: &[(&str, &str)]) -> Environment {
        Environment::from_pairs(pairs.iter().copied())
    }

    /// An environment with the flag set and the given key settings.
    fn enabled(candidate: &str, secret: &str) -> Environment {
        env(&[
            (FLAG_KEY, "1"),
            (CANDIDATE_KEY, candidate),
            (CURSOR_SECRET_KEY, secret),
        ])
    }

    fn is_rejected(env: &Environment) -> bool {
        matches!(load_arbeitnow_config(env), Err(OptInConfigError))
    }

    /// Unpadded base64url encoder for building fixtures from raw bytes.
    fn encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let first = chunk.first().copied().unwrap_or(0);
            let second = chunk.get(1).copied().unwrap_or(0);
            let third = chunk.get(2).copied().unwrap_or(0);
            let values = [
                first >> 2,
                ((first & 0x03) << 4) | (second >> 4),
                ((second & 0x0F) << 2) | (third >> 6),
                third & 0x3F,
            ];
            for value in values.iter().take(chunk.len() + 1) {
                out.push(char::from(ALPHABET[usize::from(*value)]));
            }
        }
        out
    }

    /// A cyclic secret of `len` characters drawn from a 36-character alphabet.
    fn secret_of_len(len: usize) -> String {
        const SYMBOLS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        (0..len)
            .map(|index| char::from(SYMBOLS[index % SYMBOLS.len()]))
            .collect()
    }

    fn bytes_of_len(len: usize, modulus: usize) -> Vec<u8> {
        (0..len).map(|index| (index % modulus) as u8).collect()
    }

    #[test]
    fn only_the_exact_flag_value_one_enables_the_feature() -> Result<(), OptInConfigError> {
        let garbage = [(CANDIDATE_KEY, "!!!"), (CURSOR_SECRET_KEY, "short")];
        assert!(load_arbeitnow_config(&env(&garbage))?.is_none());
        for flag in ["", "0", "true", "yes", " 1", "1 ", "01", "11", "1.0"] {
            let mut pairs = garbage.to_vec();
            pairs.push((FLAG_KEY, flag));
            assert!(
                load_arbeitnow_config(&env(&pairs))?.is_none(),
                "flag {flag:?} must not enable the feature"
            );
        }
        let invalid_flag = env(&garbage).with_invalid(FLAG_KEY);
        assert!(load_arbeitnow_config(&invalid_flag)?.is_none());
        Ok(())
    }

    #[test]
    fn valid_keys_are_decoded_and_exposed() -> Result<(), OptInConfigError> {
        let config =
            load_arbeitnow_config(&enabled(KEY32, CURSOR_SECRET))?.ok_or(OptInConfigError)?;
        assert_eq!(config.candidate_key(), bytes_of_len(32, 256).as_slice());
        assert_eq!(config.cursor_key(), CURSOR_SECRET.as_bytes());
        Ok(())
    }

    #[test]
    fn padded_candidate_text_is_accepted() {
        let padded_once = format!("{KEY32}=");
        assert!(load_arbeitnow_config(&enabled(&padded_once, CURSOR_SECRET)).is_ok());
        assert!(load_arbeitnow_config(&enabled(KEY34_PADDED, CURSOR_SECRET)).is_ok());
    }

    #[test]
    fn unused_trailing_bits_are_ignored() -> Result<(), OptInConfigError> {
        let config = load_arbeitnow_config(&enabled(KEY32_UNUSED_BITS, CURSOR_SECRET))?
            .ok_or(OptInConfigError)?;
        assert_eq!(config.candidate_key(), bytes_of_len(32, 256).as_slice());
        Ok(())
    }

    #[test]
    fn malformed_candidate_text_is_rejected() {
        let plus = format!("{}+{}", &KEY32[..10], &KEY32[11..]);
        let slash = format!("{}/{}", &KEY32[..10], &KEY32[11..]);
        let inner_pad = format!("{}={}", &KEY32[..10], &KEY32[10..]);
        let cases = [
            String::new(),
            "=".to_owned(),
            format!("{KEY32}==="),
            format!("{KEY32}===="),
            inner_pad,
            plus,
            slash,
            format!("{KEY32}."),
            format!(" {KEY32}"),
            format!("{KEY32} "),
            format!("{KEY32}\n"),
            format!("{KEY32}\u{e9}"),
            format!("{KEY32}AA"),
            "A".repeat(1025),
        ];
        for candidate in cases {
            assert!(
                is_rejected(&enabled(&candidate, CURSOR_SECRET)),
                "candidate {candidate:?} must be rejected"
            );
        }
    }

    #[test]
    fn candidate_text_at_the_length_limit_is_accepted() -> Result<(), OptInConfigError> {
        let bytes = bytes_of_len(768, 256);
        let text = encode(&bytes);
        assert_eq!(text.len(), 1024);
        let config =
            load_arbeitnow_config(&enabled(&text, CURSOR_SECRET))?.ok_or(OptInConfigError)?;
        assert_eq!(config.candidate_key(), bytes.as_slice());
        Ok(())
    }

    #[test]
    fn cursor_secret_length_is_32_to_1024_bytes() {
        assert!(is_rejected(&enabled(KEY32, &secret_of_len(31))));
        assert!(load_arbeitnow_config(&enabled(KEY32, &secret_of_len(32))).is_ok());
        assert!(load_arbeitnow_config(&enabled(KEY32, &secret_of_len(1024))).is_ok());
        assert!(is_rejected(&enabled(KEY32, &secret_of_len(1025))));
    }

    #[test]
    fn cursor_secret_must_be_ascii_and_unchanged_by_strip() {
        let body = secret_of_len(40);
        let non_ascii = format!("\u{e9}{}", secret_of_len(40));
        assert!(is_rejected(&enabled(KEY32, &non_ascii)));
        let edges = [
            format!(" {body}"),
            format!("{body} "),
            format!("\t{body}"),
            format!("{body}\u{0b}"),
            format!("{body}\u{0c}"),
            format!("\r\n{body}"),
            format!("\u{1c}{body}"),
            format!("{body}\u{1d}"),
            format!("{body}\u{1e}"),
            format!("{body}\u{1f}"),
        ];
        for secret in edges {
            assert!(
                is_rejected(&enabled(KEY32, &secret)),
                "secret {secret:?} must be rejected"
            );
        }
    }

    #[test]
    fn cursor_secret_may_contain_inner_whitespace_and_control_bytes() {
        let inner = format!("cursor secret\u{01}{}", secret_of_len(40));
        assert!(load_arbeitnow_config(&enabled(KEY32, &inner)).is_ok());
    }

    #[test]
    fn cursor_secret_needs_sixteen_distinct_bytes() {
        let fifteen = "abcdefghijklmno".repeat(3);
        let sixteen = "abcdefghijklmnop".repeat(3);
        assert!(is_rejected(&enabled(KEY32, &fifteen)));
        assert!(load_arbeitnow_config(&enabled(KEY32, &sixteen)).is_ok());
    }

    #[test]
    fn candidate_key_must_decode_to_at_least_32_bytes() {
        let thirty_one = encode(&bytes_of_len(31, 256));
        assert!(is_rejected(&enabled(&thirty_one, CURSOR_SECRET)));
        let thirty_three = encode(&bytes_of_len(33, 256));
        assert!(load_arbeitnow_config(&enabled(&thirty_three, CURSOR_SECRET)).is_ok());
    }

    #[test]
    fn candidate_key_needs_sixteen_distinct_bytes() {
        let fifteen = encode(&bytes_of_len(32, 15));
        let sixteen = encode(&bytes_of_len(32, 16));
        assert!(is_rejected(&enabled(&fifteen, CURSOR_SECRET)));
        assert!(load_arbeitnow_config(&enabled(&sixteen, CURSOR_SECRET)).is_ok());
    }

    #[test]
    fn candidate_key_must_differ_from_the_cursor_bytes() {
        let decoded_equal = encode(CURSOR_SECRET.as_bytes());
        assert!(is_rejected(&enabled(&decoded_equal, CURSOR_SECRET)));
    }

    #[test]
    fn candidate_text_must_differ_from_the_cursor_secret() {
        assert!(is_rejected(&enabled(ALPHABET_SECRET, ALPHABET_SECRET)));
        let padded = format!("{ALPHABET_SECRET}==");
        assert!(is_rejected(&enabled(&padded, ALPHABET_SECRET)));
    }

    #[test]
    fn a_single_leftover_candidate_character_is_rejected() {
        let one_extra = format!("{KEY32}AA");
        assert!(is_rejected(&enabled(&one_extra, CURSOR_SECRET)));
    }

    #[test]
    fn missing_or_non_unicode_settings_fail_closed_when_enabled() {
        let flag_only = env(&[(FLAG_KEY, "1")]);
        assert!(is_rejected(&flag_only));
        let no_secret = env(&[(FLAG_KEY, "1"), (CANDIDATE_KEY, KEY32)]);
        assert!(is_rejected(&no_secret));
        let no_candidate = env(&[(FLAG_KEY, "1"), (CURSOR_SECRET_KEY, CURSOR_SECRET)]);
        assert!(is_rejected(&no_candidate));
        let bad_candidate = enabled(KEY32, CURSOR_SECRET).with_invalid(CANDIDATE_KEY);
        assert!(is_rejected(&bad_candidate));
        let bad_secret = enabled(KEY32, CURSOR_SECRET).with_invalid(CURSOR_SECRET_KEY);
        assert!(is_rejected(&bad_secret));
    }

    #[test]
    fn decoder_accepts_partial_groups() {
        assert_eq!(decode_base64url("QR"), Some(b"A".to_vec()));
        assert_eq!(decode_base64url("QUI"), Some(b"AB".to_vec()));
        assert_eq!(decode_base64url("QUJD"), Some(b"ABC".to_vec()));
        assert_eq!(decode_base64url("----"), Some(vec![0xfb, 0xef, 0xbe]));
        assert_eq!(decode_base64url("Q"), None);
        assert_eq!(decode_base64url("QUJDQ"), None);
    }

    /// A pool over a fresh temporary database file. Opening creates the file and the writer
    /// connection only.
    fn pool() -> Result<(tempfile::TempDir, SqlitePool), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let pool = SqlitePool::open(root.path().join("jobniac.sqlite3"))?;
        Ok((root, pool))
    }

    fn composed(pairs: &[(&str, &str)]) -> Result<Option<ArbeitnowServices>, OptInConfigError> {
        compose_arbeitnow(&env(pairs), None)
    }

    fn composed_with_database(
        pairs: &[(&str, &str)],
    ) -> Result<Option<ArbeitnowServices>, Box<dyn std::error::Error>> {
        let (_root, pool) = pool()?;
        Ok(compose_arbeitnow(&env(pairs), Some(&pool))?)
    }

    #[test]
    fn composition_requires_the_exact_flag() {
        for flag in [
            None,
            Some(""),
            Some("0"),
            Some("true"),
            Some("01"),
            Some(" 1"),
        ] {
            let mut extra = vec![(CANDIDATE_KEY, KEY32), (CURSOR_SECRET_KEY, CURSOR_SECRET)];
            if let Some(flag) = flag {
                extra.push((FLAG_KEY, flag));
            }
            assert!(
                matches!(composed_with_database(&extra), Ok(None)),
                "flag {flag:?} must compose nothing"
            );
        }
    }

    #[test]
    fn an_enabled_flag_with_a_missing_key_fails_with_the_fixed_error() {
        let pairs = [(FLAG_KEY, "1"), (CURSOR_SECRET_KEY, CURSOR_SECRET)];
        let Err(error) = composed(&pairs) else {
            panic!("a missing candidate key must stop composition");
        };
        let text = format!("{error} {error:?}");
        assert_eq!(
            error.to_string(),
            "Optional feature configuration is invalid"
        );
        assert!(!text.contains(CURSOR_SECRET));
    }

    #[test]
    fn valid_keys_with_a_database_compose_both_use_cases() -> Result<(), Box<dyn std::error::Error>>
    {
        let pairs = [
            (FLAG_KEY, "1"),
            (CANDIDATE_KEY, KEY32),
            (CURSOR_SECRET_KEY, CURSOR_SECRET),
        ];
        let services = composed_with_database(&pairs)?;
        if cfg!(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        )) {
            assert!(services.is_some());
        } else {
            assert!(services.is_none());
        }
        Ok(())
    }

    #[test]
    fn valid_keys_without_a_database_compose_nothing() -> Result<(), OptInConfigError> {
        let pairs = [
            (FLAG_KEY, "1"),
            (CANDIDATE_KEY, KEY32),
            (CURSOR_SECRET_KEY, CURSOR_SECRET),
        ];
        if cfg!(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        )) {
            assert!(composed(&pairs)?.is_none());
        } else {
            assert!(composed(&pairs).is_err());
        }
        Ok(())
    }

    #[test]
    fn a_board_without_a_supervisor_fails_closed() {
        let board = WorkerArbeitnowBoard::new(None);
        assert_eq!(board.fetch(1, None).err(), Some(BoardFailure::Transport));
    }

    #[test]
    fn enabled_on_an_unsupported_platform_fails_closed() {
        let env = enabled(KEY32, CURSOR_SECRET);
        assert!(matches!(
            compose_arbeitnow_on(&env, None, false),
            Err(OptInConfigError)
        ));
        // The flag decides first: any other value composes nothing, on any platform.
        let off = env_with_flag("0");
        assert!(matches!(compose_arbeitnow_on(&off, None, false), Ok(None)));
        assert!(matches!(compose_arbeitnow_on(&off, None, true), Ok(None)));
    }

    /// The same keys with the given flag value.
    fn env_with_flag(flag: &str) -> Environment {
        env(&[
            (FLAG_KEY, flag),
            (CANDIDATE_KEY, KEY32),
            (CURSOR_SECRET_KEY, CURSOR_SECRET),
        ])
    }

    #[test]
    fn the_platform_gate_admits_linux_macos_and_windows() {
        // Other platforms are refused at startup when the flag is 1.
        assert_eq!(
            platform_supported(),
            cfg!(any(
                target_os = "linux",
                target_os = "macos",
                target_os = "windows"
            ))
        );
    }

    #[test]
    fn enabled_composition_opens_no_further_connection() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, pool) = pool()?;
        let env = env_with_flag("1");
        let composed = compose_arbeitnow_on(&env, Some(&pool), true)?;
        assert!(
            composed.is_some(),
            "a valid enabled flag composes the services"
        );
        assert_eq!(
            pool.idle_readers(),
            0,
            "composition must not open a reader connection"
        );
        Ok(())
    }

    #[test]
    fn error_message_is_fixed() {
        assert_eq!(
            OptInConfigError.to_string(),
            "Optional feature configuration is invalid"
        );
    }

    #[test]
    fn debug_output_redacts_both_keys() -> Result<(), OptInConfigError> {
        let config =
            load_arbeitnow_config(&enabled(KEY32, CURSOR_SECRET))?.ok_or(OptInConfigError)?;
        let rendered = format!("{config:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("cursor-secret"));
        assert!(!rendered.contains(KEY32));
        Ok(())
    }
}
