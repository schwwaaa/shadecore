//! ShadeCore logging utilities.
//!
//! Design goals
//! - Every ShadeCore log line is shaped like:
//!     <timestamp> [TAG][thread] message
//! - Works on all platforms with std only (no extra deps).
//! - Optional file sink for audit/debug.
//! - Optional piping of child-process stdout/stderr into the same log format.
//!
//! NOTE: Some platform/framework messages (e.g. macOS IMK) bypass this logger and may still
//! appear unformatted; those are emitted by the OS/framework itself.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static LOG_FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();
static RUN_ID: OnceLock<String> = OnceLock::new();
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Initialize logging. Call once at startup.
/// - If `log_file` is Some, we append all log lines to that path.
/// - Always logs to stderr as the primary sink.
///
/// Returns the generated run_id.
pub fn init(log_file: Option<PathBuf>) -> String {
    let rid = RUN_ID
        .get_or_init(|| {
            // Short correlation id: time xor pid (good enough for debugging/audit grouping)
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
            format!("{:08x}", (now.as_nanos() as u64) ^ (std::process::id() as u64))
        })
        .clone();

    let _ = LOG_FILE.get_or_init(|| Mutex::new(None));

    if let Some(path) = log_file {
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => {
                if let Some(m) = LOG_FILE.get() {
                    *m.lock().unwrap() = Some(f);
                }
            }
            Err(_) => {
                // Can't call log* macros here (they depend on log_line), so emit directly.
                eprintln!(
                    "{} [WARN][{}] failed to open log file sink",
                    log_timestamp(),
                    log_thread_name()
                );
            }
        }
    }

    rid
}

/// Current run id (empty if init() wasn't called).
pub fn run_id() -> &'static str {
    RUN_ID.get().map(|s| s.as_str()).unwrap_or("")
}

/// Make a short session id for correlating operations (e.g. recording sessions).
pub fn make_session_id(prefix: &str) -> String {
    let n = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{}_{}", compact_utc_timestamp(), format!("{:04}", n))
}

/// Pipe a Read stream (child stdout/stderr) into the logger on its own thread.
pub fn spawn_pipe_thread<R: Read + Send + 'static>(
    thread_name: &str,
    tag: &str,
    reader: R,
    as_warn: bool,
) {
    let tag = tag.to_string();
    let tname = thread_name.to_string();
    let _ = std::thread::Builder::new()
        .name(tname)
        .spawn(move || {
            let br = BufReader::new(reader);
            for line in br.lines().flatten() {
                if as_warn {
                    log_line("WARN", &tag, &line);
                } else {
                    log_line("INFO", &tag, &line);
                }
            }
        });
}

/// Timestamp used in logs: `YYYY-MM-DD HH:MM:SS.mmm` (UTC).
pub fn log_timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as i64;
    let ms = now.subsec_millis() as i64;

    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);

    let (year, month, day) = civil_from_days(days);
    let hour = (sod / 3600) as i64;
    let min = ((sod % 3600) / 60) as i64;
    let sec = (sod % 60) as i64;

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        year, month, day, hour, min, sec, ms
    )
}

/// Best-effort thread name for log prefix.
pub fn log_thread_name() -> String {
    std::thread::current().name().unwrap_or("main").to_string()
}

/// Write one fully formatted line to stderr + optional file sink.
///
/// This must be visible to the macros (crate scope).
pub(crate) fn log_line(_level: &str, tag: &str, msg: &str) {
    let line = format!("{} [{}][{}] {}", log_timestamp(), tag, log_thread_name(), msg);

    // stderr is the canonical sink
    eprintln!("{line}");

    // optional file sink
    if let Some(m) = LOG_FILE.get() {
        if let Ok(mut guard) = m.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = writeln!(f, "{line}");
                let _ = f.flush();
            }
        }
    }
}

// Compact timestamp for ids: `YYYYMMDDThhmmssZ` (UTC).
fn compact_utc_timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as i64;

    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);

    let (year, month, day) = civil_from_days(days);
    let hour = (sod / 3600) as i64;
    let min = ((sod % 3600) / 60) as i64;
    let sec = (sod % 60) as i64;

    format!("{:04}{:02}{:02}T{:02}{:02}{:02}Z", year, month, day, hour, min, sec)
}

// Howard Hinnant civil_from_days algorithm (reimplemented).
// Converts days since Unix epoch (1970-01-01) to Gregorian Y-M-D in UTC.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }).div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096).div_euclid(365);
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2).div_euclid(153);
    let d = doy - (153 * mp + 2).div_euclid(5) + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m, d)
}

#[macro_export]
macro_rules! logi {
    ($tag:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::logging::log_line("INFO", $tag, &msg);
    }};
}

#[macro_export]
macro_rules! logw {
    ($tag:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::logging::log_line("WARN", $tag, &msg);
    }};
}

#[macro_export]
macro_rules! loge {
    ($tag:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::logging::log_line("ERROR", $tag, &msg);
    }};
}
