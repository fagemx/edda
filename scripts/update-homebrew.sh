#!/usr/bin/env bash
# Update the Homebrew formula in fagemx/homebrew-tap with SHA256 hashes
# from a GitHub Release.
#
# Usage:
#   ./scripts/update-homebrew.sh 0.1.0
#   ./scripts/update-homebrew.sh 0.1.0 /path/to/homebrew-tap

set -euo pipefail

VERSION="${1:?Usage: update-homebrew.sh <version> [tap-dir]}"
TAG="v${VERSION}"
REPO="fagemx/edda"
TAP_DIR="${2:-}"
FORMULA_OUT=""

# Fetch SHA256 for a given target
fetch_hash() {
  local target="$1"
  local asset="edda-${TAG}-${target}.tar.gz.sha256"
  gh release download "$TAG" --repo "$REPO" --pattern "$asset" --output - | awk '{print $1}'
}

echo "Fetching SHA256 hashes for ${TAG}..."

HASH_MACOS_ARM=$(fetch_hash "aarch64-apple-darwin")
HASH_MACOS_X86=$(fetch_hash "x86_64-apple-darwin")
HASH_LINUX_ARM=$(fetch_hash "aarch64-unknown-linux-gnu")
HASH_LINUX_X86=$(fetch_hash "x86_64-unknown-linux-gnu")

echo "  macOS arm64:  ${HASH_MACOS_ARM}"
echo "  macOS x86_64: ${HASH_MACOS_X86}"
echo "  Linux arm64:  ${HASH_LINUX_ARM}"
echo "  Linux x86_64: ${HASH_LINUX_X86}"

# Generate formula
generate_formula() {
cat <<RUBY
class Edda < Formula
  desc "Decision memory for coding agents"
  homepage "https://github.com/fagemx/edda"
  version "${VERSION}"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/fagemx/edda/releases/download/v#{version}/edda-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "${HASH_MACOS_ARM}"
    else
      url "https://github.com/fagemx/edda/releases/download/v#{version}/edda-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "${HASH_MACOS_X86}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/fagemx/edda/releases/download/v#{version}/edda-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "${HASH_LINUX_ARM}"
    else
      url "https://github.com/fagemx/edda/releases/download/v#{version}/edda-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${HASH_LINUX_X86}"
    end
  end

  def install
    bin.install "edda"
  end

  test do
    assert_match "edda #{version}", shell_output("#{bin}/edda --version")
  end
end
RUBY
}

if [ -n "$TAP_DIR" ]; then
  FORMULA_OUT="${TAP_DIR}/Formula/edda.rb"
  generate_formula > "$FORMULA_OUT"
  echo ""
  echo "Formula written to: ${FORMULA_OUT}"
else
  echo ""
  echo "--- Formula/edda.rb ---"
  generate_formula
fi
