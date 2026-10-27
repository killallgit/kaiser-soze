use std::io::{ErrorKind, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

fn stalled_output() -> (OwnedFd, UnixStream) {
    let (mut writer, unread) = UnixStream::pair().unwrap();
    writer.set_nonblocking(true).unwrap();
    loop {
        match writer.write(&[0; 8192]) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => panic!("unexpected I/O kind: {:?}", error.kind()),
        }
    }
    writer.set_nonblocking(false).unwrap();
    (writer.into(), unread)
}

fn wait_for_exit(mut child: Child) -> Option<ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_stalled_stderr_does_not_prevent_startup_failure_from_exiting() {
    let (stderr, _unread) = stalled_output();
    let child = Command::new(env!("CARGO_BIN_EXE_soze"))
        .arg("--tap-file")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .unwrap();
    assert!(wait_for_exit(child).is_some_and(|status| !status.success()));
}

#[test]
fn help_and_version_succeed_without_loading_configuration_or_opening_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("missing.toml");
    let output = dir.path().join("capture.jsonl");
    for flag in ["--help", "--version"] {
        let child = Command::new(env!("CARGO_BIN_EXE_soze"))
            .args([
                "--config",
                config.to_str().unwrap(),
                "--tap-file",
                output.to_str().unwrap(),
                "--log-file",
                output.to_str().unwrap(),
                flag,
            ])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(wait_for_exit(child).is_some_and(|status| status.success()));
        assert!(!output.exists());
        assert!(!config.exists());
    }
}

#[test]
fn a_stalled_stdout_does_not_prevent_help_from_exiting() {
    let (stdout, _unread) = stalled_output();
    let child = Command::new(env!("CARGO_BIN_EXE_soze"))
        .arg("--help")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(wait_for_exit(child).is_some_and(|status| !status.success()));
}
