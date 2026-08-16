#!/usr/bin/env python3
"""Generate the `apiplant-app` Claude skill from this repository's documentation.

The skill is assembled, not hand-maintained: SKILL.md carries the workflow and a
map of what to read when, `references/` is the docs directory verbatim, and
`examples/` is a copy of the runnable example apps. Regenerate it whenever the
docs change.

    ./scripts/generate-skill.py                  # -> skills/apiplant-app/ (committed)
    ./scripts/generate-skill.py --out ./somewhere
    ./scripts/generate-skill.py --install        # -> ~/.claude/skills/apiplant-app
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DOCS = REPO / "docs"
EXAMPLES = REPO / "examples"

SKILL_NAME = "apiplant-app"
DEFAULT_OUT = REPO / "skills" / SKILL_NAME


# Which guide answers which question. Order is the reading order, not the
# alphabetical one: this table is the skill's routing table.
GUIDES = [
    ("resources.md", "Define a resource: field types, options, scope, migrations"),
    ("configuration.md", "`main.toml`: server, database, auth, TLS, workers, env vars"),
    ("permissions.md", "Per-action policies, ownership, org roles"),
    ("relationships.md", "`reference` fields, `has_many`, `?expand=`, `on_delete`"),
    ("multitenancy.md", "Organisations, memberships, per-tenant isolation"),
    ("authentication.md", "Users, API keys, sessions, OAuth, extending `user`"),
    ("seed.md", "`seed/`: initial rows in TOML or CSV"),
    ("functions.md", "Compiled plugins over the stable ABI (Rust, C, Zig, Go, TypeScript)"),
    ("hooks.md", "Attaching functions to CRUD lifecycle events"),
    ("api-reference.md", "Every endpoint, query parameter and status code"),
    ("queues.md", "`publish` from a function, `[queues.subscribe]` on Postgres alone"),
    ("storage.md", "The `file` field type, directory or S3-compatible bucket"),
    ("email.md", "One `[email]` provider, `ctx.send_email`"),
    ("caching.md", "The optional `[cache]` Redis a function can reach"),
    ("payments.md", "Catalogue, subscriptions, checkout, tax"),
    ("ai.md", "`[ai]` provider, `agents/`, `ctx.chat`, streaming"),
    ("admin.md", "The built-in operator UI and `[admin]` config"),
    ("cli.md", "`apiplant cli`: the dashboard in a terminal"),
    ("security.md", "What to configure before exposing the server"),
    ("openapi.md", "The generated spec and Swagger UI"),
]

DESCRIPTION = (
    "Build a REST API application with apiplant — a directory of TOML resource "
    "definitions, permissions, seed data and compiled functions served by the "
    "apiplant binary. Use when creating or modifying an apiplant app: writing "
    "resources/*.toml, main.toml, seed data, agents, lifecycle hooks or "
    "functions, wiring auth, multitenancy, permissions, relationships, queues, "
    "storage, email, payments or AI, and when running apiplant init/seed/run/build."
)


def tracked(path: Path) -> list[Path]:
    """Files git tracks under `path`, relative to it.

    The examples carry build artefacts, uploaded files and local TLS material
    that .gitignore already keeps out of the repository. Copying only tracked
    files reuses that judgement instead of restating it as a second, drifting
    list of patterns.
    """
    out = subprocess.run(
        ["git", "-C", str(REPO), "ls-files", "-z", "--", str(path.relative_to(REPO))],
        capture_output=True, text=True, check=True,
    ).stdout
    return [Path(p).relative_to(path.relative_to(REPO))
            for p in out.split("\0") if p]


def title_of(md: Path) -> str:
    for line in md.read_text().splitlines():
        if line.startswith("# "):
            return line[2:].strip()
    return md.stem


def example_dirs() -> list[tuple[str, str]]:
    out = []
    for d in sorted(EXAMPLES.iterdir()):
        readme = d / "README.md"
        if d.is_dir() and readme.exists():
            heading = title_of(readme)
            out.append((d.name, re.sub(r"^\d+\s*[·.-]\s*", "", heading).strip()))
    return out


def yaml_scalar(s: str) -> str:
    """`s` as a single-quoted YAML scalar.

    The description contains `: `, which makes an unquoted scalar invalid YAML —
    the loader reads it as a nested mapping key. Quoting unconditionally keeps
    the frontmatter valid however the text is later reworded.
    """
    return "'" + s.replace("'", "''") + "'"


def skill_md(examples: list[tuple[str, str]]) -> str:
    guides = "\n".join(
        f"| [{f}](references/{f}) | {what} |" for f, what in GUIDES
    )
    example_rows = "\n".join(
        f"| `{name}` | {what} |" for name, what in examples
    )
    return f"""---
name: {SKILL_NAME}
description: {yaml_scalar(DESCRIPTION)}
---

# Building apiplant apps

apiplant serves an **app directory**. There is no server code: resources are
TOML, the database is migrated to match at boot, CRUD endpoints and their
permissions are generated, and compiled functions are loaded from disk.

```
my-app/
├── main.toml       # optional server/db/auth config; safe defaults if absent
├── https/          # cert + key here => the server runs HTTPS
├── resources/      # one <name>.toml per resource => a table + CRUD endpoints
├── seed/           # optional <resource>.toml|csv => initial rows
├── agents/         # optional <name>.toml per AI agent
└── functions/      # function sources, their config, and built libraries
```

## Workflow

1. **Scaffold.** `apiplant init <dir>` writes a sample app (one resource, seed
   rows, one function). For an existing directory, create `resources/` and add
   files to it — every part of the tree is optional.
2. **Model the data first.** One `resources/<name>.toml` per entity. Copy the
   shape from `examples/02-resources/resources/note.toml`, then check field
   types and options against [references/resources.md](references/resources.md).
   Do not invent field types — the list is closed.
3. **Decide scope before writing permissions.** `scope = "organization"` (the
   default) isolates rows per tenant; `scope = "global"` opts out. Read
   [references/multitenancy.md](references/multitenancy.md) once per app, not
   once per resource.
4. **Set every permission explicitly.** `list/read/create/update/delete` each
   take a policy. `"public"` is almost never right outside an example. See
   [references/permissions.md](references/permissions.md).
5. **Link resources with `reference` fields**, not with ad-hoc id columns —
   they produce real foreign keys, nested endpoints and `?expand=`.
6. **Seed.** Put an administrator and some rows in `seed/`. `apiplant seed
   <dir>` is re-runnable and does not duplicate.
7. **Add behaviour only where configuration cannot reach.** A function is a
   compiled library mounted as an endpoint; a hook is that function attached to
   a CRUD lifecycle event. Write plain `.rs` files and let `apiplant build`
   compile them. See [references/functions.md](references/functions.md) and
   [references/hooks.md](references/hooks.md).
8. **Run and verify.** `apiplant run <dir>` serves on
   `http://127.0.0.1:8099/api`. Confirm the endpoints with curl against
   [references/api-reference.md](references/api-reference.md) before declaring
   the app done.

## Commands

```bash
apiplant init  <dir>            # scaffold (also takes a git URL as a template)
apiplant build <dir>            # compile functions/ into loadable libraries
apiplant seed  <dir>            # migrate, then load seed/
apiplant run   <dir>            # serve the app
apiplant cli   <dir>            # the admin dashboard, in a terminal
```

A Postgres URL is required — `$DATABASE_URL`, or `[database] url` in
`main.toml`. Any string in any config file can reference the environment
(`url = "$DATABASE_URL"`, `region = "${{AWS_REGION:-eu-west-1}}"`), so committed
files hold no credentials.

## Rules that prevent the common failures

- `organization`, `membership`, `user`, `api_key` and `oauth_connection` exist
  already. To extend one, drop a file with the *same name* — do not define a
  parallel resource.
- Migrations are additive. A renamed field is a new column plus an orphan, not
  a rename; plan the schema before seeding production data.
- Functions are compiled artefacts. After editing a function source, `apiplant
  build` must run before `apiplant run` picks the change up.
- The container image has no shell or toolchain: build functions before
  mounting the directory.
- Before exposing a server, walk [references/security.md](references/security.md).

## Reference

Read the guide for the area you are touching; they are the authority, this file
is only the map.

| Guide | What's in it |
|-------|--------------|
{guides}

## Example apps

Complete, runnable apps under `examples/`, each introducing one concept. Read
the `README.md` inside one before copying from it.

| Example | Concept |
|---------|---------|
{example_rows}
"""


def check_guides() -> list[str]:
    """GUIDES must name every guide in docs/, and nothing else."""
    listed = {f for f, _ in GUIDES}
    present = {md.name for md in DOCS.glob("*.md")} - {"README.md"}
    return (
        [f"{f}: in GUIDES but missing from docs/" for f in sorted(listed - present)]
        + [f"{f}: in docs/ but missing from GUIDES in {Path(__file__).name}"
           for f in sorted(present - listed)]
    )


INSTALL_README = f"""# The apiplant skill

`{SKILL_NAME}/` is a [Claude
skill](https://code.claude.com/docs/en/skills) for building apiplant apps: the
workflow, the full documentation as reference material, and every example app.
It is generated from `docs/` and `examples/` by
[`scripts/generate-skill.py`](../scripts/generate-skill.py) — edit those, not
this directory.

## Install

This repository is also a [plugin
marketplace](https://code.claude.com/docs/en/plugin-marketplaces) —
[`.claude-plugin/marketplace.json`](../.claude-plugin/marketplace.json) offers
the skill as the `{SKILL_NAME}` plugin. In Claude Code:

```
/plugin marketplace add apiplant/apiplant
/plugin install {SKILL_NAME}@apiplant
```

`/plugin update {SKILL_NAME}` picks up later releases; `/plugin uninstall`
removes it. The `owner/repo` shorthand clones over SSH — if you have no key on
GitHub, add the marketplace as
`https://github.com/apiplant/apiplant.git` instead.

A skill is only a directory, so copying works too, and is the way to vendor it
into a project that should carry its own:

```bash
mkdir -p .claude/skills
cp -r /path/to/apiplant/skills/{SKILL_NAME} .claude/skills/
```

From a clone of this repository, `./scripts/generate-skill.py --install` writes
straight to `~/.claude/skills/{SKILL_NAME}`.

## Use

There is nothing to enable and no command to type. Claude reads the skill's
description, and loads it when the work looks like apiplant work:

> build me an apiplant app for tracking client invoices, one organisation per
> client, and a hook that stamps the due date 30 days out

> add a `reference` from comment to post and expose the nested endpoint

> why is my `before_create` hook not firing?

It works on an existing app directory as well as a new one — point Claude at the
directory and it reads your `resources/` before changing them.

Two things make the result better:

* **Say what the data is.** The skill's workflow models resources first and
  writes permissions second, so a sentence about who owns what is worth more
  than a sentence about endpoints — those are generated either way.
* **Let it run the app.** `apiplant seed` and `apiplant run` against a local
  Postgres turn "this should work" into "this works"; without a database Claude
  can only check the TOML by eye.

Ask it to explain a choice and it will cite the guide under `references/` that
made it — those are this repository's `docs/`, verbatim.
"""


def build(out: Path) -> None:
    if out.exists():
        shutil.rmtree(out)
    (out / "references").mkdir(parents=True)

    for md in sorted(DOCS.glob("*.md")):
        if md.name != "README.md":
            shutil.copy2(md, out / "references" / md.name)

    examples = example_dirs()
    for name, _ in examples:
        src = EXAMPLES / name
        for rel in tracked(src):
            dest = out / "examples" / name / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src / rel, dest)

    (out / "SKILL.md").write_text(skill_md(examples))

    files = sum(1 for _ in out.rglob("*") if _.is_file())
    lines = len((out / "SKILL.md").read_text().splitlines())
    print(f"{out}: SKILL.md ({lines} lines), "
          f"{len(list((out / 'references').glob('*.md')))} guides, "
          f"{len(examples)} examples, {files} files")
    if lines > 500:
        print("warning: SKILL.md exceeds 500 lines", file=sys.stderr)


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--out", type=Path, default=DEFAULT_OUT,
                   help="output directory (default: skills/%s)" % SKILL_NAME)
    p.add_argument("--install", action="store_true",
                   help="write to ~/.claude/skills/%s instead" % SKILL_NAME)
    args = p.parse_args()

    out = Path.home() / ".claude" / "skills" / SKILL_NAME if args.install else args.out
    if not DOCS.is_dir():
        print(f"error: {DOCS} not found", file=sys.stderr)
        return 1
    if problems := check_guides():
        for p in problems:
            print(f"error: {p}", file=sys.stderr)
        return 1
    build(out.resolve())
    if out == DEFAULT_OUT:
        (DEFAULT_OUT.parent / "README.md").write_text(INSTALL_README)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
