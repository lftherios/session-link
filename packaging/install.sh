#!/bin/sh
# session.link CLI installer — the Go build (P4). Served at
# https://session.link/install.sh once cutover happens; until then this is
# the staged artifact. Downloads the right archive from the GitHub Release,
# verifies its checksum, and drops `slink` on your PATH. No Node required.
#
#   curl -fsSL https://session.link/install.sh | sh
#
# Overrides: SLINK_VERSION (default: latest), SLINK_INSTALL_DIR
# (default: /usr/local/bin, or ~/.local/bin without write access),
# SLINK_REPO (default: lftherios/session-link).
set -eu

REPO="${SLINK_REPO:-lftherios/session-link}"
VERSION="${SLINK_VERSION:-latest}"

say() { printf '%s\n' "$*" >&2; }
die() { say "error: $*"; exit 1; }

os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)
case "$os" in
  linux|darwin) ;;
  *) die "unsupported OS: $os (Windows: download the .zip from the releases page)" ;;
esac
case "$arch" in
  x86_64|amd64) arch=amd64 ;;
  aarch64|arm64) arch=arm64 ;;
  *) die "unsupported architecture: $arch" ;;
esac

# Resolve the version tag.
if [ "$VERSION" = "latest" ]; then
  VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
  [ -n "$VERSION" ] || die "could not resolve the latest release"
fi
ver_no_v=${VERSION#v}

asset="slink_${ver_no_v}_${os}_${arch}.tar.gz"
base="https://github.com/${REPO}/releases/download/${VERSION}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

say "downloading ${asset} (${VERSION})…"
curl -fsSL "${base}/${asset}" -o "${tmp}/${asset}" || die "download failed: ${base}/${asset}"

# Verify the checksum against the release's checksums.txt.
if curl -fsSL "${base}/checksums.txt" -o "${tmp}/checksums.txt" 2>/dev/null; then
  want=$(grep " ${asset}\$" "${tmp}/checksums.txt" | awk '{print $1}')
  if [ -n "$want" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      got=$(sha256sum "${tmp}/${asset}" | awk '{print $1}')
    else
      got=$(shasum -a 256 "${tmp}/${asset}" | awk '{print $1}')
    fi
    [ "$got" = "$want" ] || die "checksum mismatch for ${asset}"
    say "checksum ok"
  fi
else
  say "warning: no checksums.txt found — skipping verification"
fi

tar -xzf "${tmp}/${asset}" -C "${tmp}"
[ -f "${tmp}/slink" ] || die "archive did not contain the slink binary"
chmod +x "${tmp}/slink"

# Pick an install dir we can write to.
dir="${SLINK_INSTALL_DIR:-}"
if [ -z "$dir" ]; then
  if [ -w /usr/local/bin ] 2>/dev/null; then
    dir=/usr/local/bin
  else
    dir="${HOME}/.local/bin"
    mkdir -p "$dir"
  fi
fi

if [ -w "$dir" ] 2>/dev/null; then
  mv "${tmp}/slink" "${dir}/slink"
else
  say "installing to ${dir} (needs sudo)…"
  sudo mv "${tmp}/slink" "${dir}/slink"
fi

say "✓ installed slink ${VERSION} → ${dir}/slink"
case ":${PATH}:" in
  *":${dir}:"*) ;;
  *) say "  note: ${dir} is not on your PATH — add it, or run ${dir}/slink" ;;
esac
say "  get started: slink login && slink on"
