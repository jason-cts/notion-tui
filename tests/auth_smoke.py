"""Check OAuth startup in a Linux PTY without opening a browser or contacting Notion."""
import fcntl
import http.client
import os
from pathlib import Path
import re
import select
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import time
from urllib.parse import parse_qs, urlencode, urlsplit


binary = Path(__file__).resolve().parents[1] / "target/debug/notion-tui"


def check_login(config_home, force, deny):
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    redirect = f"http://127.0.0.1:{port}/oauth/callback"
    env = {
        **os.environ,
        "TERM": "xterm-256color",
        "XDG_CONFIG_HOME": str(config_home),
        "NOTION_API_TOKEN": "ignored-by-forced-login" if force else " \t ",
        "NOTION_OAUTH_CLIENT_ID": "test-client",
        "NOTION_OAUTH_CLIENT_SECRET": "secret-must-not-appear",
        "NOTION_OAUTH_REDIRECT_URI": redirect,
        # All desktop launchers are unavailable, so only the printed URL is used.
        "PATH": str(config_home / "empty-path"),
    }
    master, slave = os.openpty()
    original = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))

    def controlling_terminal():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

    process = subprocess.Popen(
        [str(binary), *(["--login"] if force else [])],
        stdin=slave, stdout=slave, stderr=slave, env=env,
        preexec_fn=controlling_terminal,
    )
    transcript = bytearray()

    def wait_for(marker):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                transcript.extend(os.read(master, 65536))
                if marker in transcript:
                    return
            if process.poll() is not None:
                break
        raise AssertionError(f"Missing {marker!r}: {transcript!r}")

    try:
        wait_for(b"Could not open a browser automatically")
        url = re.search(rb"https://api\.notion\.com/[^\s]+", transcript).group().decode()
        query = parse_qs(urlsplit(url).query)
        assert query["redirect_uri"] == [redirect]
        assert query["client_id"] == ["test-client"]
        if deny:
            connection = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
            connection.request("GET", "/oauth/callback?" + urlencode({
                "state": query["state"][0], "error": "access_denied",
            }))
            response = connection.getresponse()
            assert response.status == 200
            response.read()
            connection.close()
            wait_for(b"Notion authorization was cancelled")
        else:
            os.kill(process.pid, signal.SIGINT)
            wait_for(b"OAuth login cancelled")
        assert process.wait(timeout=10) == (1 if deny else 0)
        assert termios.tcgetattr(slave) == original, "Login changed terminal attributes"
        assert b"\x1b[?1049h" not in transcript, "Login entered the alternate screen"
        assert b"secret-must-not-appear" not in transcript
        assert not (config_home / "notion-tui/auth.json").exists()
        with socket.socket() as released:
            assert released.connect_ex(("127.0.0.1", port)) != 0
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
        os.close(slave)


with tempfile.TemporaryDirectory() as directory:
    config_home = Path(directory)
    (config_home / "empty-path").mkdir()
    env = {**os.environ, "XDG_CONFIG_HOME": directory}
    help_result = subprocess.run([str(binary), "--help"], capture_output=True, env=env)
    assert help_result.returncode == 0 and b"NOTION_OAUTH_CLIENT_ID" in help_result.stdout
    check_login(config_home, force=False, deny=False)
    check_login(config_home, force=True, deny=True)
    auth_dir = config_home / "notion-tui"
    auth_dir.mkdir(mode=0o700, exist_ok=True)
    auth_file = auth_dir / "auth.json"
    auth_file.write_text("a corrupt local login can still be removed")
    auth_file.chmod(0o600)
    logout = subprocess.run([str(binary), "--logout"], capture_output=True, env=env)
    assert logout.returncode == 0 and not auth_file.exists(), logout.stderr

print("PASS: blank-token OAuth fallback, forced login, browser fallback, Ctrl-C, denial, terminal preservation, non-TTY help/logout")
