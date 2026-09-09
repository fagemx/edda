#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/tap/Formula"

cat > "$TMP/bin/curl" <<'EOF'
#!/bin/sh
set -eu
out=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output)
      out=$2
      shift 2
      ;;
    --retry|--user-agent)
      shift 2
      ;;
    -*)
      shift
      ;;
    *)
      url=$1
      shift
      ;;
  esac
done
[ "$url" = "https://static.crates.io/crates/edda/edda-9.8.7.crate" ]
[ -n "$out" ]
printf 'homebrew-source-fixture' > "$out"
EOF
chmod +x "$TMP/bin/curl"

PATH="$TMP/bin:$PATH" bash "$ROOT/scripts/update-homebrew.sh" 9.8.7 "$TMP/tap" >/dev/null
FORMULA="$TMP/tap/Formula/edda.rb"

if command -v sha256sum >/dev/null 2>&1; then
  EXPECTED=$(printf 'homebrew-source-fixture' | sha256sum | awk '{print $1}')
else
  EXPECTED=$(printf 'homebrew-source-fixture' | shasum -a 256 | awk '{print $1}')
fi

grep -F 'url "https://static.crates.io/crates/edda/edda-9.8.7.crate"' "$FORMULA" >/dev/null
grep -F "sha256 \"$EXPECTED\"" "$FORMULA" >/dev/null
grep -F 'depends_on "pkgconf" => :build' "$FORMULA" >/dev/null
grep -F 'depends_on "rust" => :build' "$FORMULA" >/dev/null
grep -F 'depends_on "openssl@3"' "$FORMULA" >/dev/null
grep -F 'system "cargo", "install", *std_cargo_args' "$FORMULA" >/dev/null

if grep -Eq '^[[:space:]]+version "' "$FORMULA"; then
  echo "test-update-homebrew: explicit version is redundant with the URL" >&2
  exit 1
fi
if grep -F 'releases/download' "$FORMULA" >/dev/null; then
  echo "test-update-homebrew: generated formula still uses glibc-bound binaries" >&2
  exit 1
fi
if PATH="$TMP/bin:$PATH" bash "$ROOT/scripts/update-homebrew.sh" v9.8.7 "$TMP/tap" >/dev/null 2>&1; then
  echo "test-update-homebrew: accepted a version with a leading v" >&2
  exit 1
fi

bash -n "$ROOT/scripts/update-homebrew.sh"
echo "test-update-homebrew: OK"
