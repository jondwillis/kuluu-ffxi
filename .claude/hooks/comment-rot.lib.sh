#!/usr/bin/env bash
# Shared comment heuristics for the PreToolUse inline nudge
# (comment-rot-reminder.sh) and the Stop self-review checkpoint
# (stop.d/30-comments.sh). Source this; do not execute it.
#
# Stance: this project bans narrative code comments by default. The code
# itself — names, types, asserts — carries WHAT and HOW; a comment is
# kept only when it carries a WHY that cannot be encoded, cites an
# external/vendor/protocol source, or justifies an `unsafe` block. So the
# scan surfaces EVERY new plain `//` comment outside that carve-out, with
# a sharper label for the worst families (narrative, dead code, …) where
# a pattern matches. The agent makes the keep/delete call.
#
# Doc comments (/// //!) are NOT exempt — they ramble and rot like any
# prose. They run through the rot families (narrative, decoration,
# restated offsets, unenforced invariants), so a stale or rambling doc is
# flagged; only a tight, accurate one escapes the blanket catch-all that
# the plain `//` ban applies.
#
# Carve-out (kept, never flagged): SAFETY justifications, vendor/spec
# citations, and license headers — tune them in CR_RE_ALLOWED so the two
# hooks never drift apart.

# Allowed comments — stripped before flagging so the ban doesn't fight
# the project's own conventions: `// SAFETY:` blocks (required by
# clippy::undocumented_unsafe_blocks), citations to the vendored
# authoritative sources (the LSB-boundary convention), and SPDX /
# copyright headers. Doc comments are deliberately absent — see above.
CR_RE_ALLOWED='(SAFETY|#[[:space:]]*Safety|SPDX-|[Cc]opyright|\bvendor/|\bresearch/|\bLSB\b|Phoenix|POLUtils|XiEvents|XiPackets|atom0s|\bRFC[ -]?[0-9])'

# Doc-comment lines (/// //!). Scanned by the rot families above, but
# excluded from the blanket catch-all so a clean one-line API doc isn't
# treated as a banned narrative comment.
CR_RE_DOC='^[[:space:]]*//[/!]'

# Narrative / session-history / temporal — describes how the code got
# here or a passing moment, not what is true now.
CR_RE_NARRATIVE='(why we |we (abandoned|switched|re-?wrote|removed|replaced|migrated)|no longer|used to |previously|originally|prior to |\bregression\b|stage [0-9]|phase [0-9]|\bfor now\b|for the moment|this replaces|the (old|previous) )'

# Decoration — renders as noise (or not at all) in a code comment:
# markdown bold, long rule separators, markdown headings in //! ///.
CR_RE_DECORATION='(\*\*[^*]+\*\*|[=_*-]{8,}|//[!/]?[[:space:]]*#{1,6}[[:space:]])'

# Commented-out code — should be deleted, not parked.
CR_RE_DEADCODE='^[[:space:]]*//[[:space:]]*(let |let mut |fn |pub |if |for |while |match |self\.|return[ ;]|use |impl |struct |enum |\}|[A-Za-z_][A-Za-z0-9_:]*\(.*\)[;,]?[[:space:]]*$)'

# Invariant / safety claims — fine IF enforced by code (assert, newtype,
# enum, or a vendor citation); a bare prose claim silently misleads.
CR_RE_INVARIANT='\b(always|never|guaranteed|cannot happen|can.?t happen|impossible|must be|unreachable|infallible|won.?t panic)\b'

# Code / offset / formula restated in prose — de-syncs when the literal
# next to it changes.
CR_RE_DESYNC='(0x[0-9A-Fa-f]+[^,]*=|=[^=]*0x[0-9A-Fa-f]+|[0-9]+[[:space:]]*[-+*][[:space:]]*[0-9]+[[:space:]]*=)'

# Bare hex literal in a comment — a magic number that almost always wants a
# named const (after which the comment disappears, and the same literal in
# adjacent code reads itself). High-signal in this protocol-heavy tree.
# Runs after DESYNC so `0xNN =` keeps its sharper restated-offset label.
CR_RE_MAGIC='0x[0-9A-Fa-f]+'

# Bare hex literal used in a CODE comparison or bitmask (`== 0xNN`, `& 0xNN`,
# `>= 0xNN`, …) — a magic value that wants a named const so the test reads
# itself (e.g. `b == 0x0b` -> `b == CC_SELECTION`). Deliberately narrow:
# operator-adjacent hex only, so idiomatic raw byte offsets (`data[0x1A]`,
# `buf.get(0x1A)`), `const X = 0xNN` defs, and `0xNN =>` match arms are spared
# in this protocol-heavy tree.
CR_RE_CODE_MAGIC='(==|!=|<=|>=|[[:space:]][&|^<>][[:space:]])[[:space:]]*0x[0-9A-Fa-f]+'

# scan_comment_rot: read plain source text on stdin, print labeled
# findings to stdout (one per line, capped per category), and return 0
# if anything matched, 1 if clean. URLs and the allowed carve-out are
# excluded up front; the worst families get a sharp label and everything
# else remaining falls through to a generic [comment] flag.
scan_comment_rot() {
  local comments flaggable found=1 hits matched rest

  # Every line comment, minus URLs (so prose links don't trip).
  comments=$(grep -E '//' | grep -vE 'https?://' || true)
  [ -z "$comments" ] && return 1

  # Drop the allowed carve-out before flagging anything.
  flaggable=$(printf '%s\n' "$comments" | grep -vE "$CR_RE_ALLOWED" || true)
  [ -z "$flaggable" ] && return 1

  matched=''
  _cr_emit() { # $1=label  $2=regex
    hits=$(printf '%s\n' "$flaggable" | grep -iE "$2" | head -4 || true)
    [ -z "$hits" ] && return 1
    matched+="$hits"$'\n'
    printf '%s\n' "$hits" | sed -E "s#^[[:space:]]*#  [$1] #"
    return 0
  }

  _cr_emit 'narrative/history'    "$CR_RE_NARRATIVE"  && found=0
  _cr_emit 'code/offset restated' "$CR_RE_DESYNC"     && found=0
  _cr_emit 'magic literal — name as const' "$CR_RE_MAGIC" && found=0
  _cr_emit 'decoration'           "$CR_RE_DECORATION" && found=0
  _cr_emit 'commented-out code'   "$CR_RE_DEADCODE"   && found=0
  _cr_emit 'unenforced invariant' "$CR_RE_INVARIANT"  && found=0

  # Catch-all: any remaining PLAIN `//` comment the families didn't label
  # (doc comments are excluded here — a clean API doc isn't a banned
  # narrative comment). The default is no narrative comment at all, so
  # surface these — the agent encodes the intent, cites a source, or deletes.
  rest=$(printf '%s\n' "$flaggable" | grep -vxF -f <(printf '%s' "$matched") \
    | grep -vE "$CR_RE_DOC" | head -6 || true)
  if [ -n "$rest" ]; then
    printf '%s\n' "$rest" | sed -E "s#^[[:space:]]*#  [comment] #"
    found=0
  fi

  return $found
}

# scan_code_magic: read plain source text on stdin, flag CODE lines (not
# comments) that compare or mask against a bare hex literal (see
# CR_RE_CODE_MAGIC). Prints labeled findings, returns 0 if any matched, 1 if
# clean. Complements scan_comment_rot, which owns hex inside comments.
scan_code_magic() {
  local hits
  # Drop full-line comments and strip trailing ` // …` comments so comment hex
  # stays owned by scan_comment_rot, then match the operator-adjacent shape.
  hits=$(grep -vE "$CR_RE_DOC|^[[:space:]]*//" \
    | sed -E 's#[[:space:]]//[^"]*$##' \
    | grep -E "$CR_RE_CODE_MAGIC" \
    | grep -vE '=>|0x[0-9A-Fa-f]+[[:space:]]*\.\.|\.\.=?[[:space:]]*0x' \
    | grep -vE "$CR_RE_ALLOWED" \
    | head -6 || true)
  [ -z "$hits" ] && return 1
  printf '%s\n' "$hits" | sed -E 's#^[[:space:]]*#  [code magic literal — name as const] #'
  return 0
}
