#!/usr/bin/env bash
#
# Fetch the XIM browser FFXI client source for local, read-only reference.
#
# XIM (https://xim.pages.dev/) is GPL-3 and is NOT redistributed by this repo.
# It lives under research/xim/ (gitignored) purely as a feature reference while
# re-implementing vanilla client behavior. See research/README.md.
#
# Usage:  research/fetch-xim.sh
set -euo pipefail

cd "$(dirname "$0")"

URL="https://xim.pages.dev/source.zip"
DEST="xim"

command -v curl >/dev/null || { echo "error: curl not found" >&2; exit 1; }
command -v unzip >/dev/null || { echo "error: unzip not found" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Fetching $URL ..."
curl -fsSL "$URL" -o "$TMP/source.zip"

# The zip extracts flat (project root at top level), so unzip straight into a
# fresh research/xim/.
rm -rf "$DEST"
mkdir -p "$DEST"
unzip -q "$TMP/source.zip" -d "$DEST"

echo "XIM source extracted to research/$DEST/ (gitignored, reference-only)."
