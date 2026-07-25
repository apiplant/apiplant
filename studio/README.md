# apiplant studio

**A local editor for an apiplant app directory.** Point it at the folder you
would hand to the `apiplant` binary; it loads the resources, permissions, hooks
and functions that folder declares, lets you edit them as forms or as TOML, and
writes the result straight back to disk.

```bash
cd studio
pnpm install     # or npm install
pnpm dev         # http://localhost:5273
```

Then click **Open app directory** and choose an app — or choose a parent folder
like `examples/` and pick the app inside it.

There is no server and no build step for your app: the page holds a
[File System Access][fsa] handle to the directory you picked, and everything
happens in the browser. Chrome, Edge, Opera and Arc support that API; Firefox
and Safari do not, and the studio says so instead of half-working.

It wears apiplant.com's colours in both light and dark — the switch is in the
header, it starts from your system preference, and it is remembered.

## What it edits

| Piece | What the studio gives you |
|-------|---------------------------|
| `main.toml` | Every `[server]`, `[database]`, `[auth]` and `[docs]` key, with the framework's default shown as the placeholder. TLS is reported, not configured — it comes from `https/`. |
| `models/*.toml` | Fields (type, `required`/`unique`/`hidden`, `max_length`, `default`, `references`, `on_delete`), the five permission actions including `role:<name>`, tenancy scope, `table`, `owner_field`, timestamps, and `[auth]` on `user`. |
| Built-in resources | `organization`, `user`, `membership`, `api_key` and `oauth_connection` are listed with the definitions the framework ships. Editing one writes a `models/*.toml` that replaces the default — the framework's own override mechanism. |
| `[hooks]` | All ten events, with a picker over the function names the libraries in `functions/` actually export. A hook naming a function nothing exports is flagged. |
| `functions/` | New libraries in **Rust, C, Zig or Go**, as a single file or a directory (a crate, a Go module, a multi-file C or Zig project). Sources and per-function `<name>.toml` config are editable in place; compiled `lib*.so` artifacts are listed with their size and never touched. |

Every generated template compiles with `apiplant build` as written and mounts at
`<base>/functions/<name>`; the Rust one uses the `function!` macro, the others
export the four C symbols from [`apiplant.h`](../crates/apiplant-abi/include/apiplant.h).

## How saving works

Edits stay in memory until you press **Save** (or ⌘/Ctrl+S). The *Pending
changes* view lists exactly which files will be added, modified or removed, with
the full contents of each — nothing else in the directory is touched, including
compiled libraries, `https/` material and files the studio does not model.

Two things worth knowing:

* **Form edits rewrite the file.** A model file is re-emitted from the form, so
  hand-written comments in it are lost. The studio warns when the file on disk
  has comments; edit on the **TOML** tab to keep them. Every resource and the
  config have that tab, and it is the same file either way.
* **Deleting a resource deletes its file, not its table.** apiplant's migrations
  are additive by design; dropping columns or tables stays a deliberate act you
  perform in SQL.

The studio never builds or serves anything. `apiplant build` and `apiplant run`
stay where they were — the Overview page has the commands for the open app.

The CLI now also reuses this design system for a generated **static admin
panel**:

```bash
apiplant admin ./my-app --api https://api.example.com --out ./my-app/admin
```

That command does **not** read the directory at runtime. It bakes the app's
schema, permissions, auth rules and loaded function endpoints into a separate
frontend that talks to the live API, while leaving the studio itself focused on
editing app directories. When you write that bundle to `APP_DIR/admin`, the
server now serves it automatically at `/admin/`.

## Layout

```
src/
├── lib/
│   ├── types.ts       the resource/config model, mirroring apiplant-core's schema.rs
│   ├── toml.ts        parsing (smol-toml) and an emitter that writes apiplant's file layout
│   ├── builtins.ts    the five framework resources, transcribed from defaults.rs
│   ├── fs.ts          File System Access: pick, scan, write, delete
│   ├── functions.ts   reading functions/ the way `apiplant build` does
│   ├── templates.ts   the per-language function scaffolds
│   ├── store.ts       the open project: models, the file map, and pending changes
│   ├── theme.ts       light/dark, remembered, system preference by default
│   └── nav.ts         which page is showing
└── components/
    ├── CodeEditor.tsx CodeMirror 6 — Rust, C, Go, TOML, and a small Zig mode
    ├── ui.tsx         the shared primitives
    └── …              one file per page
```

`store.ts` is the piece to read first. Files are the single source of truth for
what gets saved: every form edits a model and immediately re-emits it into the
file map, so "what will change on disk" is answerable at any moment and
discarding is just a rescan.

Every colour is a CSS variable defined twice in `app.css` — once for each theme —
so components never mention light or dark, and the editor re-paints with the rest
of the page. Zig has no CodeMirror grammar, so `CodeEditor.tsx` carries a small
stream tokenizer for it.

Built with [Solid](https://solidjs.com), [Tailwind](https://tailwindcss.com) and
[CodeMirror](https://codemirror.net); `pnpm build` type-checks and emits a static
bundle to `dist/`.

[fsa]: https://developer.mozilla.org/en-US/docs/Web/API/File_System_API
