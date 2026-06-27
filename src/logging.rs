use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

const DEFAULT_STDOUT: &str = "http402_forge_api=info,tower_http=warn";
const DEFAULT_FILE: &str = "http402_forge_api=trace,tower_http=warn";
const DEFAULT_LOG_DIR: &str = "/app/logs";

/// Keeps the non-blocking file writer alive for the process lifetime.
pub struct LogGuard {
    _file: Option<tracing_appender::non_blocking::WorkerGuard>,
}

pub fn init() -> LogGuard {
    let rust_log_set = std::env::var("RUST_LOG")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    let stdout_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_filter(env_filter(DEFAULT_STDOUT));

    let log_dir = log_file_dir();
    let guard = if let Some(ref dir) = log_dir {
        match open_file_sink(dir) {
            Some((writer, file_guard)) => {
                let file_filter = if rust_log_set {
                    env_filter(DEFAULT_STDOUT)
                } else {
                    env_filter(DEFAULT_FILE)
                };
                let file_layer = tracing_subscriber::fmt::layer()
                    .compact()
                    .with_writer(writer)
                    .with_filter(file_filter);
                tracing_subscriber::registry()
                    .with(stdout_layer)
                    .with(file_layer)
                    .init();
                Some(file_guard)
            }
            None => {
                eprintln!(
                    "http402-forge-api: LOG_FILE_DIR={dir} unavailable; logging to stdout only"
                );
                tracing_subscriber::registry().with(stdout_layer).init();
                None
            }
        }
    } else {
        tracing_subscriber::registry().with(stdout_layer).init();
        None
    };

    info!(
        log_file_dir = log_dir.as_deref(),
        rust_log = std::env::var("RUST_LOG")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .as_deref(),
        "logging initialized"
    );

    LogGuard { _file: guard }
}

fn log_file_dir() -> Option<String> {
    match std::env::var("LOG_FILE_DIR") {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Err(_) => {
            // Docker systemd units mount host logs here; skip file sink locally when absent.
            if std::path::Path::new(DEFAULT_LOG_DIR).is_dir() {
                Some(DEFAULT_LOG_DIR.to_string())
            } else {
                None
            }
        }
    }
}

fn env_filter(default: &str) -> EnvFilter {
    EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(default))
        .unwrap_or_else(|_| EnvFilter::new("info"))
}

fn open_file_sink(
    dir: &str,
) -> Option<(
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
)> {
    std::fs::create_dir_all(dir).ok()?;
    let appender = tracing_appender::rolling::daily(dir, "forge.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    Some((writer, guard))
}
