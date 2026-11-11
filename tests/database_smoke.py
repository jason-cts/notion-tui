"""Browse saved list/board/table views through a real PTY, without credentials."""
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

def resize():
    size[1] = size[1] + 1 if size[1] % 2 == 0 else size[1] - 1
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", *size, 0, 0))
    if process.poll() is None:
        os.kill(process.pid, signal.SIGWINCH)

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

try:
    wait_for("Welcome to Notion TUI")
    send("\r")
    wait_for("Your workspace")
    send("Gk")
    wait_for("child_database (list)")
    wait_for("Task database  ›")
    send("\r")
    wait_for("Alpha")
    send("]")
    wait_for("To do (1)")
    send("l\r")
    wait_for("Your workspace")
    send("\x7f")
    wait_for("In progress (1)")
    send("v")
    wait_for("Beta")
    send("\x1b")
    wait_for("child_database (board)")
    send("]")
    wait_for("child_database (table)")
    wait_for("Alpha")
    send("a")
    wait_for("Database browsing is read-only")
    send("[")
    wait_for("To do (1)")
    for back in ["\x1b[D", "\x7f", "\x08"]:
        send(back)
        wait_for("Your workspace")
        send("Gk\r")
        wait_for("To do (1)")
    size[:] = [12, 45]
    resize()
    wait_for("Alpha")
    send("q")
    assert process.wait(timeout=10) == 0
    assert termios.tcgetattr(slave) == original, "Terminal settings were not restored"
    print("Database PTY smoke passed: list, board, table, child indicator, Left/DEL/BS parent navigation, narrow layout")
finally:
    if process.poll() is None:
        process.kill()
        process.wait()
    os.close(master)
    os.close(slave)
