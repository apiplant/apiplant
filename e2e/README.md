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

## Requirements

`cargo`, a PostgreSQL cluster reachable at the URL in the example's
`main.toml` with `psql`/`createdb` on PATH, Node 20+, and the Chromium
Playwright downloads. The functions in example 07 are Rust, so no extra
toolchain is needed; examples 09–11 would want `cc`, `zig` and `go`.
