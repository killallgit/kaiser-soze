use kaiser_soze::claude::ClaudeAdapter;
use kaiser_soze::codex::CodexAdapter;
use kaiser_soze::config::config_dir;
use kaiser_soze::live::Live;
use kaiser_soze::proxy::{Proxy, ProxyConfig};
use kaiser_soze::BoxError;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

struct Args {
    port: u16,
    tap_file: PathBuf,
    config_dir: PathBuf,
    shape_file: Option<PathBuf>,
    live: bool,
}

fn parse_args() -> Result<Args, BoxError> {
    let mut port = 0;
    let mut tap_file = None;
    let mut shape_file = None;
    let mut live = false;
    let mut config_flag = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--live" {
            live = true;
            continue;
        }
        let value = args.next().ok_or("flag without a value")?;
        match flag.as_str() {
            "--port" => port = value.parse()?,
            "--tap-file" => tap_file = Some(PathBuf::from(value)),
            "--shape-file" => shape_file = Some(PathBuf::from(value)),
            "--config-dir" => config_flag = Some(PathBuf::from(value)),
            _ => return Err("unknown flag".into()),
        }
    }
    Ok(Args {
        port,
        tap_file: tap_file.ok_or("--tap-file is required")?,
        config_dir: config_dir(
            config_flag,
            std::env::var("SOZE_CONFIG_DIR").ok(),
            &std::env::var("HOME")?,
        ),
        shape_file,
        live,
    })
}

/// The live view: standard error, the home folder shortened to `~`, and the local clock.
fn live_output() -> Live {
    let home = std::env::var("HOME").unwrap_or_default();
    let stderr = std::io::stderr().as_fd().try_clone_to_owned().map(std::fs::File::from);
    Live::new(
        tokio::fs::File::from_std(stderr.expect("duplicate standard error")),
        home,
        local_utc_offset(),
    )
}

/// Seconds east of UTC, from `date +%z`; UTC when that is unavailable.
fn local_utc_offset() -> i64 {
    let shown = std::process::Command::new("date").arg("+%z").output().ok();
    let text = shown.map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    let Some(text) = text.filter(|t| t.len() == 5) else {
        return 0;
    };
    let (sign, digits) = text.split_at(1);
    let (Ok(hours), Ok(minutes)) = (digits[..2].parse::<i64>(), digits[2..].parse::<i64>()) else {
        return 0;
    };
    let seconds = hours * 3600 + minutes * 60;
    if sign == "-" {
        -seconds
    } else {
        seconds
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let args = parse_args()?;
    let proxy = Proxy::new(ProxyConfig {
        adapters: vec![Arc::new(CodexAdapter::default()), Arc::new(ClaudeAdapter::default())],
        ca_dir: args.config_dir.clone(),
        tap_file: args.tap_file,
        shape_file: args.shape_file,
        upstream_roots: rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        },
        upstream_addr: None,
        live: args.live.then(live_output),
    })
    .await?;
    let listener = TcpListener::bind(("127.0.0.1", args.port)).await?;
    eprintln!("listening on {}", listener.local_addr()?);
    eprintln!("ca certificate {}", args.config_dir.join("ca.pem").display());
    proxy.serve(listener).await;
    Ok(())
}
