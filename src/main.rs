mod cli;
mod config;

use config::Settings as AppSettings;
use kaiser_soze::claude::ClaudeAdapter;
use kaiser_soze::codex::CodexAdapter;
use kaiser_soze::logging::{Fields, Format, Level, Logger, Settings};
use kaiser_soze::proxy::{Proxy, ProxyConfig};
use rustls::pki_types::CertificateDer;
use rustls::RootCertStore;
use std::io::IsTerminal;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// The system store (and `SSL_CERT_FILE`) joins the bundled roots, so a network that re-signs TLS with its own CA still connects.
fn system_certs(log: &Logger) -> Vec<CertificateDer<'static>> {
    let loaded = rustls_native_certs::load_native_certs();
    if !loaded.errors.is_empty() {
        log.event(Level::Warn, "system_certs_partial", Fields::default());
    }
    loaded.certs
}

fn upstream_roots(system: Vec<CertificateDer<'static>>) -> RootCertStore {
    let mut roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    for cert in system {
        let _ = roots.add(cert);
    }
    roots
}

async fn run(args: AppSettings, log: &mut Logger) -> Result<(), &'static str> {
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
        upstream_roots: upstream_roots(system_certs(log)),
        upstream_addr: None,
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

async fn display(text: &str) -> ExitCode {
    let Ok(stdout) = std::io::stdout().as_fd().try_clone_to_owned() else {
        return ExitCode::FAILURE;
    };
    let mut stdout = tokio::fs::File::from_std(std::fs::File::from(stdout));
    let written = tokio::time::timeout(Duration::from_millis(250), async {
        stdout.write_all(text.as_bytes()).await?;
        stdout.flush().await
    })
    .await;
    if matches!(written, Ok(Ok(()))) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn service() -> ExitCode {
    let terminal = std::io::stderr().is_terminal();
    let default_format = if terminal { Format::Human } else { Format::Json };
    let parsed = cli::parse_args(default_format);
    if let Err(cli::Error::Display(text)) = &parsed {
        return display(text).await;
    }
    let settings = parsed.as_ref().map_or(
        Settings {
            format: default_format,
            ..Settings::default()
        },
        |args| args.logging(),
    );
    let Ok(stderr) = std::io::stderr().as_fd().try_clone_to_owned() else {
        return ExitCode::FAILURE;
    };
    let mut log = Logger::new(tokio::fs::File::from_std(std::fs::File::from(stderr)), settings);
    let result = match parsed {
        Ok(args) => run(args, &mut log).await,
        Err(error) => Err(error.kind()),
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

    #[test]
    fn system_certs_join_the_bundled_roots_and_unreadable_ones_are_skipped() {
        // It trusts a private CA the system store holds, keeps the bundled roots, and ignores bytes that are no cert
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let private_ca = params.self_signed(&key).unwrap().der().clone();
        let bundled = webpki_roots::TLS_SERVER_ROOTS.len();

        let roots = upstream_roots(vec![private_ca, rustls::pki_types::CertificateDer::from(vec![1, 2, 3])]);

        assert_eq!(roots.roots.len(), bundled + 1);
    }

    #[tokio::test]
    async fn a_duplicate_service_launch_does_not_reset_the_active_debug_file() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let debug = dir.path().join("debug.jsonl");
        std::fs::write(&debug, b"synthetic active run\n").unwrap();
        let args = cli::parse_args_from(
            [
                "--port".to_string(),
                listener.local_addr().unwrap().port().to_string(),
                "--tap-file".to_string(),
                dir.path().join("tap.jsonl").display().to_string(),
                "--log-file".to_string(),
                debug.display().to_string(),
            ],
            config::Sources::new(Format::Human),
        )
        .unwrap();
        assert!(run(args, &mut Logger::default()).await.is_err());
        assert_eq!(std::fs::metadata(debug).unwrap().len(), 21);
        assert!(!dir.path().join("tap.jsonl").exists());
    }
}
