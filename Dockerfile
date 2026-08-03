# The apiplant server, built from source and shipped on a small Debian base.
#
# The front-end builds are tracked in `crates/apiplant-assets/assets`, so no
# pnpm stage is needed: a checkout compiles as-is.
#
#   docker build -t apiplant .
#   docker run --rm -p 8080:8080 -v "$PWD:/app" apiplant run /app
#
# Debian rather than Alpine because V8 (via deno_core) ships prebuilt static
# libraries for glibc targets only.

# Dependencies are built in their own layer, keyed on the manifests only, so a
# source edit does not rebuild them. That matters here more than usual: linking
# V8 (via deno_core) is most of the build, and it is entirely a dependency.
# cargo-chef is what makes the split possible — `prepare` reduces the workspace
# to a recipe naming every dependency, and `cook` builds just those against
# stub sources.
FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

# Only the recipe is copied in, so this layer is reused for as long as no
# dependency changes — the rest of the tree is invisible to it.
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# The V8 build downloads its prebuilt library over HTTPS; nothing else here
# needs a system package that the rust image does not already carry.
COPY . .
RUN cargo build --release --locked --bin apiplant


FROM debian:bookworm-slim

# ca-certificates: outbound HTTPS (email APIs, Stripe, AI providers) uses
# webpki roots for verification but still needs the store for `git`.
# git: `apiplant init --from <repo>` clones a template.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/apiplant /usr/local/bin/apiplant

# Compiling `functions/*.rs` into loadable libraries needs a Rust toolchain,
# which is not in this image — run `apiplant build` before mounting the app, or
# use TypeScript functions, which are transpiled and run in-process.

WORKDIR /app

# `[server] host` defaults to 0.0.0.0 and `port` to 8080.
EXPOSE 8080

ENTRYPOINT ["apiplant"]
CMD ["run", "/app"]
