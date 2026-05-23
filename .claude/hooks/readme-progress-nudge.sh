#!/usr/bin/env bash
# PostToolUse hook (Edit|Write): when the agent edits a file that maps to a
# tracked feature in README.md, and that feature's checkbox is still `[~]` or
# `[ ]`, emit a one-shot reminder to update the scoreboard. The hook itself
# never edits README.md — the agent decides whether the work actually moves
# the marker.
#
# Throttle: once per (session, feature) — same session won't get spammed.
# Patterns matched left-to-right; first match wins.

set -euo pipefail

payload=$(cat)
session_id=$(printf '%s' "$payload" | jq -r '.session_id // empty')
cwd=$(printf '%s' "$payload" | jq -r '.cwd // empty')
file=$(printf '%s' "$payload" | jq -r '.tool_input.file_path // empty')

[ -z "$session_id" ] && exit 0
[ -z "$file" ] && exit 0
[ -z "$cwd" ] && cwd="$PWD"

# Make the path relative to cwd if possible — patterns match the in-repo shape.
rel="${file#$cwd/}"

# Feature map: <regex>=><feature line snippet>.
# The snippet must appear (case-insensitive) on a README line that also has
# `[~]` or `[ ]`. Keep this list small — broad strokes only. `=>` is the
# delimiter so labels can contain pipes / regex alternation.
map=(
  'ffxi-viewer-core/src/hud/menu\.rs$=>Main menu'
  'ffxi-client/src/view_native/text_input\.rs$=>Main menu'
  'ffxi-viewer-core/src/hud/dialog\.rs$=>NPC dialogue'
  'ffxi-viewer-core/src/hud/quick_action\.rs$=>Quick-action bar'
  'ffxi-viewer-core/src/audio\.rs$=>(Music playback|Sound effects)'
  'ffxi-viewer-core/src/minimap/.*\.rs$=>Minimap'
  'ffxi-viewer-core/src/nameplate.*\.rs$=>Nameplate billboards'
  'ffxi-viewer-core/src/(sun_moon|skybox|weather|weather_fx)\.rs$=>Sky'
  'ffxi-client/src/view_native/camera_collision\.rs$=>Chase camera'
  'ffxi-client/src/launcher\.rs$=>Character (create|delete) flow'
  'ffxi-proto/src/.*\.rs$=>opcode'
  'ffxi-client/src/reactor\.rs$=>action dispatch'
)

feature=""
for entry in "${map[@]}"; do
  pat="${entry%%=>*}"
  label="${entry##*=>}"
  if [[ "$rel" =~ $pat ]]; then
    feature="$label"
    break
  fi
done

[ -z "$feature" ] && exit 0

readme="$cwd/README.md"
[ -f "$readme" ] || exit 0

# Find the README line that mentions the feature snippet AND carries an
# incomplete glyph. If the line is already `[x]` (or the feature has no entry
# at all), exit silently.
hit=$(grep -inE '^\s*-\s*`\[(~| )\]`' "$readme" | grep -iE "$feature" | head -1 || true)
[ -z "$hit" ] && exit 0

# Throttle: at most one nudge per (session, feature) per harness run.
slug=$(printf '%s' "$feature" | tr '[:upper:] /' '[:lower:]--' | tr -cd 'a-z0-9-')
throttle_dir="${TMPDIR:-/tmp}/claude-readme-nudge"
mkdir -p "$throttle_dir"
marker="$throttle_dir/${session_id}.${slug}"
[ -f "$marker" ] && exit 0
touch "$marker"

readme_line=$(printf '%s' "$hit" | cut -d: -f2-)
msg=$(printf 'Edited %s — maps to README scoreboard feature "%s".\nCurrent line:\n  %s\nIf this lands more of that subsystem, flip the marker in README.md in the same commit.' \
  "$rel" "$feature" "$readme_line")

jq -n --arg m "$msg" '{systemMessage: $m}'
