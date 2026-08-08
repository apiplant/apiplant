# 01 · Hello world

The smallest app there is: **one config file and nothing else**.

```
01-hello-world/
└── main.toml
```

There are no resources, yet the server still starts, connects to Postgres, migrates,
serves interactive docs, and exposes the built-in resources every app gets
(`organization`, `membership`, `user`, `api_key`, `oauth_connection`).

## Run it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_hello
cargo run -p apiplant -- run examples/01-hello-world
```

```bash
curl -s localhost:8099/api/_health
# → {"status":"ok","framework":"apiplant"}
```

Open <http://127.0.0.1:8099/api/docs> for Swagger UI over the generated spec.

Registration works out of the box, because `user` is a built-in resource:

```bash
curl -s -XPOST localhost:8099/api/auth/register \
  -H 'content-type: application/json' \
  -d '{"email":"you@example.com","password":"pw"}'
# → {"token":"…","user":{"id":"…","email":"you@example.com"}}
```

## What `main.toml` does

| Section | Purpose |
|---------|---------|
| `[server]` | bind address, port, and the `base_path` everything is mounted under |
| `[database]` | connection URL; `auto_migrate` reconciles tables with your resources each boot |
| `[auth]` | JWT signing secret for sessions (set a real one in production) |
| `[docs]` | the Swagger UI page and its title |

Even this file is optional — with no `main.toml` the server binds `0.0.0.0:8080`
at base path `/` and builds a database URL from defaults. See
[Configuration](../../docs/configuration.md).

**Next:** [02 · Resources](../02-resources) adds your first table.
