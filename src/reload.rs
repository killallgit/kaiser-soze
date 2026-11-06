use crate::cli;
use crate::config::Settings as AppSettings;
use kaiser_soze::logging::{Fields, Level, Logger};
use std::sync::Arc;
use tokio::signal::unix::Signal;

fn failed(log: &Logger, error_kind: &'static str) {
    let fields = Fields {
        error_kind: Some(error_kind),
        ..Fields::default()
    };
    log.event(Level::Error, "config_reload_failed", fields);
}

/// The new level applies first, so the notices that follow obey it.
fn applied(log: &Logger, running: &mut AppSettings, candidate: AppSettings) {
    let before = running.logging();
    let restart = running.reload_into(candidate);
    if running.logging() != before {
        log.reload(running.logging());
        log.event(Level::Info, "config_reloaded", Fields::default());
    }
    for field in restart {
        log.event(
            Level::Warn,
            "config_restart_required",
            Fields {
                field: Some(field),
                ..Fields::default()
            },
        );
    }
}

/// A `SIGHUP` re-reads the files under the flags and environment the run started with. A bad file
/// is reported and changes nothing; the listener and the tap never wait on it.
async fn on_hangup(mut hangup: Signal, reload: Arc<cli::Reload>, mut running: AppSettings, log: Logger) {
    while hangup.recv().await.is_some() {
        let reload = reload.clone();
        match tokio::task::spawn_blocking(move || reload.run()).await {
            Ok(Ok(candidate)) => applied(&log, &mut running, candidate),
            Ok(Err(error)) => failed(&log, error.kind()),
            Err(_) => failed(&log, "reload_panicked"),
        }
    }
}

/// Starts the reload task, or reports that the signal could not be installed. `None` means a terminal
/// keeps the default `SIGHUP`, so nothing starts.
pub fn watch(hangup: Option<std::io::Result<Signal>>, reload: Arc<cli::Reload>, running: AppSettings, log: Logger) {
    match hangup {
        Some(Ok(hangup)) => {
            tokio::spawn(on_hangup(hangup, reload, running, log));
        }
        Some(Err(error)) => {
            log.event(Level::Warn, "reload_unavailable", Fields::io(&error));
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use kaiser_soze::logging::{Format, Settings};

    #[tokio::test]
    async fn a_failed_reload_is_reported_at_the_error_level() {
        let (writer, mut reader) = tokio::io::duplex(8192);
        let log = Logger::new(
            writer,
            Settings {
                level: Level::Error,
                format: Format::Json,
                ..Settings::default()
            },
        );

        failed(&log, "invalid_configuration");
        drop(log);

        let mut text = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut reader, &mut text)
            .await
            .unwrap();
        let record: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(record["event"], "config_reload_failed");
        assert_eq!(record["error_kind"], "invalid_configuration");
    }

    #[tokio::test]
    async fn restart_notices_follow_the_level_the_reload_just_applied() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let write = |text: &str| std::fs::write(&config, format!("tap_file = 'synthetic.jsonl'\n{text}")).unwrap();
        write("port = 1000\nlog_level = 'error'\n");
        let (mut running, reload) = cli::parse_with_reload(
            ["--config".to_string(), config.display().to_string()],
            config::Sources::new(Format::Json),
        )
        .ok()
        .unwrap();
        let (writer, mut reader) = tokio::io::duplex(8192);
        let log = Logger::new(writer, running.logging());

        write("port = 2000\nlog_level = 'warn'\n");
        applied(&log, &mut running, reload.run().ok().unwrap());
        drop(log);

        let mut text = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut reader, &mut text)
            .await
            .unwrap();
        let record: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(record["event"], "config_restart_required");
        assert_eq!(record["field"], "port");
    }
}
