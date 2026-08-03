//! `apiplant init` — write a new app directory.
//!
//! Two ways to start. Without a template it writes a small sample app: an
//! organisation-scoped `note` resource, the rows to sign in with, and one
//! function, which is enough to run `apiplant seed` and then `apiplant run` and
//! see a working API. With `--from <REPO>` it clones a git repository instead
//! and drops its `.git`, so a team's own starting point is as close to hand as
//! the built-in one.
//!
//! The directory must be empty (or absent). Refusing to write into a directory
//! with anything in it is the whole safety story here: `init` writes files
//! someone else's `main.toml` may be sitting on, and there is no undo.

use anyhow::{bail, Context as _};
use std::path::Path;

/// What `init` was asked to produce.
#[derive(Debug)]
pub struct Options {
    /// A git repository to clone instead of writing the sample app.
    pub from: Option<String>,
    /// The branch, tag or commit to check out. Only with `from`.
    pub branch: Option<String>,
    /// The app's name in `main.toml`. Defaults to the directory's name.
    pub name: Option<String>,
}

/// Create the app directory at `dir`.
pub fn init(dir: &Path, options: Options) -> anyhow::Result<()> {
    ensure_empty(dir)?;

    match options.from.as_deref() {
        Some(repo) => clone(dir, repo, options.branch.as_deref())?,
        None => {
            if options.branch.is_some() {
                bail!("`--branch` only applies with `--from <REPO>`");
            }
            scaffold(dir, &app_name(dir, options.name.as_deref()))?;
        }
    }

    let shown = dir.display();
    println!("\nCreated an apiplant app in {shown}\n");
    println!("Next:");
    println!("  apiplant seed {shown}     # create the tables and the first rows");
    println!("  apiplant run  {shown}     # serve it, with the dashboard at /admin/");
    Ok(())
}

/// The app's name: what was asked for, else the directory's own name.
fn app_name(dir: &Path, given: Option<&str>) -> String {
    if let Some(name) = given {
        return name.to_string();
    }
    // `.` and `..` have no useful file name of their own; ask the filesystem.
    let absolute = dir.canonicalize();
    let path = absolute.as_deref().unwrap_or(dir);
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "apiplant-app".to_string())
}

/// A new app goes into an empty directory or none at all.
fn ensure_empty(dir: &Path) -> anyhow::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    if !dir.is_dir() {
        bail!("{} exists and is not a directory", dir.display());
    }
    let mut entries = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(Result::ok)
        // A directory holding nothing but `.git` is the usual "I made the repo
        // first" case, and initialising into it is what the user meant.
        .filter(|entry| entry.file_name() != ".git");

    if entries.next().is_some() {
        bail!(
            "{} is not empty — `init` writes a whole app directory and will not \
             write over what is already there",
            dir.display()
        );
    }
    Ok(())
}

/// `--from <REPO>`: shallow-clone a template and make it the user's own.
fn clone(dir: &Path, repo: &str, branch: Option<&str>) -> anyhow::Result<()> {
    println!("cloning {repo}");

    let mut command = std::process::Command::new("git");
    command.arg("clone").arg("--depth").arg("1");
    if let Some(branch) = branch {
        command.arg("--branch").arg(branch);
    }
    command.arg(repo).arg(dir);

    let status = command.status().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("`--from` needs git on PATH, and it isn't there")
        } else {
            anyhow::Error::new(error).context("running git clone")
        }
    })?;

    if !status.success() {
        bail!("git clone failed — is `{repo}` a repository you can read?");
    }

    // The template's history is the template author's, not this app's. Dropping
    // it leaves a plain directory the user can `git init` as their own.
    let git_dir = dir.join(".git");
    if git_dir.exists() {
        std::fs::remove_dir_all(&git_dir)
            .with_context(|| format!("removing {}", git_dir.display()))?;
    }

    if !dir.join("main.toml").exists() && !dir.join("models").is_dir() {
        // Not fatal: a template may be bare on purpose, and the clone already
        // succeeded. Saying so beats the user finding out from `run`.
        println!(
            "note: {} has no main.toml and no models/ — it may not be an app directory",
            dir.display()
        );
    }
    Ok(())
}

/// Write the sample app, file by file.
fn scaffold(dir: &Path, name: &str) -> anyhow::Result<()> {
    // A database name from the app name: Postgres would take the quoted form,
    // but a URL with punctuation in it is a bad first impression.
    let database: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();

    for (path, contents) in [
        ("main.toml", main_toml(name, &database)),
        ("models/note.toml", NOTE_TOML.to_string()),
        ("seed/organization.toml", SEED_ORGANIZATION.to_string()),
        ("seed/user.toml", SEED_USER.to_string()),
        ("seed/membership.toml", SEED_MEMBERSHIP.to_string()),
        ("seed/note.toml", SEED_NOTE.to_string()),
        ("functions/greet.rs", GREET_RS.to_string()),
        ("README.md", readme(name)),
        (".gitignore", GITIGNORE.to_string()),
    ] {
        write(dir, path, &contents)?;
    }
    Ok(())
}

fn write(dir: &Path, relative: &str, contents: &str) -> anyhow::Result<()> {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
    println!("  {relative}");
    Ok(())
}

fn main_toml(name: &str, database: &str) -> String {
    format!(
        r#"# The server, the database and the docs. Every section is optional —
# delete any of them and a default takes over.

[app]
name = "{name}"

[server]
port = 8099
base_path = "/api"

[database]
# $DATABASE_URL wins when it is set, so the committed file holds no
# credentials and a deployment configures itself with the environment.
url = "${{DATABASE_URL:-postgres://postgres@127.0.0.1:5432/{database}}}"
auto_migrate = true

[auth]
# Change this before anyone else can reach the server: it signs the session
# tokens. `${{JWT_SECRET}}` reads it from the environment instead.
jwt_secret = "change-me-before-you-deploy"

[docs]
enabled = true
path = "/docs"
title = "{name}"
"#
    )
}

const NOTE_TOML: &str = r#"# One resource: a table, five endpoints, and a permission on each.
#
#   GET    /api/note        list        POST   /api/note        create
#   GET    /api/note/{id}   read        PATCH  /api/note/{id}   update
#                                       DELETE /api/note/{id}   delete
#
# There is no `id`, `created_at` or `updated_at` below because every resource
# gets those. Add a field here and the column appears on the next boot.

[resource]
name = "note"

[permissions]
# public | authenticated | member | owner | role:<name> | private
list   = "member"
read   = "member"
create = "member"
update = "owner"          # only the row's owner may edit it
delete = "role:admin"     # only admins of the active organisation

[fields.title]
type       = "string"
required   = true
max_length = 200

[fields.body]
type = "text"

[fields.pinned]
type    = "boolean"
default = false

[fields.owner_id]
type       = "reference"
references = "user"       # stamped by the server on create
"#;

const SEED_ORGANIZATION: &str = r#"# The organisation the app starts with.
#
# `id = "acme"` is a name, not a UUID: seeding hashes it into the same id every
# time, so the files below can point at this row by word, and running
# `apiplant seed` twice inserts it once.

[[row]]
id = "acme"
name = "Acme, Inc."
slug = "acme"
"#;

const SEED_USER: &str = r#"# Someone to sign in as: admin@example.com / password.
#
# The password is hashed with argon2 on the way in, and never comes back out of
# the API. Change it — or this row — before this app is anywhere public.

[[row]]
id = "admin"
email = "admin@example.com"
password = "password"
display_name = "Ada Admin"
"#;

const SEED_MEMBERSHIP: &str = r#"# What makes that user an administrator of Acme: `role:admin` permissions are
# checked against these rows, one per (user, organisation) pair.

[[row]]
id = "admin-at-acme"
user_id = "admin"
organization_id = "acme"
role = "admin"
"#;

const SEED_NOTE: &str = r#"# A row to look at, so the first GET returns something.

[[row]]
id = "welcome"
organization_id = "acme"
owner_id = "admin"
title = "Welcome"
body = "Edit models/note.toml, then run `apiplant run .` again."
pinned = true
"#;

const GREET_RS: &str = r#"//! An apiplant function: a separately compiled library, mounted as an endpoint.
//!
//!   apiplant build .      # cargo wraps this file and drops libgreet.so beside it
//!   curl -XPOST localhost:8099/api/functions/greet -d '{"name":"world"}'
//!
//! The server never links this in: it loads the library at boot over a stable C
//! ABI, so you can ship a new one without rebuilding apiplant.

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
struct Input {
    /// Who to greet.
    name: String,
}

#[derive(Serialize, JsonSchema)]
struct Output {
    /// The composed greeting.
    message: String,
}

fn greet(ctx: &Context<()>, input: Input) -> Result<Output, String> {
    ctx.info("greet invoked");

    // The host's database and the caller are both one call away:
    //   let rows = ctx.query("SELECT id FROM apiplant_note LIMIT 1", &[])?;
    //   let who = ctx.principal_id();

    Ok(Output {
        message: format!("Hello, {}!", input.name),
    })
}

apiplant_function::function! {
    name: "greet",
    description: "Say hello — this text shows up in the generated OpenAPI docs.",
    method: Post,
    visibility: Public,   // public | authenticated | role-gated | private
    handler: greet,
}
"#;

const GITIGNORE: &str = r#"# Built function libraries: `apiplant build` produces them from the sources
# beside them, so they are output, not source.
functions/**/*.so
functions/**/*.dylib
functions/**/*.dll
functions/**/target/
.apiplant-build/

# TLS material, if you put any here.
https/

# The baked admin panel, if you run `apiplant admin`.
admin/
"#;

fn readme(name: &str) -> String {
    format!(
        r#"# {name}

An [apiplant](https://framework.apiplant.com) app. The directory *is* the
application: there is no server code here, only the files the `apiplant` binary
reads at boot.

```
main.toml            server, database, auth and docs
models/note.toml     a resource → a table and five REST endpoints
seed/                the rows the app starts with
functions/greet.rs   a compiled plugin, mounted at /api/functions/greet
```

## Running it

You need a Postgres you can reach — the URL is in `main.toml`, and
`$DATABASE_URL` overrides it.

```bash
apiplant seed .      # migrate, then load seed/
apiplant build .     # compile functions/greet.rs (needs cargo on PATH)
apiplant run .       # serve on http://127.0.0.1:8099/api
```

Then sign in as `admin@example.com` / `password`:

```bash
curl -XPOST localhost:8099/api/auth/login \
  -H 'content-type: application/json' \
  -d '{{"email":"admin@example.com","password":"password"}}'

curl localhost:8099/api/note \
  -H "authorization: Bearer $TOKEN" \
  -H 'x-organization: acme'
```

The operator dashboard is at <http://127.0.0.1:8099/admin/>, the OpenAPI docs at
`/api/docs`, and `apiplant cli .` is the same dashboard in a terminal.

## Changing it

Add a field to `models/note.toml` and restart: migrations are additive and
automatic, so the column appears. Add another `models/*.toml` and you have a
second resource. The full reference is at <https://framework.apiplant.com/docs>.

**Before this is public**: change `auth.jwt_secret` in `main.toml`, and the
seeded password in `seed/user.toml`. See
<https://framework.apiplant.com/docs/security>.
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory that cleans itself up.
    fn temp(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "apiplant-init-{label}-{}",
            std::process::id() as u64 + label.len() as u64
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn the_sample_app_is_written_and_names_itself_after_the_directory() {
        let dir = temp("sample");
        init(
            &dir,
            Options {
                from: None,
                branch: None,
                name: None,
            },
        )
        .expect("init");

        for file in [
            "main.toml",
            "models/note.toml",
            "seed/user.toml",
            "functions/greet.rs",
            "README.md",
        ] {
            assert!(dir.join(file).is_file(), "missing {file}");
        }

        let main = std::fs::read_to_string(dir.join("main.toml")).unwrap();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(main.contains(&format!(r#"name = "{name}""#)), "{main}");
        // The database name is the app name with the punctuation flattened.
        assert!(main.contains(&name.replace('-', "_")), "{main}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_directory_with_anything_in_it_is_refused() {
        let dir = temp("occupied");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.toml"), "[app]\n").unwrap();

        let error = init(
            &dir,
            Options {
                from: None,
                branch: None,
                name: None,
            },
        )
        .expect_err("should refuse");
        assert!(error.to_string().contains("not empty"), "{error}");

        // …but a directory holding only a fresh `.git` is fine.
        std::fs::remove_file(dir.join("main.toml")).unwrap();
        std::fs::create_dir(dir.join(".git")).unwrap();
        init(
            &dir,
            Options {
                from: None,
                branch: None,
                name: Some("named".into()),
            },
        )
        .expect("init into a fresh repo");
        assert!(dir.join("main.toml").is_file());

        std::fs::remove_dir_all(&dir).ok();
    }
}
