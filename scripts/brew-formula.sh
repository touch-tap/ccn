#!/bin/sh
# Prints the Homebrew formula for a release: scripts/brew-formula.sh 0.2.0 dist/SHA256SUMS
# Copy the output to touch-tap/homebrew-tap as Formula/ccn.rb once the release is published.
set -eu
version=$1
sums=$2
sha() { awk -v f="ccn-$1.tar.gz" '$2 == f { print $1 }' "$sums"; }
base="https://github.com/touch-tap/ccn/releases/download/v$version"

cat <<RUBY
class Ccn < Formula
  desc "Phone notifications and approvals for Claude Code, from any terminal"
  homepage "https://claudecodenotifier.com/cli"
  version "$version"

  on_macos do
    on_arm do
      url "$base/ccn-aarch64-apple-darwin.tar.gz"
      sha256 "$(sha aarch64-apple-darwin)"
    end
    on_intel do
      url "$base/ccn-x86_64-apple-darwin.tar.gz"
      sha256 "$(sha x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "$base/ccn-aarch64-unknown-linux-musl.tar.gz"
      sha256 "$(sha aarch64-unknown-linux-musl)"
    end
    on_intel do
      url "$base/ccn-x86_64-unknown-linux-musl.tar.gz"
      sha256 "$(sha x86_64-unknown-linux-musl)"
    end
  end

  def install
    bin.install "ccn"
  end

  def caveats
    <<~EOS
      Connect your phone: run
        ccn
      and scan the QR code with the phone's camera, or run
        ccn pair <the code on your phone>
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ccn --version")
  end
end
RUBY
