#!/usr/bin/env bash
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "Usage: postbuild-cpp.sh <output-header-path>" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Resolve output path to absolute (it may be relative to the caller's cwd)
case "$1" in
  /*) OUTPUT_FILE="$1" ;;
  *)  OUTPUT_FILE="$PWD/$1" ;;
esac

mkdir -p "$(dirname "$OUTPUT_FILE")"
TMPFILE=$(mktemp)
trap 'rm -f "$TMPFILE"' EXIT

# Run cbindgen from the parser directory so cargo finds its vendored config
(cd "$ROOT_DIR/parser" && cbindgen --quiet --output "$TMPFILE")

# Extract version from parser/Cargo.toml
VERSION=$(grep -m1 '^\s*version\s*=' "$ROOT_DIR/parser/Cargo.toml" | sed 's/.*"\([^"]*\)".*/\1/')
if [ -z "$VERSION" ]; then
  echo "Error: Cannot find parser version in parser/Cargo.toml" >&2
  exit 1
fi

# Parse semver components
MAJOR="${VERSION%%.*}"
REST="${VERSION#*.}"
MINOR="${REST%%.*}"
REST="${REST#*.}"
if [[ "$REST" == *-* ]]; then
  PATCH="${REST%%-*}"
  PRERELEASE="${REST#*-}"
else
  PATCH="$REST"
  PRERELEASE=""
fi

# Read methods from YAML (skip comments, blank lines, and document start marker)
mapfile -t METHODS < <(grep '^- ' "$ROOT_DIR/macros/constants/methods.yml" | sed 's/^- //' | tr '-' '_')

# Build the replacement block
REPLACEMENT=$(
  printf '#define MILO_VERSION "%s"\n' "$VERSION"
  printf '#define MILO_VERSION_MAJOR %s\n' "$MAJOR"
  printf '#define MILO_VERSION_MINOR %s\n' "$MINOR"
  printf '#define MILO_VERSION_PATCH %s\n' "$PATCH"
  printf '#define MILO_VERSION_PRERELEASE "%s"\n' "$PRERELEASE"
  printf '\n'
  printf '#define MILO_METHODS_MAP(EACH) \\\n'
  for i in "${!METHODS[@]}"; do
    printf '  EACH(%d, %s, %s) \\\n' "$i" "${METHODS[$i]}" "${METHODS[$i]}"
  done
  printf '\n'
  printf 'namespace milo_parser {\n'
  printf '\n'
  printf 'struct Parser;'
)

# Replace "namespace milo_parser {" with the preamble, then collapse 3+ blank lines to 1
awk -v replacement="$REPLACEMENT" '
  /^namespace milo_parser \{$/ && !replaced {
    print replacement
    replaced = 1
    next
  }
  { print }
' "$TMPFILE" \
| awk '/^$/ { blank++; if (blank <= 1) print; next } { blank = 0; print }' \
> "$OUTPUT_FILE"
