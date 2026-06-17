#!/usr/bin/env sh
# Install the prebuilt `arg` binary from the latest GitHub release (no Rust needed).
#
#   curl -fsSL https://raw.githubusercontent.com/polarian/argus/master/install.sh | sh
#
# Detects OS/arch, downloads the matching release asset, verifies its sha256
# (if present), and installs to ~/.local/bin. Fetched via curl, so macOS
# Gatekeeper does not quarantine it — no code signing/notarization needed.
set -eu

REPO="polarian/argus"
DEST="${ARG_INSTALL_DIR:-$HOME/.local/bin}"

# OS/arch → release slug (arg-<version>-<os>-<arch>.tar.gz).
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin)
    case "$arch" in
      arm64 | aarch64) slug="darwin-arm64" ;;
      x86_64) slug="darwin-amd64" ;;
    esac ;;
  Linux)
    case "$arch" in
      x86_64) slug="linux-amd64" ;;
      aarch64 | arm64) slug="linux-arm64" ;;
    esac ;;
esac
if [ -z "${slug:-}" ]; then
  echo "✗ unsupported platform: $os/$arch — install via 'cargo install --git https://github.com/$REPO'" >&2
  exit 1
fi

# Resolve the latest release tag (assets are versioned).
tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)"
if [ -z "${tag:-}" ]; then
  echo "✗ could not determine the latest release — install via 'cargo install --git https://github.com/$REPO'" >&2
  exit 1
fi
version="${tag#v}"

asset="arg-$version-$slug.tar.gz"
url="https://github.com/$REPO/releases/download/$tag/$asset"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "▶ downloading $asset …"
curl -fsSL "$url" -o "$tmp/$asset"

# Verify checksum if the release published one.
if curl -fsSL "$url.sha256" -o "$tmp/$asset.sha256" 2>/dev/null; then
  echo "▶ verifying checksum …"
  ( cd "$tmp" && { shasum -a 256 -c "$asset.sha256" >/dev/null 2>&1 \
      || sha256sum -c "$asset.sha256" >/dev/null 2>&1; } ) \
    || { echo "✗ checksum verification failed" >&2; exit 1; }
fi

tar xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$DEST"
chmod +x "$tmp/arg"
cp "$tmp/arg" "$DEST/arg"
echo "✓ installed: $DEST/arg"

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "⚠ $DEST is not on PATH — add it to your shell profile:" >&2
     echo "    export PATH=\"$DEST:\$PATH\"" >&2 ;;
esac

if ! command -v gh >/dev/null 2>&1 && ! command -v bkt >/dev/null 2>&1; then
  echo "  next: install a backend CLI (gh for GitHub · bkt for Bitbucket); arg guides setup on first run."
fi
