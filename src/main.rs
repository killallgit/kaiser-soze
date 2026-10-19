use kaiser_soze::claude::ClaudeAdapter;
use kaiser_soze::codex::CodexAdapter;
use kaiser_soze::proxy::{Proxy, ProxyConfig};
use kaiser_soze::BoxError;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

struct Args {
    port: u16,
    tap_file: PathBuf,
    ca_dir: PathBuf,
    shape_file: Option<PathBuf>,
}

fn parse_args() -> Result<Args, BoxError> {
    let mut port = 0;
    let mut tap_file = None;
    let mut shape_file = None;
    let mut ca_dir = PathBuf::from(std::env::var("HOME")?).join(".config/soze");
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("flag without a value")?;
        match flag.as_str() {
            "--port" => port = value.parse()?,
            "--tap-file" => tap_file = Some(PathBuf::from(value)),
            "--shape-file" => shape_file = Some(PathBuf::from(value)),
            "--ca-dir" => ca_dir = PathBuf::from(value),
            _ => return Err("unknown flag".into()),
        }
    }
    Ok(Args {
        port,
        tap_file: tap_file.ok_or("--tap-file is required")?,
        ca_dir,
        shape_file,
    })
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let args = parse_args()?;
    let proxy = Proxy::new(ProxyConfig {
        adapters: vec![Arc::new(CodexAdapter::default()), Arc::new(ClaudeAdapter::default())],
        ca_dir: args.ca_dir.clone(),
        tap_file: args.tap_file,
        shape_file: args.shape_file,
        upstream_roots: rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        },
        upstream_addr: None,
    })
    .await?;
    let listener = TcpListener::bind(("127.0.0.1", args.port)).await?;
    eprintln!("listening on {}", listener.local_addr()?);
    eprintln!("ca certificate {}", args.ca_dir.join("ca.pem").display());
    proxy.serve(listener).await;
    Ok(())
}
