"""Exercise the built app using a real Linux PTY, without Notion credentials."""
import fcntl
import os
import re
from pathlib import Path
import select
import struct
import subprocess
import termios
import time

root = Path(__file__).resolve().parents[1]
master, slave = os.openpty()
original = termios.tcgetattr(slave)
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 110, 0, 0))

def controlling_terminal():
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

process = subprocess.Popen(
    [str(root / "target/debug/notion-tui"), "--demo"],
    stdin=slave, stdout=slave, stderr=slave,
    env={**os.environ, "TERM": "xterm-256color"},
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
            # Ratatui emits only changed cells, using cursor jumps between words.
            visible = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", bytes(data))
            if b"".join(text.encode().split()) in b"".join(visible.split()):
                return
        if process.poll() is not None:
            break
    raise AssertionError(f"Did not observe {text!r}: {bytes(data[-3000:])!r}")

def send(text):
    os.write(master, text.encode())

try:
    wait_for("Welcome to Notion TUI")
    send("\r")
    wait_for("Your workspace, at your fingertips")
    send("jjj\r")
    wait_for("Nested content")
    send("\x7f")
    wait_for("Your workspace, at your fingertips")
    send("d")
    wait_for("DELETE BLOCK")
    send("\r")
    # Enter leaves the modal open; n cancels instead of creating a child page.
    send("n")
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 111, 0, 0))
    os.kill(process.pid, __import__("signal").SIGWINCH)
    wait_for("CONTENT")
    send("d")
    wait_for("Move to Trash?")
    send("\x1b")
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 110, 0, 0))
    os.kill(process.pid, __import__("signal").SIGWINCH)
    wait_for("CONTENT")
    send("d")
    wait_for("DELETE BLOCK")
    send("y")
    # Ratatui retains the shared "De" cells when Deleting becomes Demo.
    wait_for("to delete blocks")
    send("n")
    wait_for("NEW CHILD PAGE")
    # Exercise both DEL and BS through crossterm, including a multibyte glyph.
    send("🦀x\x7f\x08\x13")
    wait_for("A page title is required")
    send("\x1b")
    wait_for("Discard this draft?")
    send("y")
    wait_for("CONTENT")
    send("a")
    wait_for("APPEND BLOCK")
    send("draft text\x13")
    wait_for("Demo is read-only")
    send("\x1b")
    wait_for("Discard this draft?")
    send("y/")
    wait_for("SEARCH PAGES")
    send("Welcome\r")
    wait_for("1 pages")
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 45, 0, 0))
    os.kill(process.pid, __import__("signal").SIGWINCH)
    wait_for("PAGES")
    send("\t")
    wait_for("CONTENT")
    send("v")
    wait_for("FULL BLOCK")
    send("\x1b")
    wait_for("CONTENT")
    send("q")
    assert process.wait(timeout=10) == 0
    assert termios.tcgetattr(slave) == original, "Terminal attributes were not restored"
    while select.select([master], [], [], 0)[0]:
        transcript.extend(os.read(master, 65536))
    assert b"\x1b[?1049l" in transcript, "Alternate screen was not restored"
    assert b"\x1b[?2004l" in transcript, "Bracketed paste was not disabled"
    print("PASS: search, nested navigation, deletion confirmation/cancel/read-only failure, both Backspace encodings, failed-save draft, narrow layout, full-block reader, terminal restoration")
finally:
    if process.poll() is None:
        process.kill()
        process.wait()
    os.close(master)
    os.close(slave)
