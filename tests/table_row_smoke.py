"""Edit simple-table cells through a real PTY, without credentials or Notion writes."""
import fcntl
import os
from pathlib import Path
import re
import select
import signal
import struct
import subprocess
import termios
import time

root = Path(__file__).resolve().parents[1]
master, slave = os.openpty()
original = termios.tcgetattr(slave)
size = [32, 110]


def controlling_terminal():
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)


fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", *size, 0, 0))
process = subprocess.Popen(
    [str(root / "target/debug/notion-tui"), "--demo"],
    stdin=slave, stdout=slave, stderr=slave,
    env={**os.environ, "TERM": "xterm-256color"},
    preexec_fn=controlling_terminal,
)


def resize():
    size[1] += 1 if size[1] % 2 == 0 else -1
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", *size, 0, 0))
    if process.poll() is None:
        os.kill(process.pid, signal.SIGWINCH)


def wait_for(text):
    data = bytearray()
    deadline = time.monotonic() + 10
    next_resize = time.monotonic() + 0.5
    expected = b"".join(text.encode().split())
    while time.monotonic() < deadline:
        if select.select([master], [], [], 0.1)[0]:
            data.extend(os.read(master, 65536))
            visible = re.sub(rb"\x1b\][^\x1b]*\x1b\\", b"", bytes(data))
            visible = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", visible)
            if expected in b"".join(visible.split()):
                return
        if time.monotonic() > next_resize:
            resize()
            next_resize = time.monotonic() + 0.5
        if process.poll() is not None:
            break
    raise AssertionError(f"Did not observe {text!r}: {bytes(data[-2000:])!r}")


def send(text):
    os.write(master, text.encode())


def paste(text):
    send("\x1b[200~" + text + "\x1b[201~")


try:
    wait_for("Welcome to Notion TUI")
    send("\r")
    wait_for("Your workspace")
    send("jjj\r")
    wait_for("Nested content")
    send("G\r")
    wait_for("Alpha")
    send("e")
    wait_for("EDIT TABLE ROW")
    send("\x13")
    wait_for("No changes")
    send("e")
    wait_for("Cell 1/3")
    paste(" first │ value\n你🦀x")
    send("\x1b[D\x7f")
    wait_for("你x")
    send("\t")
    paste(" second draft")
    wait_for("Beta second draft")
    send("\x1b[Z")
    wait_for("Cell 1/3")
    wait_for("你x")
    send("\x1b[Z")
    paste("third value")
    wait_for("Cell 3/3")
    send("\x13")
    wait_for("Demo is read-only")
    wait_for("third value")
    send("\t")
    wait_for("你x")
    send("\t")
    wait_for("Beta second draft")
    send("\x1b")
    wait_for("Discard this draft?")
    send("n")
    wait_for("Beta second draft")
    send("\x1b")
    wait_for("Discard this draft?")
    send("yje")
    wait_for("Styled")
    paste(" changed")
    send("\x13")
    wait_for("Unchanged cells keep their formatting")
    send("y")
    wait_for("Demo is read-only")
    paste(" again")
    send("\x13")
    wait_for("Unchanged cells keep their formatting")
    send("n")
    size[:] = [12, 45]
    resize()
    wait_for("Cell 1/3")
    send("\x1b[Z")
    paste("narrow")
    wait_for("Cell 3/3")
    wait_for("narrow")
    send("\x1b")
    wait_for("Discard this draft?")
    send("yq")
    assert process.wait(timeout=10) == 0
    assert termios.tcgetattr(slave) == original, "Terminal settings were not restored"
    print("Table-row PTY smoke passed: cells, Unicode, paste, Backspace, formatting confirmation, retained drafts, narrow layout, terminal restoration")
finally:
    if process.poll() is None:
        process.kill()
        process.wait()
    os.close(master)
    os.close(slave)
