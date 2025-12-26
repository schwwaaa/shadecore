//! Centralized timestamped logging
//!
//! All logs should go through `logi!`, `logw!`, or `loge!` so they include:
//!   <timestamp> [TAG][thread] message
//!
//! This is intentionally lightweight and dependency-minimal.

// NOTE: We use the `time` crate purely for formatting timestamps with millisecond precision.
// Local time is used when available; it falls back to UTC.
pub(crate) fn log_timestamp() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let fmt = time::format_description::parse(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"
    ).expect("valid time format description");
    now.format(&fmt).unwrap_or_else(|_| "<time-format-error>".to_string())
}

pub(crate) fn log_thread_name() -> String {
    std::thread::current().name().unwrap_or("thread").to_string()
}

/// Info log: printed to stdout
#[macro_export]
macro_rules! logi {
    ($tag:expr, $($arg:tt)*) => {{
        println!("{} [{}][{}] {}", $crate::logging::log_timestamp(), $tag, $crate::logging::log_thread_name(), format!($($arg)*));
        ()
    }};
}

/// Warning log: printed to stderr
#[macro_export]
macro_rules! logw {
    ($tag:expr, $($arg:tt)*) => {{
        eprintln!("{} [{}][{}] {}", $crate::logging::log_timestamp(), $tag, $crate::logging::log_thread_name(), format!($($arg)*));
        ()
    }};
}

/// Error log: printed to stderr
#[macro_export]
macro_rules! loge {
    ($tag:expr, $($arg:tt)*) => {{
        eprintln!("{} [{}][{}] {}", $crate::logging::log_timestamp(), $tag, $crate::logging::log_thread_name(), format!($($arg)*));
        ()
    }};
}
