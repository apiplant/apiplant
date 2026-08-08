# Packaging

Four package definitions live here. The `packages`, `homebrew`, `apt` and
`pacman` jobs in
[`.github/workflows/release.yml`](../.github/workflows/release.yml) substitute
the `@VERSION@`, `@SHA_*@` and `@ARCH@` placeholders with the tag's version and
the checksums of the archives that release just built, then publish the result.
None of the definitions compiles the project from source — they all install the
binaries the `binaries` job already produced, which is what keeps the packages
and the release byte-identical.

| File | Publishes to | Installs |
| --- | --- | --- |
| `homebrew/apiplant.rb` | `apiplant/homebrew-tap`, as `Formula/apiplant.rb` | macOS arm64, Linux x86_64 and aarch64 |
| `pacman/PKGBUILD` | the release itself, as a `.pkg.tar.zst` asset, and `apiplant/pacman` | Arch Linux x86_64 |
| `debian/control` | the release itself, as `.deb` assets | Debian/Ubuntu amd64 and arm64 |
| `apt/apt-ftparchive.conf` | `apiplant/apt`, served at `apt.apiplant.com` | the same `.deb`s, over `apt install` |

The order is: build every package, publish the release, then publish to every
repository. The `packages` job builds both `.deb`s and the pacman package —
they carry the binary rather than pointing at it, so none of them needs the
release to exist yet — and `release` attaches all of them alongside the
archives. It needs no credential and always runs. Only then do `homebrew`,
`apt` and `pacman` run: the formula references the release assets by URL and
would checksum a 404 otherwise, and neither repository should serve a version
the release itself does not have. Each publish job is guarded on its credential
being present, so a fork — or this repository before the setup below is done —
skips it rather than failing the release.

The pacman package is x86_64 only. The aarch64 build is deliberately off: it
would need an arm runner or emulation, and the Arch repository has no aarch64
audience. The `.deb`s and the plain archives still cover Linux arm64.

For one-off builds outside CI, run [`local-release.sh`](local-release.sh). It
compiles apiplant for the host platform, writes release-shaped artifacts into
`dist/local-release/`, uploads any missing assets to the tagged GitHub release,
and, when the matching credentials are present, syncs the package repository
that serves that platform:

```bash
./packaging/local-release.sh
```

On macOS it builds the release archive for the host CPU and, if the tag exists
and `HOMEBREW_TAP_TOKEN` is set, uploads the archive and rewrites the tap
formula from the release's available assets — which is how an extra platform
such as macOS Intel can be added without waiting for CI support.

On Linux x86_64 and aarch64 it also builds the `.deb` and, on x86_64, the
pacman package for the host architecture, uploads the `.deb` to the GitHub release, and, if the
version is not already present, updates `apt.apiplant.com` and
`apiplant.github.io/pacman` using `APT_REPO_TOKEN` / `APT_GPG_PRIVATE_KEY` and
`PACMAN_REPO_TOKEN` / `PACMAN_GPG_PRIVATE_KEY`.

## One-time setup

**Homebrew.** Create a public repository `apiplant/homebrew-tap` (the
`homebrew-` prefix is what makes `brew install apiplant/tap/apiplant` resolve).
It can be empty; the job creates `Formula/`. Then set the repository secret
`HOMEBREW_TAP_TOKEN` to a token with write access to it — a fine-grained PAT
scoped to that one repository with `Contents: read and write`, or a GitHub App
installation token. The default `GITHUB_TOKEN` cannot be used: it is scoped to
this repository only.

**AUR.** Publishing is disabled. The AUR is rejecting package updates while
under attack, so the workflow pushes neither `PKGBUILD` nor `.SRCINFO` there.
`pacman/PKGBUILD` is written for the pacman repository rather than the AUR: the
package is named `apiplant` (not `apiplant-bin`) and carries no
`provides`/`conflicts` pair. Re-enabling AUR publishing means adding a second,
AUR-shaped manifest — not pushing this one.

**pacman repo.** Create a public repository `apiplant/pacman` and serve it as
static files — GitHub Pages is enough. The workflow pushes to it when the
repository secret `PACMAN_REPO_TOKEN` is present. A conventional layout is one
directory per architecture:

```text
pacman/
└── x86_64/
│   ├── apiplant.db.tar.zst
│   ├── apiplant.db.tar.zst.sig
│   ├── apiplant.files.tar.zst
│   ├── apiplant.files.tar.zst.sig
    ├── apiplant-0.8.0-1-x86_64.pkg.tar.zst
    └── apiplant-0.8.0-1-x86_64.pkg.tar.zst.sig
```

The package itself comes from `pacman/PKGBUILD`; the repository is the
published result of building it and then running `repo-add` over the output
directory. The workflow does both automatically:

```bash
# in the packages job, in an archlinux:base-devel container
makepkg --cleanbuild --force --nodeps

# then, in the publish job
gpg --batch --yes --detach-sign --local-user "$keyid" repo/$arch/*.pkg.tar.zst
repo-add --include-sigs repo/$arch/apiplant.db.tar.zst repo/$arch/*.pkg.tar.zst
gpg --batch --yes --detach-sign --local-user "$keyid" repo/$arch/apiplant.db
gpg --batch --yes --detach-sign --local-user "$keyid" repo/$arch/apiplant.files
```

Set two secrets for it: `PACMAN_REPO_TOKEN` and `PACMAN_GPG_PRIVATE_KEY`. If
the key has a passphrase, set `PACMAN_GPG_PASSPHRASE` too. If the token is set
without the key, the job fails loudly rather than publishing an unsigned
repository.

The job exports the signing key's **public** half as `apiplant.gpg`. Users add
it to pacman's keyring once, then locally sign it:

```bash
curl -sSfL https://apiplant.github.io/pacman/apiplant.gpg -o /tmp/apiplant.gpg
keyid=$(gpg --show-keys --with-colons /tmp/apiplant.gpg | awk -F: '/^pub:/ { print $5; exit }')
sudo pacman-key --add /tmp/apiplant.gpg
sudo pacman-key --finger "$keyid"
sudo pacman-key --lsign-key "$keyid"
```

Then they add the repository to `pacman.conf`:

```bash
printf '\n[apiplant]\nSigLevel = Required DatabaseOptional\nServer = https://apiplant.github.io/pacman/$arch\n' \
  | sudo tee -a /etc/pacman.conf > /dev/null
```

With that in place, installation is the ordinary pacman flow:

```bash
sudo pacman -Sy apiplant
```

**Debian.** Nothing to set up — there is no repository to push to and no
credential to hold. The packages are release assets, installed with `dpkg -i`.

**apt.** Three things, once:

1. Create a public repository `apiplant/apt` and enable GitHub Pages on it
   (Settings → Pages → deploy from branch, root of `main`). It can be empty;
   the job creates `pool/` and `dists/`. Keep a `CNAME` file with
   `apt.apiplant.com` in that repository, so GitHub Pages serves the archive at
   `https://apt.apiplant.com`.
2. Generate a signing key and keep it. It signs every release from here on, and
   replacing it means every user re-installing the keyring, so back the private
   key up somewhere that is not a CI secret:

   ```bash
   gpg --quick-gen-key "apiplant <federico@apiplant.com>" default default never
   gpg --armor --export-secret-keys <key-id>   # → APT_GPG_PRIVATE_KEY
   ```

   A key with no passphrase is the simpler CI arrangement; if you give it one,
   set `APT_GPG_PASSPHRASE` too and the job will use it.
3. Set the repository secrets: `APT_REPO_TOKEN` (write access to
   `apiplant/apt` — same shape as the Homebrew token, and one fine-grained PAT
   can cover both repositories), `APT_GPG_PRIVATE_KEY`, and optionally
   `APT_GPG_PASSPHRASE`.

The job is guarded on `APT_REPO_TOKEN`, so until that is set the repository is
simply not published — but it fails loudly if the token is set without a key,
because an unsigned `Release` is one apt refuses to use.

### What users run

The `signed-by` form, which is what the top-level README documents:

```bash
curl -sSfL https://apt.apiplant.com/apiplant-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/apiplant.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/apiplant.gpg] https://apt.apiplant.com stable main" \
  | sudo tee /etc/apt/sources.list.d/apiplant.list > /dev/null
sudo apt update && sudo apt install apiplant
```

The keyring is exported dearmored, so it can be written straight to
`/usr/share/keyrings` with no `gpg --dearmor` step. Never suggest
`apt-key add` — it is deprecated and trusts the key for every repository on the
machine, not just this one.

Debian 12 and Ubuntu 24.04 also accept the deb822 form, which is the direction
apt is heading and keeps the key with the source that uses it:

```
# /etc/apt/sources.list.d/apiplant.sources
Types: deb
URIs: https://apt.apiplant.com
Suites: stable
Components: main
Signed-By: /usr/share/keyrings/apiplant.gpg
```

### Testing the repository

The whole thing runs offline against a `file://` source — no Pages, no key of
yours, no release:

```bash
docker run --rm -it -v "$PWD:/pkg:ro" debian:bookworm bash
apt-get update && apt-get install -y dpkg-dev apt-utils gnupg

mkdir -p /repo/pool/main/a/apiplant && cp /path/to/*.deb /repo/pool/main/a/apiplant/
cd /repo
for arch in amd64 arm64; do
  d=dists/stable/main/binary-$arch; mkdir -p $d
  dpkg-scanpackages --arch $arch pool /dev/null > $d/Packages && gzip -9kf $d/Packages
done
apt-ftparchive -c /pkg/apt/apt-ftparchive.conf release dists/stable > /tmp/R
mv /tmp/R dists/stable/Release

gpg --batch --passphrase "" --quick-gen-key "test <t@example.com>" default default never
key=$(gpg --list-secret-keys --with-colons | awk -F: '/^sec:/ {print $5; exit}')
gpg --batch --yes --local-user $key --clearsign -o dists/stable/InRelease dists/stable/Release
gpg --export $key > /usr/share/keyrings/apiplant.gpg

echo "deb [signed-by=/usr/share/keyrings/apiplant.gpg] file:///repo stable main" \
  > /etc/apt/sources.list.d/apiplant.list
apt-get update && apt-get install -y apiplant && apiplant version
```

A missing or bad signature shows up as `NO_PUBKEY` or `not signed` on
`apt-get update`, and a broken `Packages` file shows up as apt not finding the
package at all.

### The pool grows

Every release adds two ~22MB packages to a pool nothing prunes, and GitHub
Pages stops publishing a site over 1GB — roughly twenty releases of headroom.
The job warns from 700MB. When it does, either drop old versions from
`apiplant/apt` (keeping the newest few, then letting the next release
regenerate the indices over what is left) or move the pool to object storage
and leave only `dists/` on Pages.

## Changing a package

Edit the template here, never the copy in the tap — the next release
overwrites it. To check an Arch packaging change without cutting a release,
render it against a version that is already published and build it:

```bash
sed -e 's/@VERSION@/0.8.0/g' \
    -e "s/@SHA_LINUX_X86_64@/$(curl -sSfL https://github.com/apiplant/apiplant/releases/download/v0.8.0/apiplant-v0.8.0-x86_64-unknown-linux-gnu.tar.gz | sha256sum | cut -d' ' -f1)/" \
    pacman/PKGBUILD > /tmp/PKGBUILD
(cd /tmp && makepkg -f)
```

A `.deb` needs neither a tag nor a release to check — the job is a shell
function, so the same steps run anywhere `dpkg-deb` does:

```bash
docker run --rm -v "$PWD:/w" -w /w debian:bookworm bash -c '
  dpkg-deb --root-owner-group --build stage/amd64 test.deb && dpkg -i test.deb'
```

A version bump needs nothing here: the templates read the version from the tag,
and both revision numbers only reset to `1` — bump `pkgrel` in `pacman/PKGBUILD`,
or the `-1` in `debian/control`, by hand if a package has to be rebuilt for an
unchanged upstream version.
