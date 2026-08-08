#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
DIST=${DIST_DIR:-"$ROOT/dist/local-release"}
TMP=$(mktemp -d "${TMPDIR:-/tmp}/apiplant-local-release.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

REPO_SLUG=${REPO_SLUG:-apiplant/apiplant}
HOMEBREW_TAP_REPO=${HOMEBREW_TAP_REPO:-apiplant/homebrew-tap}
APT_REPO=${APT_REPO:-apiplant/apt}
PACMAN_REPO=${PACMAN_REPO:-apiplant/pacman}

log() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || die "missing required tool: $1"
}

# True only when the URL really serves the file. `curl -f` alone is not that
# check: both repositories are GitHub Pages sites with a custom domain, so
# github.io answers every path — present or not — with a 301 to that domain,
# and a 301 is not a failure. Without `-L` the probe therefore reported every
# package as already published and skipped the upload. Redirects are followed
# and only the final status decides.
published() {
  local code
  code=$(curl -sSIL -o /dev/null -w '%{http_code}' --max-time 30 "$1" 2>/dev/null) || return 1
  case "$code" in
    2??) return 0 ;;
    *) return 1 ;;
  esac
}

# Imports a signing key into a keyring of its own and prints its id.
#
# The keyring is a throwaway one because both alternatives are wrong on a
# developer machine, and neither shows up in CI where the keyring starts empty:
# importing into the user's own ~/.gnupg leaves the repository key in their
# personal keyring, and "the first secret key in the keyring" then resolves to
# whichever key sorts first — usually the developer's own — so the repository
# gets signed with the wrong key. Here the keyring holds exactly one key, so
# the lookup cannot pick the wrong one.
#
# The key material is accepted in the three shapes it actually arrives in: an
# armoured block, a path to a file holding one, or base64 of either — a `.env`
# or a secret store that cannot hold newlines usually produces the last.
import_signing_key() {
  local material="$1" home="$2" keyfile="$2/import.asc"
  rm -rf "$home"
  mkdir -p "$home"
  chmod 700 "$home"

  if [ -f "$material" ]; then
    cat "$material" > "$keyfile"
  else
    printf '%s\n' "$material" > "$keyfile"
    # Not armour, so try it as base64. `-d` failing leaves the original.
    if ! grep -q 'BEGIN PGP' "$keyfile"; then
      if base64 -d < "$keyfile" > "$keyfile.decoded" 2>/dev/null &&
         grep -q 'BEGIN PGP' "$keyfile.decoded"; then
        mv "$keyfile.decoded" "$keyfile"
      fi
      rm -f "$keyfile.decoded"
    fi
  fi

  # gpg's own diagnosis, kept rather than discarded: an import that fails
  # silently is what makes this look like a broken key when it is a mangled
  # one — armour flattened onto a single line is the usual cause.
  if ! GNUPGHOME="$home" gpg --batch --import "$keyfile" 2>"$home/import.log"; then
    sed 's/^/  /' "$home/import.log" >&2
  fi
  GNUPGHOME="$home" gpg --list-secret-keys --with-colons \
    | awk -F: '/^sec:/ { print $5; exit }'
}

# The publish subshells exit 97 when the repository already had everything
# staged — a no-op re-run, not a failure.
report_publish() {
  case "$1" in
    0) log "updated $2" ;;
    97) log "$2 already up to date" ;;
    *) die "failed to update $2" ;;
  esac
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

version() {
  sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1
}

detect_platform() {
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64)
      OS=darwin
      ARCH=arm64
      TARGET=aarch64-apple-darwin
      BREW_OS=macos
      BREW_CPU=arm
      ;;
    Darwin-x86_64)
      OS=darwin
      ARCH=x86_64
      TARGET=x86_64-apple-darwin
      BREW_OS=macos
      BREW_CPU=intel
      ;;
    Linux-aarch64|Linux-arm64)
      OS=linux
      ARCH=arm64
      TARGET=aarch64-unknown-linux-gnu
      DEB_ARCH=arm64
      PACMAN_ARCH=aarch64
      BREW_OS=linux
      BREW_CPU=arm
      ;;
    Linux-x86_64)
      OS=linux
      ARCH=x86_64
      TARGET=x86_64-unknown-linux-gnu
      DEB_ARCH=amd64
      PACMAN_ARCH=x86_64
      BREW_OS=linux
      BREW_CPU=intel
      ;;
    *)
      die "unsupported platform: $(uname -s)-$(uname -m)"
      ;;
  esac
}

gh_release_exists() {
  command -v gh >/dev/null 2>&1 && gh release view "$TAG" --repo "$REPO_SLUG" >/dev/null 2>&1
}

release_has_asset() {
  local asset="$1"
  gh release view "$TAG" --repo "$REPO_SLUG" --json assets --jq '.assets[].name' \
    | grep -Fxq "$asset"
}

ensure_release_asset() {
  local file="$1" asset
  asset=$(basename "$file")
  gh_release_exists || return 0
  if release_has_asset "$asset"; then
    log "release already has $asset"
    return 0
  fi
  log "uploading $asset to $TAG"
  gh release upload "$TAG" "$file" --repo "$REPO_SLUG"
}

download_release_shas() {
  local dir="$1"
  rm -rf "$dir"
  mkdir -p "$dir"
  gh release download "$TAG" --repo "$REPO_SLUG" --pattern '*.sha256' --dir "$dir" >/dev/null
}

sha_for_asset() {
  local asset="$1" dir="$2"
  local file
  file=$(find "$dir" -name '*.sha256' -type f -print | while read -r path; do
    if grep -Fq "  $asset" "$path"; then
      printf '%s\n' "$path"
      break
    fi
  done)
  [ -n "$file" ] || return 1
  awk -v name="$asset" '$2 == name { print $1; exit }' "$file"
}

append_formula_cpu_block() {
  local cpu="$1" url="$2" sha="$3" out="$4"
  if [ "$cpu" = arm ]; then
    printf '    on_arm do\n' >> "$out"
  else
    printf '    on_intel do\n' >> "$out"
  fi
  printf '      url "%s"\n' "$url" >> "$out"
  printf '      sha256 "%s"\n' "$sha" >> "$out"
  printf '    end\n' >> "$out"
  printf '  end\n\n' >> "$out"
}

render_formula() {
  local out="$1" shas_dir="$2"
  local asset sha
  local mac_intel_sha= mac_arm_sha= linux_intel_sha= linux_arm_sha=

  cat > "$out" <<EOF
# Generated by packaging/local-release.sh in apiplant/apiplant. The release
# workflow may overwrite this file on the next tagged release.
class Apiplant < Formula
  desc "Point it at an app directory and it serves an API"
  homepage "https://github.com/apiplant/apiplant"
  version "$VERSION"
  license any_of: ["MIT", "Apache-2.0"]

EOF

  asset="apiplant-${TAG}-x86_64-apple-darwin.tar.gz"
  if sha=$(sha_for_asset "$asset" "$shas_dir" 2>/dev/null); then
    mac_intel_sha=$sha
  fi

  asset="apiplant-${TAG}-aarch64-apple-darwin.tar.gz"
  if sha=$(sha_for_asset "$asset" "$shas_dir" 2>/dev/null); then
    mac_arm_sha=$sha
  fi

  asset="apiplant-${TAG}-x86_64-unknown-linux-gnu.tar.gz"
  if sha=$(sha_for_asset "$asset" "$shas_dir" 2>/dev/null); then
    linux_intel_sha=$sha
  fi

  asset="apiplant-${TAG}-aarch64-unknown-linux-gnu.tar.gz"
  if sha=$(sha_for_asset "$asset" "$shas_dir" 2>/dev/null); then
    linux_arm_sha=$sha
  fi

  if [ -n "$mac_intel_sha" ] || [ -n "$mac_arm_sha" ]; then
    printf '  on_macos do\n' >> "$out"
    if [ -n "$mac_intel_sha" ]; then
      append_formula_cpu_block intel \
        "https://github.com/${REPO_SLUG}/releases/download/${TAG}/apiplant-${TAG}-x86_64-apple-darwin.tar.gz" \
        "$mac_intel_sha" "$out"
    fi
    if [ -n "$mac_arm_sha" ]; then
      append_formula_cpu_block arm \
        "https://github.com/${REPO_SLUG}/releases/download/${TAG}/apiplant-${TAG}-aarch64-apple-darwin.tar.gz" \
        "$mac_arm_sha" "$out"
    fi
    printf '  end\n\n' >> "$out"
  fi

  if [ -n "$linux_intel_sha" ] || [ -n "$linux_arm_sha" ]; then
    printf '  on_linux do\n' >> "$out"
    if [ -n "$linux_intel_sha" ]; then
      append_formula_cpu_block intel \
        "https://github.com/${REPO_SLUG}/releases/download/${TAG}/apiplant-${TAG}-x86_64-unknown-linux-gnu.tar.gz" \
        "$linux_intel_sha" "$out"
    fi
    if [ -n "$linux_arm_sha" ]; then
      append_formula_cpu_block arm \
        "https://github.com/${REPO_SLUG}/releases/download/${TAG}/apiplant-${TAG}-aarch64-unknown-linux-gnu.tar.gz" \
        "$linux_arm_sha" "$out"
    fi
    printf '  end\n\n' >> "$out"
  fi

  cat >> "$out" <<'EOF'
  def install
    bin.install "apiplant"
    doc.install "README.md"
  end

  def caveats
    <<~EOS
      `apiplant build` shells out to a toolchain per language — cargo for .rs,
      cc for .c, zig for .zig, go for .go — so install whichever your functions
      use. TypeScript needs nothing; it is transpiled in-process.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/apiplant version")
  end
end
EOF
}

build_archive() {
  need cargo
  mkdir -p "$DIST"
  log "building apiplant for $TARGET"
  cargo build --release --locked --bin apiplant --target "$TARGET"

  local dir="$TMP/apiplant-${TAG}-${TARGET}"
  mkdir -p "$dir"
  cp "$ROOT/target/$TARGET/release/apiplant" "$dir/"
  cp "$ROOT/README.md" "$dir/"

  ARCHIVE="$DIST/apiplant-${TAG}-${TARGET}.tar.gz"
  CHECKSUM="$ARCHIVE.sha256"
  tar czf "$ARCHIVE" -C "$TMP" "$(basename "$dir")"
  printf '%s  %s\n' "$(sha256_file "$ARCHIVE")" "$(basename "$ARCHIVE")" > "$CHECKSUM"
  log "wrote $(basename "$ARCHIVE")"
}

build_deb() {
  [ "$OS" = linux ] || return 0
  command -v dpkg-deb >/dev/null 2>&1 || { warn "dpkg-deb not found; skipping .deb"; return 0; }

  local root="$TMP/stage/$DEB_ARCH"
  local src="$TMP/deb-src"
  rm -rf "$root" "$src"
  mkdir -p "$src"
  tar xzf "$ARCHIVE" -C "$src"
  src="$src/apiplant-${TAG}-${TARGET}"

  install -Dm755 "$src/apiplant" "$root/usr/bin/apiplant"
  install -Dm644 "$src/README.md" "$root/usr/share/doc/apiplant/README.md"

  local size
  size=$(du -ks "$root/usr" | cut -f1)

  install -d "$root/DEBIAN"
  sed -e "s/@VERSION@/$VERSION/" \
      -e "s/@ARCH@/$DEB_ARCH/" \
      -e "s/@INSTALLED_SIZE@/$size/" \
      "$ROOT/packaging/debian/control" > "$root/DEBIAN/control"

  DEB="$DIST/apiplant_${VERSION}-1_${DEB_ARCH}.deb"
  dpkg-deb --root-owner-group --build "$root" "$DEB" >/dev/null
  printf '%s  %s\n' "$(sha256_file "$DEB")" "$(basename "$DEB")" > "$DEB.sha256"
  log "wrote $(basename "$DEB")"
}

build_pacman_pkg() {
  [ "$OS" = linux ] || return 0
  # x86_64 only, matching the pacman repository the release workflow publishes.
  [ "$PACMAN_ARCH" = x86_64 ] || { warn "pacman packages are x86_64 only; skipping"; return 0; }
  command -v makepkg >/dev/null 2>&1 || { warn "makepkg not found; skipping pacman package"; return 0; }
  [ "$(id -u)" -ne 0 ] || die "run packaging/local-release.sh as a non-root user when building pacman packages"

  local builddir="$TMP/pacman"
  local pkgsha
  pkgsha=$(sha256_file "$ARCHIVE")
  rm -rf "$builddir"
  mkdir -p "$builddir"
  cp "$ARCHIVE" "$builddir/"

  cat > "$builddir/PKGBUILD" <<EOF
pkgname=apiplant
pkgver=$VERSION
pkgrel=1
pkgdesc="Point it at an app directory and it serves an API"
arch=('$PACMAN_ARCH')
url="https://github.com/apiplant/apiplant"
license=('MIT' 'Apache-2.0')
source=("$(basename "$ARCHIVE")")
sha256sums=('$pkgsha')
options=('!strip' '!debug')

package() {
  local src="\$srcdir/apiplant-$TAG-$TARGET"
  install -Dm755 "\$src/apiplant" "\$pkgdir/usr/bin/apiplant"
  install -Dm644 "\$src/README.md" "\$pkgdir/usr/share/doc/\$pkgname/README.md"
}
EOF

  (cd "$builddir" && makepkg --cleanbuild --clean --force --nodeps >/dev/null)
  PACMAN_PKG="$builddir/apiplant-$VERSION-1-$PACMAN_ARCH.pkg.tar.zst"
  # An unmatched glob used to be passed to `cp` verbatim; name the file outright
  # and say which one is missing instead.
  [ -f "$PACMAN_PKG" ] || die "makepkg did not produce $(basename "$PACMAN_PKG")"
  cp "$PACMAN_PKG" "$DIST/"
  PACMAN_PKG="$DIST/$(basename "$PACMAN_PKG")"
  printf '%s  %s\n' "$(sha256_file "$PACMAN_PKG")" "$(basename "$PACMAN_PKG")" > "$PACMAN_PKG.sha256"
  log "wrote $(basename "$PACMAN_PKG")"
}

publish_homebrew() {
  [ "$OS" = darwin ] || return 0
  [ -n "${HOMEBREW_TAP_TOKEN:-}" ] || { warn "HOMEBREW_TAP_TOKEN not set; skipping Homebrew"; return 0; }
  gh_release_exists || { warn "release $TAG does not exist; skipping Homebrew"; return 0; }

  ensure_release_asset "$ARCHIVE"
  ensure_release_asset "$CHECKSUM"

  local shas_dir="$TMP/homebrew-shas"
  local tap="$TMP/homebrew-tap"
  local formula="$TMP/apiplant.rb"
  download_release_shas "$shas_dir"
  render_formula "$formula" "$shas_dir"

  git clone --depth 1 "https://x-access-token:${HOMEBREW_TAP_TOKEN}@github.com/${HOMEBREW_TAP_REPO}.git" "$tap"
  mkdir -p "$tap/Formula"
  if [ -f "$tap/Formula/apiplant.rb" ] && cmp -s "$formula" "$tap/Formula/apiplant.rb"; then
    log "homebrew formula already up to date"
    return 0
  fi

  cp "$formula" "$tap/Formula/apiplant.rb"
  (
    cd "$tap"
    git config user.name "github-actions[bot]"
    git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
    git add Formula/apiplant.rb
    git commit -m "apiplant $VERSION"
    git push
  )
  log "updated Homebrew tap"
}

publish_apt() {
  [ "$OS" = linux ] || return 0
  [ -n "${DEB:-}" ] || { warn "no .deb built; skipping apt"; return 0; }
  # Probed through github.io rather than apt.apiplant.com: the custom domain
  # has no valid certificate for https, so every probe there fails the TLS
  # handshake and reports a package that is published as missing.
  if published "https://apiplant.github.io/apt/pool/main/a/apiplant/$(basename "$DEB")"; then
    log "apt already has $(basename "$DEB")"
    return 0
  fi

  [ -n "${APT_REPO_TOKEN:-}" ] || { warn "APT_REPO_TOKEN not set; skipping apt"; return 0; }
  [ -n "${APT_GPG_PRIVATE_KEY:-}" ] || die "APT_REPO_TOKEN is set but APT_GPG_PRIVATE_KEY is not"

  local repo="$TMP/apt-repo"
  local keyid pass_args rc

  git clone --depth 1 "https://x-access-token:${APT_REPO_TOKEN}@github.com/${APT_REPO}.git" "$repo"
  mkdir -p "$repo/pool/main/a/apiplant"
  cp "$DEB" "$repo/pool/main/a/apiplant/"

  export GNUPGHOME="$TMP/gnupg-apt"
  keyid=$(import_signing_key "$APT_GPG_PRIVATE_KEY" "$GNUPGHOME")
  [ -n "$keyid" ] || die "failed to load apt signing key from APT_GPG_PRIVATE_KEY"

  (
    cd "$repo"
    for arch in amd64 arm64; do
      dir="dists/stable/main/binary-$arch"
      mkdir -p "$dir"
      dpkg-scanpackages --arch "$arch" pool /dev/null > "$dir/Packages"
      gzip -9kf "$dir/Packages"
    done
    apt-ftparchive -c "$ROOT/packaging/apt/apt-ftparchive.conf" release dists/stable > /tmp/Release
    mv /tmp/Release dists/stable/Release

    if [ -n "${APT_GPG_PASSPHRASE:-}" ]; then
      gpg --batch --yes --pinentry-mode loopback --passphrase "$APT_GPG_PASSPHRASE" \
        --local-user "$keyid" --clearsign -o dists/stable/InRelease dists/stable/Release
      gpg --batch --yes --pinentry-mode loopback --passphrase "$APT_GPG_PASSPHRASE" \
        --local-user "$keyid" --detach-sign --armor -o dists/stable/Release.gpg dists/stable/Release
    else
      gpg --batch --yes --local-user "$keyid" \
        --clearsign -o dists/stable/InRelease dists/stable/Release
      gpg --batch --yes --local-user "$keyid" \
        --detach-sign --armor -o dists/stable/Release.gpg dists/stable/Release
    fi
    gpg --export "$keyid" > apiplant-archive-keyring.gpg
    touch .nojekyll

    git config user.name "github-actions[bot]"
    git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
    git add -A
    # Re-running for a version the repository already has is a no-op rather
    # than an error: `git commit` with nothing staged exits non-zero, which
    # under `set -e` would abort the whole script. 97 says so to the caller.
    git diff --cached --quiet && exit 97
    git commit -m "apiplant $VERSION"
    git push
  ) && rc=0 || rc=$?
  report_publish "$rc" "apt repository"
}

publish_pacman() {
  [ "$OS" = linux ] || return 0
  [ -n "${PACMAN_PKG:-}" ] || { warn "no pacman package built; skipping pacman repo"; return 0; }
  command -v repo-add >/dev/null 2>&1 || { warn "repo-add not found; skipping pacman repo"; return 0; }
  if published "https://apiplant.github.io/pacman/$PACMAN_ARCH/$(basename "$PACMAN_PKG")"; then
    log "pacman repo already has $(basename "$PACMAN_PKG")"
    return 0
  fi

  [ -n "${PACMAN_REPO_TOKEN:-}" ] || { warn "PACMAN_REPO_TOKEN not set; skipping pacman repo"; return 0; }
  [ -n "${PACMAN_GPG_PRIVATE_KEY:-}" ] || die "PACMAN_REPO_TOKEN is set but PACMAN_GPG_PRIVATE_KEY is not"

  local repo="$TMP/pacman-repo"
  local keyid rc

  git clone --depth 1 "https://x-access-token:${PACMAN_REPO_TOKEN}@github.com/${PACMAN_REPO}.git" "$repo"
  mkdir -p "$repo/$PACMAN_ARCH"
  cp "$PACMAN_PKG" "$repo/$PACMAN_ARCH/"

  export GNUPGHOME="$TMP/gnupg-pacman"
  keyid=$(import_signing_key "$PACMAN_GPG_PRIVATE_KEY" "$GNUPGHOME")
  [ -n "$keyid" ] || die "failed to load pacman signing key from PACMAN_GPG_PRIVATE_KEY"

  (
    cd "$repo"
    sign() {
      rm -f "$1.sig"
      if [ -n "${PACMAN_GPG_PASSPHRASE:-}" ]; then
        gpg --batch --yes --pinentry-mode loopback --passphrase "$PACMAN_GPG_PASSPHRASE" \
          --local-user "$keyid" --detach-sign "$1"
      else
        gpg --batch --yes --local-user "$keyid" --detach-sign "$1"
      fi
    }

    sign "$PACMAN_ARCH/$(basename "$PACMAN_PKG")"
    repo-add --include-sigs "$PACMAN_ARCH/apiplant.db.tar.zst" "$PACMAN_ARCH/$(basename "$PACMAN_PKG")"

    # repo-add leaves apiplant.db and apiplant.files as symlinks to the .tar.zst
    # files. git stores a symlink as its target path, and GitHub Pages serves
    # that back as a 19-byte text file — so `pacman -Sy` would download the
    # string "apiplant.db.tar.zst" instead of a database. Real copies are what
    # gets committed.
    for name in apiplant.db apiplant.files; do
      cp --remove-destination "$PACMAN_ARCH/$name.tar.zst" "$PACMAN_ARCH/$name"
    done

    sign "$PACMAN_ARCH/apiplant.db"
    sign "$PACMAN_ARCH/apiplant.files"
    gpg --armor --export "$keyid" > apiplant.gpg
    touch .nojekyll

    git config user.name "github-actions[bot]"
    git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
    git add -A
    # Re-running for a version the repository already has is a no-op rather
    # than an error: `git commit` with nothing staged exits non-zero, which
    # under `set -e` would abort the whole script. 97 says so to the caller.
    git diff --cached --quiet && exit 97
    git commit -m "apiplant $VERSION"
    git push
  ) && rc=0 || rc=$?
  report_publish "$rc" "pacman repository"
}

main() {
  detect_platform
  VERSION=$(version)
  [ -n "$VERSION" ] || die "failed to read version from Cargo.toml"
  TAG="v$VERSION"

  build_archive
  if gh_release_exists; then
    ensure_release_asset "$ARCHIVE"
    ensure_release_asset "$CHECKSUM"
  else
    warn "release $TAG does not exist on GitHub; keeping artifacts local only"
  fi

  if [ "$OS" = linux ]; then
    build_deb
    build_pacman_pkg

    # Written out rather than chained with `&&`: as the last command of an `if`
    # body, a chain that short-circuits — no .deb, or no release to upload to —
    # makes the whole block exit non-zero, and `set -e` then killed the script
    # here, before any repository was published.
    if gh_release_exists; then
      if [ -n "${DEB:-}" ]; then
        ensure_release_asset "$DEB"
        ensure_release_asset "$DEB.sha256"
      fi
      # The release carries the pacman package too, matching what the release
      # workflow attaches.
      if [ -n "${PACMAN_PKG:-}" ]; then
        ensure_release_asset "$PACMAN_PKG"
        ensure_release_asset "$PACMAN_PKG.sha256"
      fi
    fi
  fi

  publish_homebrew
  publish_apt
  publish_pacman

  log "artifacts are in $DIST"
}

main "$@"
