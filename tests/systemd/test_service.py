#!/usr/bin/env python3
"""Real user-service integration tests; run only inside the disposable systemd image."""

import concurrent.futures
import hashlib
import http.client
import json
import os
from pathlib import Path
import selectors
import shlex
import socket
import subprocess
import tempfile
import time
import unittest


ROOT = Path("/opt/soze")
DATA = Path.home() / ".local/share/soze"
CONFIG = Path.home() / ".config/soze/config.toml"
UNIT = Path.home() / ".config/systemd/user/soze.service"
BINARY = ROOT / "soze"


class UserService(unittest.TestCase):
    def setUp(self):
        self.assertNotEqual(os.getuid(), 0)
        self.assertEqual(Path("/proc/1/comm").read_text().strip(), "systemd")
        self.run_command("systemctl", "--user", "reset-failed")
        CONFIG.parent.mkdir(parents=True, exist_ok=True)
        CONFIG.write_text('log_format = "json"\nlog_level = "info"\nlive = false\n')
        self.addCleanup(self.service, "uninstall", "--purge")

    def run_command(self, *args):
        result = subprocess.run(args, capture_output=True, timeout=20, check=False)
        self.assertEqual(result.returncode, 0, f"command failed: {args[0]}")
        return result.stdout.decode()

    def service(self, *args):
        return self.run_command("sh", str(ROOT / "scripts/service.sh"), *args)

    def property(self, name):
        return self.run_command(
            "systemctl", "--user", "show", "soze.service", f"--property={name}", "--value"
        ).strip()

    def until(self, predicate):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(0.1)
        self.fail("service condition did not settle within 15 seconds")

    def listening(self):
        with socket.socket() as connection:
            connection.settimeout(0.2)
            return connection.connect_ex(("127.0.0.1", 48731)) == 0

    def install(self):
        self.service("install", str(BINARY))
        self.until(self.listening)

    def events(self):
        path = DATA / "debug.jsonl"
        if not path.exists():
            return []
        # The writer may still be appending the last metadata record.
        return [json.loads(line)["event"] for line in path.read_bytes().splitlines(keepends=True) if line.endswith(b"\n")]

    def relay(self):
        payload = b"synthetic-systemd-probe"
        with socket.socket() as upstream, concurrent.futures.ThreadPoolExecutor() as executor:
            upstream.bind(("127.0.0.1", 0))
            upstream.listen(1)
            upstream.settimeout(5)

            def echo():
                connection, _ = upstream.accept()
                with connection:
                    connection.settimeout(5)
                    received = connection.makefile("rb").read(len(payload))
                    connection.sendall(received)

            response = executor.submit(echo)
            client = http.client.HTTPConnection("127.0.0.1", 48731, timeout=5)
            client.set_tunnel("127.0.0.1", upstream.getsockname()[1])
            try:
                client.connect()
                client.sock.sendall(payload)
                received = client.sock.makefile("rb").read(len(payload))
                self.assertTrue(received == payload, "tunnel changed the synthetic bytes")
            finally:
                client.close()
            response.result(timeout=5)
        self.until(lambda: (DATA / "tap.jsonl").stat().st_size > 0)

    def test_install_enables_a_real_non_root_service(self):
        CONFIG.unlink()

        self.install()

        # Real systemd loads the unit, starts it as the test user and enables login startup.
        self.assertEqual(self.property("ActiveState"), "active")
        self.assertEqual(self.run_command("systemctl", "--user", "is-enabled", "soze.service").strip(), "enabled")
        self.run_command("systemd-analyze", "--user", "verify", str(UNIT))
        pid = int(self.property("MainPID"))
        uid = next(line.split()[1] for line in Path(f"/proc/{pid}/status").read_text().splitlines() if line.startswith("Uid:"))
        self.assertEqual(int(uid), os.getuid())
        self.assertEqual(Path(f"/proc/{pid}/exe").resolve(), DATA / "bin/soze")
        self.assertEqual(CONFIG.read_text(), 'log_format = "human"\nlive = true\n')
        self.assertEqual((DATA / "logs/soze.log").stat().st_mode & 0o777, 0o600)
        self.assertEqual((DATA / "tap.jsonl").stat().st_mode & 0o777, 0o600)
        self.assertFalse((Path.home() / ".claude").exists())
        self.assertFalse((Path.home() / ".codex").exists())
        self.assertFalse((Path.home() / ".cursor").exists())

    def test_status_reports_the_real_pid_and_installation(self):
        self.install()

        status = dict(line.split("=", 1) for line in self.service("status").splitlines())

        # The public status command agrees with the manager, including installation metadata.
        self.assertEqual(status["state"], "running")
        self.assertEqual(status["pid"], self.property("MainPID"))
        self.assertEqual(status["source"], "local")
        self.assertIn("installed_at", status)

    def test_proxy_relays_synthetic_traffic_under_systemd(self):
        self.install()

        self.relay()

        # Running under systemd still forwards bytes and creates capture output.
        self.assertGreater((DATA / "tap.jsonl").stat().st_size, 0)
        self.assertGreater((DATA / "logs/soze.log").stat().st_size, 0)

    def test_reload_applies_logging_without_restarting(self):
        self.install()
        pid = self.property("MainPID")
        CONFIG.write_text('log_format = "json"\nlog_level = "debug"\nlive = false\n')

        self.service("reload")
        self.until(lambda: "config_reloaded" in self.events())

        # ExecReload delivers SIGHUP to the same running process.
        self.assertEqual(self.property("MainPID"), pid)
        self.assertTrue(self.listening())

    def test_logs_follow_operational_metadata(self):
        self.install()
        follower = subprocess.Popen(
            ["sh", str(ROOT / "scripts/service.sh"), "logs"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        try:
            with selectors.DefaultSelector() as ready:
                ready.register(follower.stdout, selectors.EVENT_READ)
                self.assertTrue(ready.select(timeout=5), "no operational metadata arrived")
                record = json.loads(follower.stdout.readline())
        finally:
            follower.terminate()
            follower.wait(timeout=5)
            follower.stdout.close()

        # The real logs command follows structured operational output, not captured content.
        self.assertIn(record["event"], self.events())

    def test_restart_replaces_the_process(self):
        self.install()
        pid = self.property("MainPID")

        self.service("restart")
        self.until(self.listening)

        # Explicit restart returns a new live process.
        self.assertNotEqual(self.property("MainPID"), pid)
        self.assertEqual(self.property("ActiveState"), "active")

    def test_systemd_recovers_after_a_crash(self):
        self.install()
        pid = self.property("MainPID")

        self.run_command("systemctl", "--user", "kill", "--kill-whom=main", "--signal=KILL", "soze.service")
        self.until(lambda: self.property("MainPID") not in ("0", pid) and self.listening())

        # Restart=on-failure recovers a killed proxy without the installer intervening.
        self.assertEqual(self.property("ActiveState"), "active")
        self.assertGreaterEqual(int(self.property("NRestarts")), 1)

    def test_update_preserves_config_and_rotates_existing_capture(self):
        self.install()
        self.relay()
        self.run_command("systemctl", "--user", "stop", "soze.service")
        config_before = CONFIG.read_bytes()
        capture_before = hashlib.sha256((DATA / "tap.jsonl").read_bytes()).digest()
        inode_before = (DATA / "bin/soze").stat().st_ino

        self.install()

        # Update replaces the binary inode and retains the old capture in a closed segment.
        self.assertEqual(CONFIG.read_bytes(), config_before)
        self.assertNotEqual((DATA / "bin/soze").stat().st_ino, inode_before)
        self.assertTrue(any(hashlib.sha256(path.read_bytes()).digest() == capture_before for path in DATA.glob("tap.*.jsonl")))
        self.assertTrue((DATA / "logs/soze.log.1").is_file())

    def test_caller_only_config_override_leaves_the_running_installation_untouched(self):
        self.install()
        self.relay()
        pid = self.property("MainPID")
        paths = [UNIT, *CONFIG.parent.rglob("*"), *DATA.rglob("*")]
        before = {
            path: (path.stat().st_ino, hashlib.sha256(path.read_bytes()).digest())
            for path in paths if path.is_file()
        }
        with tempfile.TemporaryDirectory(prefix="soze caller config ") as directory:
            override = Path(directory) / "config"
            environment = dict(os.environ, XDG_CONFIG_HOME=str(override))

            result = subprocess.run(
                ["sh", str(ROOT / "scripts/service.sh"), "install", str(BINARY)],
                env=environment, capture_output=True, timeout=20, check=False,
            )

            # Rejection cannot replace service/config/data files, rotate logs, or restart the PID.
            self.assertEqual(result.returncode, 2)
            self.assertFalse(override.exists())
        after_paths = [UNIT, *CONFIG.parent.rglob("*"), *DATA.rglob("*")]
        after = {
            path: (path.stat().st_ino, hashlib.sha256(path.read_bytes()).digest())
            for path in after_paths if path.is_file()
        }
        self.assertTrue(before == after, "installed file identities and digests changed")
        self.assertEqual(self.property("MainPID"), pid)
        self.assertEqual(self.property("ActiveState"), "active")
        self.assertTrue(self.listening())

    def test_a_searched_but_shadowed_directory_cannot_update_the_loaded_installation(self):
        self.install()
        self.relay()
        lower_config = Path.home() / ".local/share"
        lower_unit = lower_config / "systemd/user/soze.service"
        search_paths = shlex.split(self.run_command(
            "systemctl", "--user", "show", "--property=UnitPath", "--value"
        ))
        self.assertIn(str(lower_unit.parent), search_paths)
        self.assertLess(search_paths.index(str(UNIT.parent)), search_paths.index(str(lower_unit.parent)))
        self.assertFalse(lower_unit.exists())
        pid = self.property("MainPID")
        fragment = self.property("FragmentPath")
        paths = [UNIT, *CONFIG.parent.rglob("*"), *DATA.rglob("*")]
        before = {
            path: (path.stat().st_ino, hashlib.sha256(path.read_bytes()).digest())
            for path in paths if path.is_file()
        }

        result = subprocess.run(
            ["sh", str(ROOT / "scripts/service.sh"), "install", str(BINARY)],
            env=dict(os.environ, XDG_CONFIG_HOME=str(lower_config)),
            capture_output=True, timeout=20, check=False,
        )

        # A lower-priority searched directory cannot replace shared files or restart the loaded unit.
        self.assertEqual(result.returncode, 2)
        self.assertFalse(lower_unit.exists())
        after_paths = [UNIT, *CONFIG.parent.rglob("*"), *DATA.rglob("*")]
        after = {
            path: (path.stat().st_ino, hashlib.sha256(path.read_bytes()).digest())
            for path in after_paths if path.is_file()
        }
        self.assertTrue(before == after, "installed file identities and digests changed")
        self.assertEqual(self.property("MainPID"), pid)
        self.assertEqual(self.property("FragmentPath"), fragment)
        self.assertEqual(self.property("ActiveState"), "active")
        self.assertEqual(self.run_command("systemctl", "--user", "is-enabled", "soze.service").strip(), "enabled")

    def test_uninstall_preserves_data_and_purge_removes_only_data(self):
        self.install()
        self.relay()
        self.run_command("systemctl", "--user", "stop", "soze.service")
        capture_before = hashlib.sha256((DATA / "tap.jsonl").read_bytes()).digest()

        self.service("uninstall")

        # Uninstall removes supervision and executable files, not captured data or CA/config.
        self.assertEqual(self.property("LoadState"), "not-found")
        self.assertEqual(self.service("status"), "state=not loaded\npid=\n")
        self.assertFalse(UNIT.exists())
        self.assertFalse((DATA / "bin").exists())
        self.assertEqual(hashlib.sha256((DATA / "tap.jsonl").read_bytes()).digest(), capture_before)
        self.assertTrue(CONFIG.exists())
        self.assertTrue((CONFIG.parent / "ca.pem").exists())
        self.service("uninstall", "--purge")
        self.assertFalse(DATA.exists())
        self.assertTrue(CONFIG.exists())
        self.assertTrue((CONFIG.parent / "ca.pem").exists())

    def test_caller_only_override_cannot_uninstall_or_purge_the_running_service(self):
        self.install()
        self.relay()
        pid = self.property("MainPID")
        fragment = self.property("FragmentPath")
        paths = [UNIT, *CONFIG.parent.rglob("*"), *DATA.rglob("*")]
        before = {
            path: (path.stat().st_ino, hashlib.sha256(path.read_bytes()).digest())
            for path in paths if path.is_file()
        }
        with tempfile.TemporaryDirectory(prefix="soze caller config ") as directory:
            override = Path(directory) / "config"
            environment = dict(os.environ, XDG_CONFIG_HOME=str(override))
            for arguments in (("uninstall",), ("uninstall", "--purge")):
                with self.subTest(arguments=arguments):
                    result = subprocess.run(
                        ["sh", str(ROOT / "scripts/service.sh"), *arguments],
                        env=environment, capture_output=True, timeout=20, check=False,
                    )

                    # A wrong caller path cannot stop, disable, remove, purge, or alter the service.
                    self.assertEqual(result.returncode, 2)
                    self.assertFalse(override.exists())
                    after_paths = [UNIT, *CONFIG.parent.rglob("*"), *DATA.rglob("*")]
                    after = {
                        path: (path.stat().st_ino, hashlib.sha256(path.read_bytes()).digest())
                        for path in after_paths if path.is_file()
                    }
                    self.assertTrue(before == after, "installed file identities and digests changed")
                    self.assertEqual(self.property("MainPID"), pid)
                    self.assertEqual(self.property("FragmentPath"), fragment)
                    self.assertEqual(self.property("ActiveState"), "active")
                    self.assertEqual(self.run_command("systemctl", "--user", "is-enabled", "soze.service").strip(), "enabled")


if __name__ == "__main__":
    unittest.main(verbosity=2)
