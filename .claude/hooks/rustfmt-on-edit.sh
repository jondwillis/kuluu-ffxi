#!/usr/bin/env bash
# PostToolUse hook (Edit|Write): format the just-edited Rust file so
# whitespace/import drift never reaches the pre-push `cargo fmt --check`
# gate (scripts/checks.sh fmt) as a late failure.
#
# Edition is hardcoded 2021 — the single workspace edition (Cargo.toml).
# There is no rustfmt.toml, so rustfmt's defaults match what `cargo fmt`
# would produce per crate.
#
# `rustfmt --check` runs first: already-formatted files (the common case)
# are left untouched, so the on-disk mtime doesn't churn and the harness's
# file-state tracker isn't tripped. Only a genuinely-unformatted file is
# rewritten, and only then does Claude get a stderr note to re-read it.
#
# Exits 0 unconditionally — formatting is a convenience, not a gate; a
# parse error mid-edit must not block the tool call.

set -uo pipefail

payload=$(cat)
file=$(printf '%s' "$payload" | /usr/bin/python3 -c \
  'import json,sys; d=json.load(sys.stdin); print(d.get("tool_input",{}).get("file_path",""), end="")' \
  2>/dev/null || true)

case "$file" in
  *.rs) ;;
  *) exit 0 ;;
esac

[ -f "$file" ] || exit 0

case "$file" in
  */vendor/*|*/target/*) exit 0 ;;
esac

if rustfmt --edition 2021 --check "$file" >/dev/null 2>&1; then
  exit 0
fi

before=$(shasum "$file" 2>/dev/null | awk '{print $1}')
rustfmt --edition 2021 "$file" >/dev/null 2>&1 || true
after=$(shasum "$file" 2>/dev/null | awk '{print $1}')

if [ -n "$before" ] && [ "$before" != "$after" ]; then
  echo "[rustfmt] reformatted $file — re-read it before your next edit to that file." >&2
fi

exit 0
