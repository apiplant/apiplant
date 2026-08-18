#!/usr/bin/env python3
"""Drive `apiplant cli` in a pseudo-terminal and record what it draws.

The console is a full-screen terminal application, so there is no DOM to
photograph and no browser to drive. Instead this opens a pty of a fixed size,
runs the real binary against the real seeded server, sends the keystrokes a
reader would send, and saves the raw output stream at each interesting moment.

What lands in `.cli-shots/` is the byte stream from the program's start up to
that moment, escape sequences and all — not a picture. `render-ansi.mjs` replays
each one through a terminal emulator to produce the PNG. Keeping the capture and the
rendering apart means the pictures can be restyled without running the app
again.

The console remembers its credential per server, so this points
APIPLANT_CONFIG_DIR at a throwaway directory: the run always starts from the
connect screen, and it never reads or writes the credentials of whoever is
running it.
"""

from __future__ import annotations

import fcntl
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import termios
import time
import urllib.error
import urllib.request
from pathlib import Path

try:
    import pyte
except ImportError:  # pragma: no cover - a setup problem, not a runtime one
    raise SystemExit(
        "cli-shots: needs pyte to read the screen it is driving — `pip install pyte`"
    )

ROOT = Path(__file__).resolve().parents[2]
E2E = ROOT / "e2e"
OUT = E2E / ".cli-shots"
CONFIG_DIR = OUT / "config"

APP_DIR = os.environ.get("APP_DIR", "examples/13-real-world")
ORIGIN = os.environ.get("APP_ORIGIN", "http://127.0.0.1:8099")
BASE_PATH = os.environ.get("APP_BASE_PATH", "/api")
EMAIL = os.environ.get("SHOTS_EMAIL", "admin@example.com")
PASSWORD = os.environ.get("SHOTS_PASSWORD", "password")

# Wide enough for a sidebar beside a table with room for its columns, short
# enough that the picture stays legible in a page of prose.
COLS, ROWS = 110, 32

DEBUG = "--debug" in sys.argv

DOWN, UP, ENTER, ESC, TAB = "\x1b[B", "\x1b[A", "\r", "\x1b", "\t"


# --- the server -------------------------------------------------------------


def healthy() -> bool:
    try:
        with urllib.request.urlopen(f"{ORIGIN}{BASE_PATH}/_health", timeout=2) as r:
            return r.status == 200
    except (urllib.error.URLError, OSError):
        return False


def start_server() -> subprocess.Popen | None:
    """Bring the app up, unless one is already answering.

    Reuses a running server the way the browser screenshots do: capturing is
    iterative, and a rebuild between every attempt is most of the wall clock.
    """
    if healthy():
        print(f"cli-shots: reusing the server at {ORIGIN}")
        return None

    print(f"cli-shots: starting {APP_DIR}")
    proc = subprocess.Popen(
        ["bash", str(E2E / "scripts" / "start-app.sh")],
        cwd=E2E,
        env={**os.environ, "APP_DIR": APP_DIR, "APP_SEED": "1"},
        stdout=subprocess.DEVNULL if not DEBUG else None,
        stderr=subprocess.STDOUT if not DEBUG else None,
        start_new_session=True,
    )
    deadline = time.time() + 600
    while time.time() < deadline:
        if healthy():
            return proc
        if proc.poll() is not None:
            raise SystemExit("cli-shots: the server exited before it was ready")
        time.sleep(1)
    raise SystemExit("cli-shots: the server never became ready")


# --- the console ------------------------------------------------------------


class Console:
    """A running `apiplant cli`, and everything it has drawn so far."""

    def __init__(self, target: str) -> None:
        self.stream = bytearray()
        # The console repaints only the cells that changed, so the bytes alone
        # never spell out the current screen — the last frame in the stream is a
        # handful of edits to a picture that exists only in the terminal. To
        # decide anything from what is displayed, the driver has to keep a
        # terminal of its own and feed it the same bytes.
        self.screen = pyte.Screen(COLS, ROWS)
        self.emulator = pyte.ByteStream(self.screen)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:  # child
            env = {
                **os.environ,
                "TERM": "xterm-256color",
                "COLORTERM": "truecolor",
                "COLUMNS": str(COLS),
                "LINES": str(ROWS),
                "APIPLANT_CONFIG_DIR": str(CONFIG_DIR),
            }
            binary = str(ROOT / "target" / "debug" / "apiplant")
            os.execvpe(binary, ["apiplant", "cli", target], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        self.settle()

    def settle(self, seconds: float = 0.5) -> None:
        """Read for a while, and keep everything read.

        Not "read until it goes quiet": the console repaints on a timer, so it
        is never quiet, and waiting for silence would wait forever. A fixed
        window is what is left — generous where a keystroke costs a request,
        brief where it only moves a cursor.
        """
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([self.fd], [], [], 0.05)
            if not ready:
                continue
            try:
                chunk = os.read(self.fd, 65536)
            except OSError:
                return
            if not chunk:
                return
            self.stream.extend(chunk)
            self.emulator.feed(chunk)

    def press(self, *keys: str, wait: float = 0.5) -> "Console":
        for key in keys:
            os.write(self.fd, key.encode())
            self.settle(wait)
        return self

    def type(self, text: str) -> "Console":
        return self.press(text, wait=0.3)

    def shoot(self, name: str) -> "Console":
        # A little longer before a picture than between keystrokes: this is the
        # frame that ends up in the documentation.
        self.settle(1.2)
        (OUT / f"{name}.ansi").write_bytes(bytes(self.stream))
        print(f"  {name}")
        if DEBUG:
            print("\n".join(self.lines()))
        return self

    def lines(self) -> list[str]:
        """What is on the screen, one string per row."""
        return self.screen.display

    def text(self) -> str:
        return "\n".join(self.lines())

    def expect(self, pattern: str) -> "Console":
        """Fail loudly when the screen is not the one being photographed.

        Every shot is a claim about what the console shows, and a wrong turn
        earlier in the run produces a plausible-looking picture of the wrong
        screen. Checking is what keeps a stale screenshot from reaching the
        documentation unnoticed.
        """
        if not re.search(pattern, self.text()):
            preview = "\n".join(line for line in self.lines() if line.strip())
            raise SystemExit(f"cli-shots: expected /{pattern}/ on screen, got:\n{preview}")
        return self

    def close(self) -> None:
        try:
            os.write(self.fd, b"\x03")
            time.sleep(0.4)
        except OSError:
            pass
        try:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        except (ProcessLookupError, ChildProcessError):
            pass
        os.close(self.fd)


def sidebar_width(console: Console) -> int:
    """The column the sidebar's box ends at, read off its own border."""
    for line in console.lines():
        if "Navigate" in line:
            return line.index("\u256e") + 1
    raise SystemExit("cli-shots: no sidebar on screen")


def sidebar_to(console: Console, label: str) -> Console:
    """Move the sidebar's cursor onto an entry, by what the entry says.

    The sidebar is grouped, and both the groups and their order come from the
    app's own `[admin]` settings — so a fixed number of keystrokes would point
    somewhere else the moment an example gained a resource. Reading the screen
    instead lets the walk name its destination.

    It rewinds to the top and then goes one way: for an app with more entries
    than rows the sidebar scrolls, and under a scrolling list an entry's
    position on screen moves as the cursor does, so heading for where it
    appears to be oscillates forever at the bottom edge.
    """
    ensure_sidebar(console)
    for _ in range(60):
        before = current_entry(console)
        console.press(UP, wait=0.1)
        if current_entry(console) == before:
            break  # the top

    seen: list[str] = []
    for _ in range(80):
        here = current_entry(console)
        if here == label:
            return console
        if here is not None and here not in seen:
            seen.append(here)
        console.press(DOWN, wait=0.12)
    raise SystemExit(f"cli-shots: never reached {label!r}; the sidebar offers {seen}")


def ensure_sidebar(console: Console) -> Console:
    """Put focus on the sidebar, whichever half currently has it.

    Focus is worth testing rather than inferring: the sidebar keeps showing
    where its cursor was left while the pane is being used, so the highlight
    alone does not say who is listening. Moving the cursor and watching whether
    it moves does.
    """
    for _ in range(4):
        if sidebar_listening(console):
            return console
        console.press(TAB, wait=0.5)
    raise SystemExit("cli-shots: could not get back to the sidebar")


def sidebar_listening(console: Console) -> bool:
    """Whether the sidebar has focus — asked by nudging it and putting it back."""
    before = current_entry(console)
    for key, back in ((DOWN, UP), (UP, DOWN)):  # the second covers an end of the list
        console.press(key, wait=0.15)
        if current_entry(console) != before:
            console.press(back, wait=0.15)
            return True
    return False


def sidebar_rows(console: Console) -> list[tuple[str, bool]]:
    """The sidebar as it stands: every visible row, and whether it is current.

    Which entry is current is carried by the highlight rather than by any
    character, so this reads cell attributes and not text.
    """
    width = sidebar_width(console)
    rows: list[tuple[str, bool]] = []
    for y, line in enumerate(console.lines()):
        if not line.startswith("\u2502"):  # only rows inside the sidebar's box
            continue
        text = line[:width].strip(" \u2502")
        if not text or text.isupper():  # group headings are not selectable
            continue
        cells = console.screen.buffer[y]
        highlighted = any(
            cells[x].reverse or cells[x].bg != "default" for x in range(1, width - 1)
        )
        rows.append((text, highlighted))
    return rows


def current_entry(console: Console) -> str | None:
    """The entry the sidebar's cursor is on, or None when the pane has focus."""
    for text, highlighted in sidebar_rows(console):
        if highlighted:
            return text
    return None


def first_entry(console: Console) -> str | None:
    rows = sidebar_rows(console)
    return rows[0][0] if rows else None


# --- the run ----------------------------------------------------------------


def main() -> int:
    server = start_server()
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)

    console = Console(ORIGIN)
    try:
        # Connecting: the first screen of a console that has never run here.
        console.expect(r"Connect this console to the app").shoot("cli-connect")

        # Sign in with an email and password — the second offer. Its form is a
        # field at a time: enter opens one, enter commits it, down moves on.
        console.press(DOWN, ENTER)
        console.expect(r"Sign in").shoot("cli-sign-in")
        console.press(ENTER).type(EMAIL).press(ENTER, DOWN, ENTER)
        console.type(PASSWORD).press(ENTER, DOWN, ENTER, wait=1.2)
        console.expect(EMAIL)

        # The seeded trade belongs to Acme; a fresh session lands in the
        # personal organization every account is created with.
        console.press("O").press(DOWN, ENTER, wait=1.2)
        console.expect(r"Acme").shoot("cli-home")

        # A resource: the list, one record, and that record's children.
        sidebar_to(console, "Orders").press(TAB, wait=1.0)
        console.expect(r"Orders").shoot("cli-list")
        console.press(ENTER, wait=1.0).shoot("cli-record")
        console.press("c", wait=1.0).shoot("cli-children")
        console.press(ESC, ESC)

        # A function: the form built from its input schema, and what it
        # returns. `enter` on a field opens it for editing, so running the
        # thing means moving past the fields onto the button first.
        sidebar_to(console, "Sales summary").press(TAB, wait=1.0)
        console.expect(r"Sales summary").shoot("cli-function")
        console.press(DOWN).press(ENTER, wait=2.5)
        console.shoot("cli-function-result")

        # The Console group: the screens that are not tables of rows.
        console.press(ESC)
        sidebar_to(console, "Team").press(TAB, wait=1.0)
        console.expect(r"@").shoot("cli-team")
        console.press(ESC)
        sidebar_to(console, "Session").press(TAB, wait=1.0)
        console.expect(EMAIL).shoot("cli-session")

        # The key map, over whatever is behind it.
        console.press("?", wait=0.8).shoot("cli-keys")
    finally:
        console.close()
        if server is not None:
            os.killpg(os.getpgid(server.pid), signal.SIGTERM)

    print(f"cli-shots: {len(list(OUT.glob('*.ansi')))} captures in {OUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
