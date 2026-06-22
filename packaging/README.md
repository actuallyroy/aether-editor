# Linux packaging

How Aether is distributed on Linux, and how to maintain it.

## What ships with each release

The `Release` workflow (`.github/workflows/release.yml`) builds, on the
`ubuntu-22.04` runner, three Linux artifacts and attaches them to the GitHub
Release:

| Artifact | Built by | Install with |
| --- | --- | --- |
| `aether-linux-x86_64` | `cargo build` | bare binary / `install.sh` / self-updater |
| `aether_<ver>_amd64.deb` | `packaging/build-deb.sh` | `sudo apt install ./aether_*.deb` |
| `Aether-x86_64.AppImage` | `packaging/build-appimage.sh` | `chmod +x` and run |

All three are self-contained (fonts/assets are embedded in the binary). The
`.deb` and AppImage add a desktop launcher and the SVG icon so Aether appears
in the application menu.

## Hosted APT repository

The `Pages` workflow (`.github/workflows/pages.yml`) publishes both the landing
page and an APT repo to <https://actuallyroy.github.io/aether-editor/>. It runs
on every published release (and on `docs/` changes). It:

1. downloads every `*.deb` from all GitHub Releases into `apt/pool/main/`,
2. generates `dists/stable/...` indices with `dpkg-scanpackages` + `apt-ftparchive`,
3. signs them if the `APT_GPG_PRIVATE_KEY` secret is set.

The repo is rebuilt from scratch each run, so it always reflects the current set
of releases — nothing is committed to git.

### Users install with

```sh
curl -fsSL https://actuallyroy.github.io/aether-editor/apt/aether.gpg.key \
  | sudo gpg --dearmor -o /usr/share/keyrings/aether.gpg
echo "deb [signed-by=/usr/share/keyrings/aether.gpg] https://actuallyroy.github.io/aether-editor/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/aether.list
sudo apt update && sudo apt install aether
```

### One-time setup: the signing key

`apt` rejects unsigned repos by default. Create a key and store its private half
as the repo Actions secret `APT_GPG_PRIVATE_KEY` (Settings → Secrets and
variables → Actions):

```sh
# Generate (no passphrase — it runs unattended in CI):
gpg --batch --quick-generate-key "Aether APT <claude01hyd@gmail.com>" rsa4096 sign never
# Export the PRIVATE key (ASCII-armored) and paste the whole block as the secret:
gpg --armor --export-secret-keys "Aether APT" | xclip -selection clipboard   # or pbcopy / wl-copy
```

Until that secret exists the workflow still publishes the repo, but **unsigned** —
users would need `deb [trusted=yes] <url> stable main` instead of `signed-by=`.

> Note: a `.deb`/APT install lands the binary in `/usr/bin`, which the in-app
> self-updater cannot overwrite without root. apt-managed installs should update
> via `apt upgrade`, not the in-app updater.

## Building locally

```sh
cargo build --release -p aether-renderer
packaging/build-deb.sh      target/release/aether 0.4.9 dist
packaging/build-appimage.sh target/release/aether 0.4.9 dist
```
