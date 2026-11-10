"""Check terminal styles, OSC 8 links, and browser dispatch without network access."""
import fcntl
import os
from pathlib import Path
import re
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time

root = Path(__file__).resolve().parents[1]


def assert_styled_words(transcript):
    """Check the effective SGR state at glyphs, including resets between spans."""
    active = set()
    glyphs = []
    for match in re.finditer(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][\s\S]*?(?:\x07|\x1b\\)|\x1b[78]|[^\x1b]+", transcript.decode()):
        chunk = match.group()
        if chunk.startswith("\x1b[") and chunk.endswith("m"):
            parameters = [int(value or "0") for value in chunk[2:-1].split(";")]
            index = 0
            while index < len(parameters):
                code = parameters[index]
                if code == 0:
                    active.clear()
                elif code in (1, 3, 4, 9):
                    active.add(code)
                elif code in (22, 23, 24, 29):
                    active.discard({22: 1, 23: 3, 24: 4, 29: 9}[code])
                elif code in (38, 48, 58) and index + 1 < len(parameters):
                    index += 5 if parameters[index + 1] == 2 else 3
                    continue
                index += 1
        elif not chunk.startswith("\x1b"):
            glyphs.extend((character, active.copy()) for character in chunk)
    for word, code in [("Bold", 1), ("italic", 3), ("underline", 4), ("strike", 9)]:
        assert any(all(character == expected and code in attributes
                       for (character, attributes), expected in zip(glyphs[start:start + len(word)], word))
                   for start in range(len(glyphs) - len(word) + 1)), f"{word} did not have SGR {code}"


with tempfile.TemporaryDirectory() as directory:
    temporary = Path(directory)
    browser_log = temporary / "browser-url"
    launcher = temporary / "xdg-open"
    launcher.write_text('#!/bin/sh\nprintf \'%s\\n\' "$1" > "$NOTION_TUI_TEST_BROWSER_LOG"\n')
    launcher.chmod(0o700)
    master, slave = os.openpty()
    original = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 110, 0, 0))

    def controlling_terminal():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

    process = subprocess.Popen(
        [str(root / "target/debug/notion-tui"), "--demo"],
        stdin=slave, stdout=slave, stderr=slave,
        env={**os.environ, "TERM": "xterm-256color", "PATH": directory,
             "NOTION_TUI_TEST_BROWSER_LOG": str(browser_log)},
        preexec_fn=controlling_terminal,
    )
    transcript = bytearray()

    def wait_for(text):
        data = bytearray()
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                chunk = os.read(master, 65536)
                data.extend(chunk)
                transcript.extend(chunk)
                visible = re.sub(rb"\x1b\][^\x07]*?(?:\x07|\x1b\\)", b"", bytes(data))
                visible = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", visible)
                if b"".join(text.encode().split()) in b"".join(visible.split()):
                    return
            if process.poll() is not None:
                break
        raise AssertionError(f"Did not observe {text!r}: {bytes(data[-2000:])!r}")

    def wait_for_browser():
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.05)[0]:
                transcript.extend(os.read(master, 65536))
            if browser_log.exists():
                assert browser_log.read_text().strip() == "https://www.google.com/"
                return
            if process.poll() is not None:
                break
        raise AssertionError("Browser launcher did not receive the URL")

    try:
        wait_for("Welcome to Notion TUI")
        os.write(master, b"\r")
        wait_for("Google")
        os.write(master, b"Go")
        wait_for_browser()
        assert browser_log.read_text().strip() == "https://www.google.com/"
        os.write(master, b"v")
        wait_for("FULL BLOCK")
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 45, 0, 0))
        os.kill(process.pid, signal.SIGWINCH)
        wait_for("FULL BLOCK")
        os.write(master, b"\x1b")
        wait_for("CONTENT")
        os.write(master, b"q")
        assert process.wait(timeout=10) == 0
        while select.select([master], [], [], 0)[0]:
            transcript.extend(os.read(master, 65536))
        for attribute in [1, 3, 4, 9]:
            assert f"\x1b[{attribute}m".encode() in transcript
        assert_styled_words(transcript)
        assert b"\x1b]8;;https://www.google.com/\x1b\\" in transcript
        assert b"\x1b]8;;\x1b\\" in transcript
        assert termios.tcgetattr(slave) == original
        assert b"\x1b[?1049l" in transcript
        assert b"\x1b[?2004l" in transcript
        print("PASS: bold, italic, underline, strike, OSC 8 targets, browser dispatch, reader resize, terminal restoration")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
        os.close(slave)
