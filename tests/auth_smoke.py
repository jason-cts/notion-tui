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


def check_login(config_home, force, deny, interactive=False):
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
        "NOTION_OAUTH_CLIENT_SECRET": "" if interactive else "secret-must-not-appear",
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
                visible = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", bytes(transcript))
                if b"".join(marker.split()) in b"".join(visible.split()):
                    return
            if process.poll() is not None:
                break
        raise AssertionError(f"Missing {marker!r}: {transcript[-2000:]!r}")

    try:
        if interactive:
            wait_for(b"CONNECT TO NOTION")
            # Client ID and redirect come from env; complete the missing secret.
            os.write(master, b"\t\x1b[200~secret-must-not-appear\x1b[201~\x13")
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
        if interactive:
            assert b"\x1b[?1049l" in transcript, "Setup did not restore the alternate screen"
        else:
            assert b"\x1b[?1049h" not in transcript, "Configured login entered the alternate screen"
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


def check_setup(config_home, api_token=False):
    env = {key: value for key, value in os.environ.items() if not key.startswith("NOTION_")}
    env.update({"TERM": "xterm-256color", "XDG_CONFIG_HOME": str(config_home)})
    # A manually entered dummy API token must not contact Notion in this test.
    for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"]:
        env[key] = "http://127.0.0.1:9"
    env["NO_PROXY"] = env["no_proxy"] = ""
    master, slave = os.openpty()
    original = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))

    def controlling_terminal():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

    process = subprocess.Popen([str(binary)], stdin=slave, stdout=slave, stderr=slave,
                               env=env, preexec_fn=controlling_terminal)
    transcript = bytearray()

    def wait_for(marker):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                transcript.extend(os.read(master, 65536))
                visible = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", bytes(transcript))
                if b"".join(marker.split()) in b"".join(visible.split()):
                    return
            if process.poll() is not None:
                break
        raise AssertionError(f"Missing {marker!r}: {transcript[-2000:]!r}")

    try:
        wait_for(b"CONNECT TO NOTION")
        os.write(master, b"\r")
        wait_for(b"API token is required")
        token = b"typed-token-must-be-hidden"
        os.write(master, b"\x1b[200~" + token + b"\x1b[201~")
        wait_for(b"********")
        if api_token:
            os.write(master, b"\x13")
            wait_for(b"PAGES")
            os.write(master, b"q")
        else:
            # Exercise switching methods, narrow layout, and setup cancellation.
            os.write(master, b"\x1bOQ")  # F2
            wait_for(b"OAuth client ID")
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 12, 25, 0, 0))
            os.kill(process.pid, signal.SIGWINCH)
            wait_for(b"Resize terminal")
            os.write(master, b"\x03")
        assert process.wait(timeout=10) == 0
        while select.select([master], [], [], 0)[0]:
            transcript.extend(os.read(master, 65536))
        assert token not in transcript
        assert termios.tcgetattr(slave) == original
        assert b"\x1b[?1049l" in transcript
        assert b"\x1b[?2004l" in transcript
        assert not (config_home / "notion-tui/auth.json").exists()
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
    check_login(config_home, force=True, deny=False, interactive=True)
    check_setup(config_home)
    check_setup(config_home, api_token=True)
    auth_dir = config_home / "notion-tui"
    auth_dir.mkdir(mode=0o700, exist_ok=True)
    auth_file = auth_dir / "auth.json"
    auth_file.write_text("a corrupt local login can still be removed")
    auth_file.chmod(0o600)
    logout = subprocess.run([str(binary), "--logout"], capture_output=True, env=env)
    assert logout.returncode == 0 and not auth_file.exists(), logout.stderr

print("PASS: interactive API/OAuth setup, masking, validation, method switching, blank-token fallback, forced login, browser fallback, Ctrl-C, denial, terminal preservation, non-TTY help/logout")
