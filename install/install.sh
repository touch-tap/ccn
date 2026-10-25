#!/bin/sh
# Installs ccn, the Claude Code Notifier CLI, on macOS or Linux.
#
#   curl -fsSL https://claudecodenotifier.com/install.sh | sh              # install only
#   curl -fsSL https://claudecodenotifier.com/install.sh | sh -s 482913    # install and pair
#
# CCN_VERSION=0.2.0 picks a release (default: the latest). CCN_INSTALL_DIR overrides ~/.local/bin.
set -eu

repo="touch-tap/ccn"
code="${1:-}"
dir="${CCN_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { printf 'ccn install: %s\n' "$*" >&2; exit 1; }

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-musl" ;;
  *) fail "this script is for macOS and Linux. On Windows run: irm https://claudecodenotifier.com/install.ps1 | iex" ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64) arch="x86_64" ;;
  *) fail "unsupported processor $(uname -m)" ;;
esac
# A shell running under Rosetta reports x86_64; the native build is better
if [ "$os" = apple-darwin ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
  arch="aarch64"
fi
asset="ccn-$arch-$os.tar.gz"

if [ -n "${CCN_DOWNLOAD_BASE:-}" ]; then
  base="$CCN_DOWNLOAD_BASE"   # for testing the script against a local build
elif [ -n "${CCN_VERSION:-}" ]; then
  base="https://github.com/$repo/releases/download/v${CCN_VERSION#v}"
else
  base="https://github.com/$repo/releases/latest/download"
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
else
  fail "needs curl or wget"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "Downloading ccn for $arch-$os..."
fetch "$base/$asset" "$tmp/$asset" || fail "could not download $base/$asset"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || fail "could not download the checksums"

expected="$(awk -v f="$asset" '$2 == f { print $1 }' "$tmp/SHA256SUMS")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$asset" | awk '{ print $1 }')"
else
  actual="$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')"
fi
[ -n "$expected" ] && [ "$expected" = "$actual" ] || fail "checksum mismatch for $asset"

tar xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$dir"
# Write beside the target and rename, so a hook running the old binary is never half-overwritten
cp "$tmp/ccn" "$dir/.ccn.new"
chmod 755 "$dir/.ccn.new"
mv "$dir/.ccn.new" "$dir/ccn"
say "Installed $("$dir/ccn" --version) to $dir/ccn"

ccn="ccn"
case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    ccn="$dir/ccn"
    say ""
    say "$dir is not on your PATH. The hooks don't need it, but to type ccn yourself add this to your shell profile:"
    say "  export PATH=\"$dir:\$PATH\""
    ;;
esac

if [ -n "$code" ]; then
  say ""
  # stdin is the piped script, so give pair the terminal for any question it asks
  if [ -r /dev/tty ]; then
    exec "$dir/ccn" pair "$code" < /dev/tty
  else
    exec "$dir/ccn" pair "$code"
  fi
else
  say ""
  say "Next: open Claude Code Notifier on your phone, add a computer, and run"
  say "  $ccn pair <the code on your phone>"
fi
