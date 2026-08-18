# End-to-end tests

One run, one story: a database created empty, the real `apiplant` binary serving
an example app, and a browser walking the [admin dashboard](../docs/admin.md)
from the sign-in screen to a deleted record — asserting each step both on screen
and against the same server's REST API.

```bash
cd e2e
pnpm install
npx playwright install chromium   # once

pnpm test                          # headless
pnpm test:headed                   # watch it happen in a real window
```

`pnpm test:headed` (or `HEADED=1 pnpm test`) opens a visible browser and slows
the run down so it can be followed; `SLOW_MO=600` slows it further. Everything
else is Playwright's own CLI — `pnpm test --debug` steps through, `--ui` opens
the runner.

## What it proves

| Step | The claim |
|------|-----------|
| boot | migrations build the schema from `resources/` alone; `/_health` and the resource answer on an empty database; the manifest carries the app's `[app] name` |
| registration | `auth.allow_registration`, a real user row, a session token that logs in over the API too |
| onboarding | creating an organisation makes the creator its admin, via a `membership` the server stamps |
| navigation | the sidebar offers the app's resources and its non-private functions, and leaves the auth resources to their own screens |
| create / edit | a form writes through the CRUD API — verified by reading the row back |
| list & search | pagination hints, and the configured search fields matched as a case-insensitive substring (`?search=`) |
| actions | a function's `Input` type becomes a form (doc comments become help text), `functions/greet.toml` config reaches the handler, and results render |
| visibility | `Public` runs anonymously, `Authenticated` answers `401` without a credential |
| API keys | a key minted in the dashboard authenticates against the API; a forged one does not |
| delete | the row is gone from the API, not only from the table |
| sign out / in | the stored session is cleared, and the work is still there on return |

## How it runs

`scripts/start-app.sh` is Playwright's `webServer`. It reads the example's own
`main.toml` for the database URL, **drops and recreates that database**, builds
the binary and the app's functions, and runs the server. Playwright waits on
`/_health` before the first test and stops the server after the last.

⚠️ The database named in the example's `main.toml` is destroyed on every run.
For `examples/07-functions` that is `apiplant_functions` on the local
development cluster (`127.0.0.1:5432`).

## Another example

The suite's assertions are written against `examples/07-functions`, but the
harness is not:

```bash
APP_DIR=examples/13-real-world APP_ORIGIN=http://127.0.0.1:8099 pnpm test
```

`APP_DIR`, `APP_ORIGIN` and `APP_BASE_PATH` are the three knobs; the start
script takes the database from whichever app you name.

## The documentation's screenshots

Every picture in [docs/admin.md](../docs/admin.md),
[docs/studio.md](../docs/studio.md) and [docs/cli.md](../docs/cli.md) is taken
by this harness, not drawn — so a screenshot cannot go quietly stale after an
interface change. Retake them all with:

```bash
pnpm shots
```

| Script | What it photographs |
|--------|---------------------|
| `shots:admin` | the dashboard of `examples/13-real-world`, seeded — home, a list, a record, an action, Team, an API key |
| `shots:back-office` | the two back-office screens, which need `examples/27-back-office` because only it sets `global_admin_role` |
| `shots:studio` | the studio, against `examples/13-real-world` |
| `shots:cli` | `apiplant cli` — connecting, a list, a record, a function and its result, Team, Session, the key map |
| `shots:optimize` | quantises the PNGs to a palette, roughly a third of the bytes for no visible loss (needs Pillow; skipped without it) |

The pictures land in `docs/images/`, at a 2× device pixel ratio so they survive
a retina screen. `screenshots.config.ts` reuses `scripts/start-app.sh` with
`APP_SEED=1` — the test suite wants an empty database, the screenshots want a
populated one — and reuses a server that is already up, since capturing is
iterative.

The studio has no server to drive: it edits a directory through the
[File System Access API][fsa], whose picker is a native dialog no automation
can reach. `shots/fs-access-shim.ts` installs an in-memory implementation of
exactly the surface `studio/src/lib/fs.ts` uses, seeded with an example read off
disk. The studio is not modified and cannot tell — and since writes land in the
object graph rather than the filesystem, photographing a checked-in example
cannot change it.

The console has no browser to drive either, and for the opposite reason: it is a
terminal application. `scripts/cli-shots.py` runs the real binary in a pty of a
fixed size and sends it the keystrokes a reader would send. Two things follow
from a terminal rather than a page:

* **Reading the screen takes a terminal.** The console repaints only the cells
  that changed, so the bytes it emits never spell out what is displayed — the
  driver keeps its own emulator ([pyte][pyte]) fed with the same bytes, and
  decides where it is from that. Every shot asserts what should be on screen
  before it is taken.
* **Nothing captured is a picture.** What each step saves is the output stream
  so far, escape sequences and all; `scripts/render-ansi.mjs` replays it through
  xterm.js in a headless browser and photographs that. The palette, the font and
  the size are decided there, so restyling the pictures costs a rerun of the
  renderer rather than another pass over the running app.

The run points `APIPLANT_CONFIG_DIR` at a throwaway directory, so it always
starts from the connect screen and never touches the credentials of whoever is
running it.

[fsa]: https://developer.mozilla.org/en-US/docs/Web/API/File_System_API
[pyte]: https://pypi.org/project/pyte/

## Requirements

`cargo`, a PostgreSQL cluster reachable at the URL in the example's
`main.toml` with `psql`/`createdb` on PATH, Node 20+, and the Chromium
Playwright downloads. The screenshots additionally want Python with
[`pyte`][pyte] (to drive the console) and [Pillow][pillow] (to shrink the
PNGs) — `pip install pyte pillow`.

The functions in example 07 are Rust, so no extra toolchain is needed; examples 09–11 would want `cc`, `zig` and `go`.

[pillow]: https://pypi.org/project/pillow/
