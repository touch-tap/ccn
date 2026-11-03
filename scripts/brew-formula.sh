#!/bin/sh
# Prints the Homebrew formula for a release: scripts/brew-formula.sh 0.2.0 dist/SHA256SUMS
# Copy the output to touch-tap/homebrew-tap as Formula/ccn.rb once the release is published.
set -eu
version=$1
sums=$2
[ -r "$sums" ] || { echo "brew-formula: cannot read $sums" >&2; exit 1; }
# `<hash>  <name>` or `<hash> *<name>`. Each is read here, not inside the heredoc, where a missing
# one would print an empty sha256 and still exit 0
sha() {
  s=$(awk -v f="ccn-$1.tar.gz" '{ n = $2; sub(/^\*/, "", n) } n == f { print $1; exit }' "$sums")
  case "$s" in
    [0-9a-fA-F]*) [ ${#s} -eq 64 ] && { echo "$s"; return; } ;;
  esac
  echo "brew-formula: no sha256 for ccn-$1.tar.gz in $sums" >&2
  exit 1
}
base="https://github.com/touch-tap/ccn/releases/download/v$version"
mac_arm=$(sha aarch64-apple-darwin)
mac_intel=$(sha x86_64-apple-darwin)
linux_arm=$(sha aarch64-unknown-linux-musl)
linux_intel=$(sha x86_64-unknown-linux-musl)

cat <<RUBY
class Ccn < Formula
  desc "Phone notifications and approvals for Claude Code, from any terminal"
  homepage "https://claudecodenotifier.com/cli"
  version "$version"

  on_macos do
    on_arm do
      url "$base/ccn-aarch64-apple-darwin.tar.gz"
      sha256 "$mac_arm"
    end
    on_intel do
      url "$base/ccn-x86_64-apple-darwin.tar.gz"
      sha256 "$mac_intel"
    end
  end

  on_linux do
    on_arm do
      url "$base/ccn-aarch64-unknown-linux-musl.tar.gz"
      sha256 "$linux_arm"
    end
    on_intel do
      url "$base/ccn-x86_64-unknown-linux-musl.tar.gz"
      sha256 "$linux_intel"
    end
  end

  def install
    bin.install "ccn"
  end

  def caveats
    <<~EOS
      Run \`ccn\` to finish setting up: it installs the Claude Code hooks (desktop
      notifications from then on) and shows a QR code to pair your phone, or run
        ccn pair <the code on your phone>
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ccn --version")
  end
end
RUBY
