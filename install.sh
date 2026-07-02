#!/bin/sh
# tmail installer — fetch a prebuilt release binary for this platform.
#
#   curl --proto '=https' --tlsv1.2 -LsSf \
#     https://raw.githubusercontent.com/raymond-UI/tmail/main/install.sh | sh
#
# Environment overrides:
#   TMAIL_VERSION      pin a release tag (default: latest, e.g. v0.1.0)
#   TMAIL_INSTALL_DIR  install location (default: /usr/local/bin if writable,
#                      else $HOME/.local/bin)
#
# Windows users: download the .zip from the Releases page instead.
set -eu

REPO="raymond-UI/tmail"
BIN="tmail"

err() { printf 'install: error: %s\n' "$1" >&2; exit 1; }
info() { printf 'install: %s\n' "$1" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

have curl || err "curl is required"
have tar || err "tar is required"

# --- detect platform -> Rust target triple --------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) err "unsupported architecture: $arch" ;;
esac
case "$os" in
  Darwin) target="${arch}-apple-darwin" ;;
  Linux)
    # Alpine and other musl distros need the musl build.
    if have ldd && ldd --version 2>&1 | grep -qi musl; then
      target="${arch}-unknown-linux-musl"
    else
      target="${arch}-unknown-linux-gnu"
    fi
    ;;
  *) err "unsupported OS: $os — download manually from https://github.com/$REPO/releases" ;;
esac

# --- resolve version ------------------------------------------------------
version="${TMAIL_VERSION:-}"
if [ -z "$version" ]; then
  # Resolve "latest" from the github.com redirect, not the REST API: the
  # unauthenticated API is rate-limited per IP (60/h) and 403s on busy
  # machines/CI, while the redirect is not metered.
  redirect="$(curl -sSfL -o /dev/null -w '%{url_effective}' \
    "https://github.com/$REPO/releases/latest" 2>/dev/null)" || redirect=""
  case "$redirect" in
    */tag/*) version="${redirect##*/tag/}" ;;
  esac
fi
if [ -z "$version" ]; then
  # Fall back to the API for environments where the redirect is intercepted.
  version="$(curl -sSfL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name" *: *"([^"]+)".*/\1/')" || version=""
  [ -n "$version" ] || err "could not determine the latest version; set TMAIL_VERSION"
fi

archive="${BIN}-${target}.tar.gz"
base="https://github.com/$REPO/releases/download/$version"
url="$base/$archive"
sum_url="$base/${BIN}-${target}.sha256"

# --- install dir ----------------------------------------------------------
dir="${TMAIL_INSTALL_DIR:-}"
if [ -z "$dir" ]; then
  if [ -w /usr/local/bin ]; then dir="/usr/local/bin"; else dir="$HOME/.local/bin"; fi
fi
mkdir -p "$dir" || err "cannot create install dir: $dir"

# --- download, verify, extract, install -----------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

info "downloading $BIN $version ($target)"
curl -sSfL "$url" -o "$tmp/$archive" || err "download failed: $url"

# Verify the SHA-256 when the checksum asset is present.
if curl -sSfL "$sum_url" -o "$tmp/$archive.sha256" 2>/dev/null; then
  expected="$(awk '{print $1}' "$tmp/$archive.sha256")"
  if have sha256sum; then
    actual="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
  elif have shasum; then
    actual="$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')"
  else
    actual=""
  fi
  if [ -n "$actual" ] && [ "$expected" != "$actual" ]; then
    err "checksum mismatch (expected $expected, got $actual)"
  fi
fi

tar -xzf "$tmp/$archive" -C "$tmp" || err "failed to extract archive"
binpath="$(find "$tmp" -type f -name "$BIN" | head -1)"
[ -n "$binpath" ] || err "binary '$BIN' not found inside the archive"
chmod +x "$binpath"
mv "$binpath" "$dir/$BIN" || err "failed to install to $dir"

info "installed $BIN -> $dir/$BIN"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) info "note: $dir is not on your PATH — add:  export PATH=\"$dir:\$PATH\"" ;;
esac
"$dir/$BIN" --version >&2 2>/dev/null || true
