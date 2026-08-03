#!/usr/bin/env bash
#
# Bring up the app the tests drive: a database with nothing in it, the example's
# functions compiled, and the server running against both.
#
# Everything here is derived from the example itself — the database name and the
# port come out of its `main.toml` — so pointing APP_DIR at a different example
# is the only change needed to test a different one.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APP_DIR="${APP_DIR:-examples/07-functions}"
APP="$ROOT/$APP_DIR"
BIN="$ROOT/target/debug/apiplant"

[ -f "$APP/main.toml" ] || { echo "no app at $APP" >&2; exit 1; }

# --- read what the app says about itself ------------------------------------

toml_value() { # key -> the quoted string value under any section
  sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$APP/main.toml" | head -1
}

DB_URL="$(toml_value url)"
[ -n "$DB_URL" ] || { echo "main.toml has no [database] url" >&2; exit 1; }
DB_NAME="${DB_URL##*/}"
DB_NAME="${DB_NAME%%\?*}"
ADMIN_URL="${DB_URL%/*}/postgres"

# --- a database with nothing in it ------------------------------------------
#
# Dropping rather than truncating: `auto_migrate` then builds every table from
# the models as it would on a first deployment, so the migration path is part of
# what the run proves.

echo "e2e: resetting database $DB_NAME"
psql "$ADMIN_URL" -v ON_ERROR_STOP=1 -q -c \
  "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$DB_NAME' AND pid <> pg_backend_pid();" >/dev/null
psql "$ADMIN_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$DB_NAME\";"
psql "$ADMIN_URL" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE \"$DB_NAME\";"

# --- the binary, and the app's functions ------------------------------------

echo "e2e: building apiplant"
cargo build --manifest-path "$ROOT/Cargo.toml" -p apiplant

if [ -d "$APP/functions" ]; then
  echo "e2e: building $APP_DIR functions"
  "$BIN" build "$APP"
fi

echo "e2e: starting $APP_DIR"
exec "$BIN" run "$APP"
