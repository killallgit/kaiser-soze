use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

struct Soze {
    child: Child,
    lines: Receiver<String>,
}

impl Soze {
    fn start(config: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_soze"))
            .args(["--log-format", "json", "--config"])
            .arg(config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = BufReader::new(child.stderr.take().unwrap());
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            stderr
                .lines()
                .map_while(Result::ok)
                .for_each(|line| drop(sender.send(line)))
        });
        Self { child, lines }
    }

    fn next(&mut self, event: &str) -> Value {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(30))
                .expect("no line in 30s");
            let record: Value = serde_json::from_str(&line).unwrap();
            if record["event"] == event {
                return record;
            }
        }
    }

    fn hangup(&self) {
        let status = Command::new("kill")
            .args(["-HUP", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }
}

impl Drop for Soze {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

fn write(path: &std::path::Path, tap: &std::path::Path, text: &str) {
    std::fs::write(path, format!("tap_file = '{}'\n{text}", tap.display())).unwrap();
}

#[test]
fn a_hangup_with_a_bad_file_keeps_serving_and_reports_the_kind() {
    let dir = tempfile::tempdir().unwrap();
    let (config, tap) = (dir.path().join("config.toml"), dir.path().join("tap.jsonl"));
    write(&config, &tap, "log_level = 'info'\n");
    let mut soze = Soze::start(&config);
    let port = soze.next("listening")["port"].as_u64().unwrap();

    write(&config, &tap, "version = 2\n");
    soze.hangup();
    let failed = soze.next("config_reload_failed");

    assert_eq!(failed["error_kind"], "unsupported_version");
    assert!(TcpStream::connect(("127.0.0.1", port as u16)).is_ok());
}

#[test]
fn a_hangup_names_each_restart_only_key_it_did_not_apply() {
    let dir = tempfile::tempdir().unwrap();
    let (config, tap) = (dir.path().join("config.toml"), dir.path().join("tap.jsonl"));
    write(&config, &tap, "log_level = 'info'\n");
    let mut soze = Soze::start(&config);
    soze.next("listening");

    write(&config, &tap, "port = 45999\nlog_level = 'info'\n");
    soze.hangup();

    assert_eq!(soze.next("config_restart_required")["field"], "port");
}

#[test]
fn a_hangup_with_a_good_file_applies_the_live_keys() {
    let dir = tempfile::tempdir().unwrap();
    let (config, tap) = (dir.path().join("config.toml"), dir.path().join("tap.jsonl"));
    write(&config, &tap, "log_level = 'info'\n");
    let mut soze = Soze::start(&config);
    soze.next("listening");

    write(&config, &tap, "log_level = 'info'\nlive = true\n");
    soze.hangup();

    soze.next("config_reloaded");
}
