use std::io::{ErrorKind, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn a_stalled_stderr_does_not_prevent_startup_failure_from_exiting() {
    let (mut writer, _unread) = UnixStream::pair().unwrap();
    writer.set_nonblocking(true).unwrap();
    loop {
        match writer.write(&[0; 8192]) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => panic!("unexpected I/O kind: {:?}", error.kind()),
        }
    }
    writer.set_nonblocking(false).unwrap();
    let stderr: OwnedFd = writer.into();
    let mut child = Command::new(env!("CARGO_BIN_EXE_soze"))
        .arg("--tap-file") // Missing value fails parsing before opening files or starting a proxy.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.is_some_and(|status| !status.success()));
}
