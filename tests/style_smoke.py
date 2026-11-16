"""Exercise rich-text shortcuts and the style dialog through a real PTY."""
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
transcript = bytearray()


def resize():
    size[1] = 111 if size[1] == 110 else 110
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
    env={**os.environ, "TERM": "xterm-256color", "NO_COLOR": "1"},
    preexec_fn=controlling_terminal,
)


def wait_for(text):
    data = bytearray()
    deadline = time.monotonic() + 10
    next_resize = time.monotonic() + 0.5
    while time.monotonic() < deadline:
        if select.select([master], [], [], 0.1)[0]:
            chunk = os.read(master, 65536)
            data.extend(chunk)
            transcript.extend(chunk)
            visible = re.sub(rb"\x1b\][^\x1b]*\x1b\\", b"", bytes(data))
            visible = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", visible)
            if b"".join(text.encode().split()) in b"".join(visible.split()):
                return
        if time.monotonic() >= next_resize:
            resize()
            next_resize = time.monotonic() + 0.5
        if process.poll() is not None:
            break
    raise AssertionError(f"Did not observe {text!r}: {bytes(data[-2500:])!r}")


def send(text):
    os.write(master, text.encode())


try:
    wait_for("Welcome to Notion TUI")
    send("\r")
    wait_for("Your workspace")
    send("a")
    wait_for("INSERT BLOCK")
    wait_for("B I U S")
    # Legacy Ctrl+I is indistinguishable from Tab: retain block-type switching.
    send("\t")
    wait_for("heading_1")
    # Enhanced Ctrl+I is distinct; Ctrl+B and Ctrl+U use their legacy encodings.
    send("\x02\x1b[105;5u\x15")
    send("StyledText")
    wait_for("StyledText")
    # Select the line and open Shift+F10's style picker.
    send("\x1b[1;2H\x1b[21;2~")
    wait_for("TEXT STYLE")
    wait_for("[x] Italic")
    send("\x13")  # Saving is suspended while the picker is open.
    resize()
    wait_for("TEXT STYLE")
    send("\x1b")
    wait_for("INSERT BLOCK")  # Esc cancels only the style picker.
    send("\x1b[21;2~")
    wait_for("TEXT STYLE")
    send("\x1b[B" * 3 + " \r")
    wait_for("INSERT BLOCK")
    send("\x13")
    wait_for("Demo is read-only")
    wait_for("StyledText")
    send("\x1b")
    wait_for("Discard this draft?")
    send("yGe")
    wait_for("EDIT BLOCK")
    send("\x13")
    wait_for("No changes")  # Existing annotations open without dirtying the draft.
    send("q")
    assert process.wait(timeout=10) == 0
    while select.select([master], [], [], 0)[0]:
        transcript.extend(os.read(master, 65536))
    for code in [b"\x1b[>1u", b"\x1b[<1u", b"\x1b[?1049l", b"\x1b[?2004l"]:
        assert code in transcript, f"Missing terminal lifecycle sequence: {code!r}"
    for modifier in [1, 3, 4, 9]:
        assert f"\x1b[{modifier}m".encode() in transcript
    assert termios.tcgetattr(slave) == original
    print("Style PTY smoke passed: enhanced Ctrl+I, legacy Tab, bold/underline, Shift+F10, selection, retained draft, monochrome styles, terminal restoration")
finally:
    if process.poll() is None:
        process.kill()
        process.wait()
    os.close(master)
    os.close(slave)
