#!/usr/bin/env bash
set -euo pipefail

CONFIGURATION="${1:?Usage: postbuild-cpp.sh <release|debug>}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

HEADER_FILE="$ROOT_DIR/dist/cpp/$CONFIGURATION/milo.h"

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
' "$HEADER_FILE" \
| awk '/^$/ { blank++; if (blank <= 1) print; next } { blank = 0; print }' \
> "${HEADER_FILE}.tmp"

mv "${HEADER_FILE}.tmp" "$HEADER_FILE"
