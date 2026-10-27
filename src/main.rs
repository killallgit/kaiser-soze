use kaiser_soze::claude::ClaudeAdapter;
use kaiser_soze::codex::CodexAdapter;
use kaiser_soze::logging::{Fields, Format, Level, Logger, Settings};
use kaiser_soze::proxy::{Proxy, ProxyConfig};
use std::io::IsTerminal;
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

struct Args {
    port: u16,
    tap_file: PathBuf,
    shape_file: Option<PathBuf>,
    log_file: Option<PathBuf>,
    logging: Settings,
}

fn parse_args_from(
    args: impl IntoIterator<Item = String>,
    mut level: Option<Level>,
    format: Format,
) -> Result<Args, &'static str> {
    let mut port = 0;
    let mut tap_file = None;
    let mut shape_file = None;
    let mut log_file = None;
    let mut logging = Settings {
        format,
        ..Settings::default()
    };
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--live" => {
                logging.live = true;
                continue;
            }
            "--stream" => {
                logging.stream = true;
                continue;
            }
            _ => {}
        }
        let value = args.next().ok_or("missing_flag_value")?;
        match flag.as_str() {
            "--port" => port = value.parse().map_err(|_| "invalid_port")?,
            "--tap-file" => tap_file = Some(PathBuf::from(value)),
            "--shape-file" => shape_file = Some(PathBuf::from(value)),
            "--log-file" => log_file = Some(PathBuf::from(value)),
            "--log-level" => level = Some(value.parse()?),
            "--log-format" => logging.format = value.parse()?,
            _ => return Err("unknown_flag"),
        }
    }
    logging.level = level.unwrap_or(if logging.stream { Level::Debug } else { Level::Info });
    let tap_file = tap_file.ok_or("missing_tap_file")?;
    if log_file
        .as_ref()
        .is_some_and(|log| same_file(log, &tap_file) || shape_file.as_ref().is_some_and(|shape| same_file(log, shape)))
    {
        return Err("debug_file_overlaps_capture");
    }
    Ok(Args {
        port,
        tap_file,
        shape_file,
        log_file,
        logging,
    })
}

fn same_file(left: &Path, right: &Path) -> bool {
    left == right
        || resolved(left)
            .zip(resolved(right))
            .is_some_and(|(left, right)| left == right)
        || std::fs::metadata(left)
            .ok()
            .zip(std::fs::metadata(right).ok())
            .is_some_and(|(left, right)| left.dev() == right.dev() && left.ino() == right.ino())
}

fn resolved(path: &Path) -> Option<PathBuf> {
    let mut path = path.to_path_buf();
    // Canonicalize existing paths and resolve final links even when their target is not created yet.
    for _ in 0..40 {
        if let Ok(resolved) = path.canonicalize() {
            return Some(resolved);
        }
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if let Ok(target) = path.read_link() {
            path = parent.join(target);
        } else {
            return Some(parent.canonicalize().ok()?.join(path.file_name()?));
        }
    }
    None
}

fn parse_args(format: Format) -> Result<Args, &'static str> {
    let level = std::env::var("SOZE_LOG").ok().map(|value| value.parse()).transpose()?;
    let format = std::env::var("SOZE_LOG_FORMAT")
        .ok()
        .map_or(Ok(format), |value| value.parse())?;
    parse_args_from(std::env::args().skip(1), level, format)
}

async fn run(args: Args, log: &mut Logger) -> Result<(), &'static str> {
    // Bind before resetting the debug file: a duplicate dev launch must not erase a live run's log.
    let listener = TcpListener::bind(("127.0.0.1", args.port)).await.map_err(|error| {
        log.event(Level::Error, "listener_bind_failed", Fields::io(&error));
        "listener_bind_failed"
    })?;
    if let Some(path) = &args.log_file {
        match log.clone().with_debug_file(path).await {
            Ok(file_log) => *log = file_log,
            Err(error) => {
                log.event(Level::Warn, "debug_file_open_failed", Fields::io(&error));
            }
        }
    }
    let ca_dir = PathBuf::from(std::env::var("HOME").map_err(|_| "home_unavailable")?).join(".config/soze");
    let proxy = Proxy::new(ProxyConfig {
        adapters: vec![Arc::new(CodexAdapter::default()), Arc::new(ClaudeAdapter::default())],
        ca_dir: ca_dir.clone(),
        tap_file: args.tap_file,
        shape_file: args.shape_file,
        upstream_roots: rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        },
        upstream_addr: None,
        live: None,
        logging: log.clone(),
    })
    .await
    .map_err(|_| "proxy_initialization_failed")?;
    let log = proxy.logger();
    let port = listener.local_addr().map_err(|_| "listener_address_failed")?.port();
    log.event(
        Level::Info,
        "listening",
        Fields {
            port: Some(port),
            ..Fields::default()
        },
    );
    log.event(
        Level::Info,
        "ca_ready",
        Fields {
            ca_path: Some(ca_dir.join("ca.pem").display().to_string()),
            ..Fields::default()
        },
    );
    proxy.serve(listener).await;
    Ok(())
}

fn main() -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread().enable_all().build() else {
        return ExitCode::FAILURE;
    };
    let exit = runtime.block_on(service());
    // A blocked stderr write uses Tokio's blocking pool; runtime drop would otherwise wait forever.
    runtime.shutdown_timeout(Duration::from_millis(250));
    exit
}

async fn service() -> ExitCode {
    let terminal = std::io::stderr().is_terminal();
    let default_format = if terminal { Format::Human } else { Format::Json };
    let parsed = parse_args(default_format);
    let settings = parsed.as_ref().map_or(
        Settings {
            format: default_format,
            ..Settings::default()
        },
        |args| args.logging,
    );
    let Ok(stderr) = std::io::stderr().as_fd().try_clone_to_owned() else {
        return ExitCode::FAILURE;
    };
    let mut log = Logger::new(tokio::fs::File::from_std(std::fs::File::from(stderr)), settings);
    let result = match parsed {
        Ok(args) => run(args, &mut log).await,
        Err(error) => Err(error),
    };
    if let Err(kind) = result {
        log.event(
            Level::Error,
            "service_start_failed",
            Fields {
                error_kind: Some(kind),
                ..Fields::default()
            },
        );
        log.flush().await;
        return ExitCode::FAILURE;
    }
    log.flush().await;
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaiser_soze::logging::{Format, Level};

    #[test]
    fn stream_selects_debug_without_an_explicit_level() {
        let args = parse_args_from(
            ["--tap-file", "synthetic.jsonl", "--stream"].map(String::from),
            None,
            Format::Human,
        )
        .unwrap();
        assert!(args.logging.stream);
        assert_eq!(args.logging.level, Level::Debug);
    }

    #[test]
    fn explicit_logging_options_override_stream_defaults() {
        let args = parse_args_from(
            [
                "--tap-file",
                "synthetic.jsonl",
                "--stream",
                "--log-level",
                "warn",
                "--log-format",
                "json",
            ]
            .map(String::from),
            None,
            Format::Human,
        )
        .unwrap();
        assert_eq!(args.logging.level, Level::Warn);
        assert_eq!(args.logging.format, Format::Json);
    }

    #[test]
    fn invalid_logging_values_and_unknown_flags_are_rejected() {
        for flags in [
            ["--log-level", "verbose"],
            ["--log-format", "xml"],
            ["--unknown", "value"],
        ] {
            assert!(parse_args_from(
                ["--tap-file", "synthetic.jsonl"]
                    .into_iter()
                    .chain(flags)
                    .map(String::from),
                None,
                Format::Human
            )
            .is_err());
        }
    }

    #[test]
    fn debug_file_is_optional_and_can_be_named() {
        let args = parse_args_from(
            ["--tap-file", "synthetic.jsonl", "--log-file", "debug.jsonl"].map(String::from),
            None,
            Format::Human,
        )
        .unwrap();
        assert_eq!(args.log_file, Some(PathBuf::from("debug.jsonl")));
        let args = parse_args_from(["--tap-file", "synthetic.jsonl"].map(String::from), None, Format::Human).unwrap();
        assert!(args.log_file.is_none());
    }

    #[test]
    fn a_debug_file_cannot_overwrite_capture_files_or_their_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let tap = dir.path().join("tap.jsonl");
        let alias = dir.path().join("alias.jsonl");
        std::fs::write(&tap, b"synthetic capture\n").unwrap();
        std::fs::hard_link(&tap, &alias).unwrap();
        for log in [&tap, &alias] {
            let args = parse_args_from(
                [
                    "--tap-file".into(),
                    tap.display().to_string(),
                    "--log-file".into(),
                    log.display().to_string(),
                ],
                None,
                Format::Human,
            );
            assert!(args.is_err());
        }
        assert_eq!(std::fs::metadata(&tap).unwrap().len(), 18);
    }

    #[test]
    fn a_new_debug_file_cannot_share_a_relative_capture_path() {
        let tap = format!("synthetic-absent-{}.jsonl", std::process::id());
        assert!(!Path::new(&tap).exists());
        let args = parse_args_from(
            [
                "--tap-file".into(),
                tap.clone(),
                "--log-file".into(),
                format!("./{tap}"),
            ],
            None,
            Format::Human,
        );
        assert!(args.is_err());
    }

    #[test]
    fn dangling_symlinks_cannot_join_debug_and_capture_destinations() {
        for capture_flag in ["--tap-file", "--shape-file"] {
            for link_is_debug in [true, false] {
                let dir = tempfile::tempdir().unwrap();
                let capture = dir.path().join("capture.jsonl");
                let debug = dir.path().join("debug.jsonl");
                let (target, link) = if link_is_debug {
                    (&capture, &debug)
                } else {
                    (&debug, &capture)
                };
                std::os::unix::fs::symlink(target.file_name().unwrap(), link).unwrap();
                let args = parse_args_from(
                    [
                        "--tap-file".into(),
                        dir.path().join("tap.jsonl").display().to_string(),
                        capture_flag.into(),
                        capture.display().to_string(),
                        "--log-file".into(),
                        debug.display().to_string(),
                    ],
                    None,
                    Format::Human,
                );
                assert!(args.is_err());
                assert!(!target.exists());
            }
        }
    }

    #[tokio::test]
    async fn a_duplicate_service_launch_does_not_reset_the_active_debug_file() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let debug = dir.path().join("debug.jsonl");
        std::fs::write(&debug, b"synthetic active run\n").unwrap();
        let args = Args {
            port: listener.local_addr().unwrap().port(),
            tap_file: dir.path().join("tap.jsonl"),
            shape_file: None,
            log_file: Some(debug.clone()),
            logging: Settings::default(),
        };
        assert!(run(args, &mut Logger::default()).await.is_err());
        assert_eq!(std::fs::metadata(debug).unwrap().len(), 21);
        assert!(!dir.path().join("tap.jsonl").exists());
    }
}
