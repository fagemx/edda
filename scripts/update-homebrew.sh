#!/usr/bin/env bash
# Update the Homebrew formula in fagemx/homebrew-tap from the published
# crates.io source package. Building inside Homebrew avoids tying Linux users
# to the glibc version of GitHub's release runner.
#
# Usage:
#   ./scripts/update-homebrew.sh 0.6.0
#   ./scripts/update-homebrew.sh 0.6.0 /path/to/homebrew-tap

set -euo pipefail

VERSION="${1:?Usage: update-homebrew.sh <version> [tap-dir]}"
TAP_DIR="${2:-}"
FORMULA_OUT=""

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: version must be a bare semantic version (for example 0.6.0)" >&2
  exit 2
fi

CRATE_URL="https://static.crates.io/crates/edda/edda-${VERSION}.crate"
CRATE_FILE=$(mktemp)
cleanup() {
  rm -f "$CRATE_FILE"
}
trap cleanup EXIT

curl -fsSL --retry 3 --user-agent "edda-homebrew-updater/1.0" \
  --output "$CRATE_FILE" "$CRATE_URL"

if command -v sha256sum >/dev/null 2>&1; then
  SOURCE_HASH=$(sha256sum "$CRATE_FILE" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
  SOURCE_HASH=$(shasum -a 256 "$CRATE_FILE" | awk '{print $1}')
else
  echo "error: sha256sum or shasum is required" >&2
  exit 1
fi

if [[ ! "$SOURCE_HASH" =~ ^[0-9a-f]{64}$ ]]; then
  echo "error: invalid SHA256 for ${CRATE_URL}" >&2
  exit 1
fi

echo "Source: ${CRATE_URL}"
echo "SHA256: ${SOURCE_HASH}"

generate_formula() {
cat <<RUBY
class Edda < Formula
  desc "Decision memory for coding agents"
  homepage "https://github.com/fagemx/edda"
  url "${CRATE_URL}"
  sha256 "${SOURCE_HASH}"
  license any_of: ["MIT", "Apache-2.0"]

  depends_on "pkgconf" => :build
  depends_on "rust" => :build
  depends_on "openssl@3"

  def install
    system "cargo", "install", *std_cargo_args
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
  echo "Formula written to: ${FORMULA_OUT}"
else
  echo ""
  echo "--- Formula/edda.rb ---"
  generate_formula
fi
