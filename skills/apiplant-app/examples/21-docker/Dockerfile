# Deploying an apiplant app as a container.
#
# The published image already contains the server, so nothing here compiles the
# framework: `ghcr.io/apiplant/apiplant` is the runtime, this app is the
# payload. What the app does need building is its functions, and that is the one
# decision this file makes — where each language's toolchain comes from.
#
#   docker build -t my-api .
#   docker run --rm -p 8080:8080 -e DATABASE_URL=... my-api
#
# Pin a release rather than tracking `latest` for anything you deploy: the tag
# is what makes a rollback a one-word change.

# The runtime, named once so every stage below agrees on the version.
FROM ghcr.io/apiplant/apiplant:latest AS apiplant


# --- functions ---------------------------------------------------------------
# `apiplant build` shells out per language: cargo for .rs, cc for .c, zig for
# .zig, go for .go. The runtime image carries none of them — deliberately, since
# a compiler in production is weight and attack surface — so the build happens
# here, on an image that has cargo, using the `apiplant` binary copied out of
# the runtime image. Swap the base for `golang:` or `zig` and the same shape
# builds those instead.
#
# TypeScript is the exception and needs no toolchain at all: a single `.ts` file
# is transpiled in-process. An app with only TypeScript functions can delete
# this stage and run `apiplant build /app` directly on the runtime image.
FROM rust:1-bookworm AS functions

COPY --from=apiplant /usr/local/bin/apiplant /usr/local/bin/apiplant

WORKDIR /app
COPY . /app

# Produces `functions/libstatus.so` from `functions/status.rs`, and
# `functions/version.js` from `functions/version.ts`. `--release` because this
# is the artefact that ships.
#
# The scaffolding and cargo target it works in live in `.apiplant-build/`, which
# is hundreds of megabytes and has no business in the shipped layer.
RUN apiplant build --release /app && rm -rf /app/.apiplant-build


# --- runtime -----------------------------------------------------------------
FROM apiplant

# What the running app reports about itself, read by `functions/status.toml` and
# `functions/version.toml`. CI passes the real values:
# `--build-arg APP_RELEASE=$(git rev-parse --short HEAD)`.
ARG APP_RELEASE=dev
ARG APP_ENV=production
ENV APP_RELEASE=${APP_RELEASE} \
    APP_ENV=${APP_ENV}

# Only the built app crosses over, so cargo's caches and the toolchain stay in
# the stage above — the shipped image is the server, the TOML, and the .so.
COPY --from=functions /app /app

WORKDIR /app

# `[server] host` is 0.0.0.0 and `port` is 8080 — see main.toml. A host that
# assigns its own port maps it onto this one.
EXPOSE 8080

# Inherited from the base image; repeated because it is the contract this image
# publishes — run the app directory that was copied in.
ENTRYPOINT ["apiplant"]
CMD ["run", "/app"]
