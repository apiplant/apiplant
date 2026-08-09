# The apiplant skill

`apiplant-app/` is a [Claude
skill](https://code.claude.com/docs/en/skills) for building apiplant apps: the
workflow, the full documentation as reference material, and every example app.
It is generated from `docs/` and `examples/` by
[`scripts/generate-skill.py`](../scripts/generate-skill.py) — edit those, not
this directory.

## Install

This repository is also a [plugin
marketplace](https://code.claude.com/docs/en/plugin-marketplaces) —
[`.claude-plugin/marketplace.json`](../.claude-plugin/marketplace.json) offers
the skill as the `apiplant-app` plugin. In Claude Code:

```
/plugin marketplace add apiplant/apiplant
/plugin install apiplant-app@apiplant
```

`/plugin update apiplant-app` picks up later releases; `/plugin uninstall`
removes it. The `owner/repo` shorthand clones over SSH — if you have no key on
GitHub, add the marketplace as
`https://github.com/apiplant/apiplant.git` instead.

A skill is only a directory, so copying works too, and is the way to vendor it
into a project that should carry its own:

```bash
mkdir -p .claude/skills
cp -r /path/to/apiplant/skills/apiplant-app .claude/skills/
```

From a clone of this repository, `./scripts/generate-skill.py --install` writes
straight to `~/.claude/skills/apiplant-app`.

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
