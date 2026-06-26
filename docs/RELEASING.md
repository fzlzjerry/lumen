# Releasing Lumen

Lumen ships through an automated, keyword-triggered pipeline
(`.github/workflows/release.yml`). You cut a release by **bumping the version in
`Cargo.toml` and putting `[release]` in the commit message** on `main`.

```bash
# 1. bump the version
sed -i 's/^version = .*/version = "0.2.0"/' Cargo.toml
# 2. commit with the keyword
git commit -am "Release 0.2.0 [release]"
git push
```

The pipeline then, for the version in `Cargo.toml`:

1. **Tests & lints** the code (`cargo test` + `cargo clippy -D warnings`).
2. **Builds** release binaries for four targets:
   - `aarch64-apple-darwin`, `x86_64-apple-darwin` (macOS)
   - `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (Linux)
3. **Publishes a GitHub Release** `v<version>` with the tarballs, their SHA-256
   checksums, and `.deb` packages for amd64 and arm64.
4. **Updates the Homebrew tap**, **publishes the apt repository**, and
   **publishes to crates.io** — each gated on a repository secret (below).

Re-pushing the same version is a no-op: the gate skips if the tag `v<version>`
already exists. A push **without** `[release]` does nothing (only the tiny gate
job runs).

## What works out of the box

With no configuration, every `[release]` produces a **GitHub Release** containing:

- `lumen-<version>-<target>.tar.gz` + `.sha256` for all four targets
- `lumen_<version>_amd64.deb` and `lumen_<version>_arm64.deb`

Users can already install from those:

```bash
# Debian/Ubuntu, single package
curl -LO https://github.com/fzlzjerry/lumen/releases/latest/download/lumen_<version>_amd64.deb
sudo apt install ./lumen_<version>_amd64.deb

# Any platform, from a tarball
curl -L https://github.com/fzlzjerry/lumen/releases/latest/download/lumen-<version>-x86_64-unknown-linux-gnu.tar.gz | tar xz
```

## Channels that need a secret

Add these under **Settings → Secrets and variables → Actions** in the `lumen`
repo. Each channel is skipped (with a warning, not a failure) when its secret is
absent.

### Homebrew — `TAP_GITHUB_TOKEN`

A Personal Access Token with `contents: write` on the tap repository
`fzlzjerry/homebrew-lumen` (created alongside this repo). The release job clones
the tap and rewrites `Formula/lumen.rb` with the new version and checksums.

- Classic PAT: scope `repo`.
- Fine-grained PAT: repository = `homebrew-lumen`, permission **Contents:
  Read and write**.

Once set, users install with:

```bash
brew install fzlzjerry/lumen/lumen
```

### crates.io — `CARGO_REGISTRY_TOKEN`

An API token from <https://crates.io/settings/tokens> with the
`publish-update` scope. The pipeline runs `cargo publish`.

> Note: the crate name `lumen` must be available (or owned by you) on crates.io.
> If it's taken, either rename the package or leave this secret unset to skip the
> channel.

### apt repository — `APT_GPG_PRIVATE_KEY` (+ optional `APT_GPG_PASSPHRASE`)

An ASCII-armored GPG private key used to sign the apt `Release` file. The
pipeline builds a signed **flat apt repository** from the `.deb` packages and
deploys it to GitHub Pages (the `gh-pages` branch).

Generate and export a key:

```bash
gpg --quick-generate-key "Lumen Apt <you@example.com>" rsa4096 sign never
gpg --armor --export-secret-keys <KEYID>   # paste into APT_GPG_PRIVATE_KEY
```

Enable **Settings → Pages → Source: gh-pages branch** once. Users then add the
repo (served at `https://fzlzjerry.github.io/lumen`):

```bash
curl -fsSL https://fzlzjerry.github.io/lumen/lumen-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/lumen.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/lumen.gpg] https://fzlzjerry.github.io/lumen ./" \
  | sudo tee /etc/apt/sources.list.d/lumen.list
sudo apt update && sudo apt install lumen
```

## Adding another channel

The pipeline is a set of independent jobs that all depend on `gate`/`build`. To
add a channel (Scoop, AUR, Nix, Snap, …), add a job that downloads the release
assets (`gh release download "v$VERSION"`) and pushes to that channel, gated on
its own secret with the same skip-if-absent pattern.
