# Packaging

Four package definitions live here. The first three are templates: the
`homebrew`, `aur` and `debian` jobs in
[`.github/workflows/release.yml`](../.github/workflows/release.yml) substitute
the `@VERSION@`, `@SHA_*@` and `@ARCH@` placeholders with the tag's version and
the checksums of the archives that release just built, then publish the result.
None of them compiles anything — they all install the binaries the `binaries`
job already produced, which is what keeps the packages and the release
byte-identical.

| File | Publishes to | Installs |
| --- | --- | --- |
| `homebrew/apiplant.rb` | `apiplant/homebrew-tap`, as `Formula/apiplant.rb` | macOS arm64, Linux x86_64 and aarch64 |
| `aur/PKGBUILD` | `aur.archlinux.org/apiplant-bin.git` | Linux x86_64 and aarch64 |
| `debian/control` | the release itself, as `.deb` assets | Debian/Ubuntu amd64 and arm64 |
| `apt/apt-ftparchive.conf` | `apiplant/apt`, served by GitHub Pages | the same `.deb`s, over `apt install` |

The Homebrew and AUR jobs run *after* `release`, because the formula and the
PKGBUILD reference the release assets by URL and would checksum a 404
otherwise. Each is guarded on its credential being present, so a fork — or this
repository before the setup below is done — skips it rather than failing the
release. The Debian job is the other way round: the `.deb`s carry the binary
rather than pointing at it, so they are built first and `release` attaches them
alongside the archives. It needs no credential and always runs. The `apt` job
then takes those same `.deb`s and folds them into the repository.

## One-time setup

**Homebrew.** Create a public repository `apiplant/homebrew-tap` (the
`homebrew-` prefix is what makes `brew install apiplant/tap/apiplant` resolve).
It can be empty; the job creates `Formula/`. Then set the repository secret
`HOMEBREW_TAP_TOKEN` to a token with write access to it — a fine-grained PAT
scoped to that one repository with `Contents: read and write`, or a GitHub App
installation token. The default `GITHUB_TOKEN` cannot be used: it is scoped to
this repository only.

**AUR.** Register an account on [aur.archlinux.org](https://aur.archlinux.org),
add an SSH public key to it, and submit the package once by hand:

```bash
git clone ssh://aur@aur.archlinux.org/apiplant-bin.git
cd apiplant-bin
# render PKGBUILD for the current release, then:
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO && git commit -m "apiplant 0.6.1" && git push
```

The first push is what creates the package; after that the workflow keeps it up
to date. Set the repository secret `AUR_SSH_KEY` to the matching **private** key
(the whole PEM, including the header and footer lines).

**Debian.** Nothing to set up — there is no repository to push to and no
credential to hold. The packages are release assets, installed with `dpkg -i`.

**apt.** Three things, once:

1. Create a public repository `apiplant/apt` and enable GitHub Pages on it
   (Settings → Pages → deploy from branch, root of `main`). It can be empty;
   the job creates `pool/` and `dists/`. The published URL becomes
   `https://apiplant.github.io/apt` — put a `CNAME` file in the repository if
   you later want `apt.apiplant.com`, and change the URL in the top-level
   README to match.
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
curl -sSfL https://apiplant.github.io/apt/apiplant-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/apiplant.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/apiplant.gpg] https://apiplant.github.io/apt stable main" \
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
URIs: https://apiplant.github.io/apt
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

Edit the template here, never the copy in the tap or the AUR repository — the
next release overwrites those. To check a PKGBUILD change without cutting a
release, render it against a version that is already published and build it:

```bash
sed -e 's/@VERSION@/0.6.1/g' \
    -e "s/@SHA_LINUX_X86_64@/$(curl -sSfL https://github.com/apiplant/apiplant/releases/download/v0.6.1/apiplant-v0.6.1-x86_64-unknown-linux-gnu.tar.gz | sha256sum | cut -d' ' -f1)/" \
    -e 's/@SHA_LINUX_ARM64@/SKIP/' \
    aur/PKGBUILD > /tmp/PKGBUILD
(cd /tmp && makepkg -f)
```

A `.deb` needs neither a tag nor a release to check — the job is a shell
function, so the same steps run anywhere `dpkg-deb` does:

```bash
docker run --rm -v "$PWD:/w" -w /w debian:bookworm bash -c '
  dpkg-deb --root-owner-group --build stage/amd64 test.deb && dpkg -i test.deb'
```

A version bump needs nothing here: the templates read the version from the tag,
and both revision numbers only reset to `1` — bump `pkgrel` in `aur/PKGBUILD`,
or the `-1` in `debian/control`, by hand if a package has to be rebuilt for an
unchanged upstream version.
