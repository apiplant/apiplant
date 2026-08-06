# The apiplant server, built from source and shipped on glibc and nothing else.
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


# The runtime is glibc, libgcc and this binary. Distroless rather than
# `debian:bookworm-slim` because the binary needs nothing else and the
# difference is most of the image: 252MB to ~125MB, the whole of it base.
#
# Not Alpine, and not `busybox:glibc`: V8 (via deno_core) ships prebuilt static
# libraries for glibc targets only, and busybox's glibc is an unversioned image
# to keep in step with whatever the builder above compiled against — the same
# trade the base below makes, but maintained by somebody else.
#
# `cc` is the variant that carries libgcc (Rust's unwinder needs it) on top of
# glibc, and it brings the CA store and the NSS modules with it, so outbound
# HTTPS and resolving a database host by name both work. Swap the tag for
# `:debug` to get a busybox shell in there when something needs poking at.
FROM gcr.io/distroless/cc-debian12

COPY --from=builder /src/target/release/apiplant /usr/local/bin/apiplant

# What this image cannot do, both because there is no shell and no toolchain:
#
#   * `apiplant build` on `.rs`, `.c`, `.zig` or `.go` — those shell out to
#     cargo, cc, zig and go. Build the app in a stage that has them and copy the
#     libraries in (see `examples/21-docker`), or write functions in TypeScript,
#     which is transpiled in-process and needs nothing.
#   * `apiplant init --from <repo>` — that runs `git clone`, and git is not
#     here. Scaffold on the host; the image is for running an app, not
#     for starting one.

WORKDIR /app

# `[server] host` defaults to 0.0.0.0 and `port` to 8080.
EXPOSE 8080

ENTRYPOINT ["apiplant"]
CMD ["run", "/app"]
