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
# hooks never drift apart. One exception rides above the carve-out: a
# citation pinned to a `:NNN` line number (CR_RE_CITE_LINE), which rots on
# the next submodule bump and so is flagged even though the citation is kept.

# Allowed comments — stripped before flagging so the ban doesn't fight
# the project's own conventions: `// SAFETY:` blocks (required by
# clippy::undocumented_unsafe_blocks), citations to the vendored
# authoritative sources (the LSB-boundary convention), and SPDX /
# copyright headers. Doc comments are deliberately absent — see above.
CR_RE_ALLOWED='(SAFETY|#[[:space:]]*Safety|SPDX-|[Cc]opyright|\bvendor/|\bresearch/|\bLSB\b|Phoenix|POLUtils|XiEvents|XiPackets|atom0s|FFXiMain|\bxim\b|\bRFC[ -]?[0-9]|[A-Za-z][A-Za-z0-9_]*\.(cpp|hpp|c|cc|cs|lua|sql|py|rs|xml|json|kt|js|md)\b)'

# The source-file alternative above matches a bare decompile/vendor filename
# (CYyGenerator.cpp, ParticleUpdaters.kt): the tree cites retail decompiles by
# basename + symbol, and a filename reference is a citation. Line-pinned
# variants stay flagged — CR_RE_CITE_LINE judges the full comment set before
# the carve-out is applied.

# Doc-comment lines (/// //!). Scanned by the rot families above, but
# excluded from the blanket catch-all so a clean one-line API doc isn't
# treated as a banned narrative comment.
CR_RE_DOC='^[[:space:]]*//[/!]'

# A pin into a dependency named with its version (`rodio-0.22.2 src/x.rs:56`)
# does not rot: the version fixes the line. Lines carrying a semver are exempt.
CR_RE_VERSIONED='[0-9]+\.[0-9]+\.[0-9]+'

# Any source citation pinned to a LINE NUMBER, full path or bare basename
# (`research/xim Actor.kt:361`, `char_update.cpp:339`). The citation itself is
# required by the LSB-boundary convention (hence CR_RE_ALLOWED keeps it) — but
# the `:NNN` suffix silently decays the moment the submodule advances, and
# nothing in the build catches it. A symbol anchor (function/struct/enum name)
# survives upstream edits and is greppable; a line number is a promise the
# submodule pin does not keep. Matched BEFORE the allow-list strip, since the
# allow-list is what would otherwise exempt these.
CR_RE_CITE_LINE='(^|[^A-Za-z0-9_])[A-Za-z0-9_.-]*\.(cpp|h|hpp|c|cc|cs|lua|sql|py|rs|xml|json|kt|js|md)([[:space:]]+[A-Za-z0-9_:./-]+){0,3}[[:space:]]*\(?:[0-9]+'

# Retail-binary addresses. An RVA/VA is a fact about ONE FFXiMain.dll build
# (the offsets moved between horizonxi-2023 and retail-2026-09), so the
# comment block carrying one must name that build: a KNOWN_CLIENTS row from
# ffxi-dat/src/client_profile.rs (read at check time, never copied here) or
# the DLL SHA-256 (CR_BUILD_SHA_MIN_HEX hex digits or more). An explicit
# `RVA 0x…`/`VA 0x…` is always judged; a bare 0x10xxxxxx token is the VA
# shape but also a DAT magic, so it is judged only when the block talks about
# the binary (CR_RE_BIN_CTX), and CR_BIN_IMAGE_BASE itself (the VA/RVA
# relation) is exempt. Hex classes are spelled out because BWK awk and mawk
# have neither \b nor {n}; the patterns reach awk through -v, whose escape
# processing differs per awk, so they carry no backslash (`[.]`, not `\.`).
CR_RE_BIN_ADDR='R?VA[[:space:]]*[=:@]?[[:space:]]*0x[0-9A-Fa-f]+'
CR_RE_BIN_BARE='^0x10[0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f]$'
CR_BIN_IMAGE_BASE='0x10000000'
CR_RE_BIN_CTX='FFXiMain|ffximain|[.]text|POL1'
CR_BUILD_SHA_MIN_HEX=12
CR_RE_HEX_TOKEN=$(i=0; while [ "$i" -lt "$CR_BUILD_SHA_MIN_HEX" ]; do printf '[0-9A-Fa-f]'; i=$((i + 1)); done)
CR_CLIENT_PROFILE=${CR_CLIENT_PROFILE:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/ffxi-dat/src/client_profile.rs"}

# cr_known_client_rows: print the KNOWN_CLIENTS row names, one per line.
# Returns 2 when the registry is unreadable or names no rows, so callers can
# hard-error instead of silently judging every address as unscoped.
cr_known_client_rows() {
  local rows
  rows=$(grep -oE 'name: "[^"]+"' "$CR_CLIENT_PROFILE" 2>/dev/null | sed -E 's/^name: "//; s/"$//') || true
  [ -n "$rows" ] || return 2
  printf '%s\n' "$rows"
}

# cr_scan_bin_addr_scope: read `file:line:text` records on stdin (grep -nH
# shape; text may be a whole source line) and print every retail-binary
# address whose comment block names no build. A block is a run of adjacent
# comment lines in one file, so the scope may sit on a header line above the
# address. Optional $1 names a file of `file:line` keys; when given, only
# blocks containing one of those lines are judged (staged mode judges what a
# commit adds, but a new address under an existing scoped header passes).
# Returns 0 clean, 1 with offenders printed, 2 when the registry is unreadable.
cr_scan_bin_addr_scope() {
  local keys="${1:-}" rows
  rows=$(cr_known_client_rows) || return 2
  awk -v rows="$(printf '%s\n' "$rows" | paste -sd '|' -)" -v keys="$keys" \
      -v addrre="$CR_RE_BIN_ADDR" -v barere="$CR_RE_BIN_BARE" -v imagebase="$CR_BIN_IMAGE_BASE" \
      -v ctxre="$CR_RE_BIN_CTX" -v hexre="$CR_RE_HEX_TOKEN" '
    BEGIN {
      nrows = split(rows, rowlist, "|")
      for (k = 1; k <= nrows; k++) rowset[rowlist[k]] = 1
      havekeys = (keys != "")
      if (keys != "") { while ((getline key < keys) > 0) { added[key] = 1 }; close(keys) }
      n = 0; rc = 0
    }
    function flush(   j, t, m, nt, tok, scoped, judged, ctx, off) {
      if (n == 0) return
      scoped = 0; ctx = 0; judged = !havekeys
      for (j = 1; j <= n; j++) {
        t = texts[j]
        if (t ~ hexre) scoped = 1
        if (t ~ ctxre) ctx = 1
        nt = split(t, tok, /[^A-Za-z0-9_-]+/)
        for (m = 1; m <= nt; m++) if (tok[m] in rowset) scoped = 1
        if (!judged && ((blkfile ":" lines[j]) in added)) judged = 1
      }
      if (judged && !scoped) {
        for (j = 1; j <= n; j++) {
          t = texts[j]; off = 0
          if (t ~ addrre) off = 1
          else if (ctx) {
            nt = split(t, tok, /[^A-Za-z0-9_]+/)
            for (m = 1; m <= nt; m++) if (tok[m] ~ barere && tok[m] != imagebase) off = 1
          }
          if (off) { print blkfile ":" lines[j] ": " t; rc = 1 }
        }
      }
      n = 0
    }
    {
      if (!match($0, /^[^:]+:[0-9]+:/)) next
      hdr = substr($0, 1, RLENGTH - 1)
      text = substr($0, RLENGTH + 1)
      c = index(hdr, ":")
      file = substr(hdr, 1, c - 1); line = substr(hdr, c + 1) + 0
      gsub(/https?:\/\/[^[:space:]]*/, "", text)
      c = index(text, "//")
      if (c == 0) next
      text = substr(text, c)
      if (file != blkfile || line != lastline + 1) { flush(); blkfile = file }
      lastline = line
      n++; lines[n] = line; texts[n] = text
    }
    END { flush(); exit rc }
  '
}

# Citations nobody in this tree can open. An elided `.../` path can't be
# checked for existence, and a `(F37)`-style finding id points at a note that
# lives outside the repo; both are what a handoff written against private
# research produces. scripts/checks.sh comments hard-fails on these.
CR_RE_ELIDED_PATH='(vendor|research)/[A-Za-z0-9._-]+[[:space:]]*/?\.\.\./'
CR_RE_FINDING_ID='(finding(s)?[[:space:]]+F[0-9]{1,3}|\(F[0-9]{1,3}([,;/][[:space:]]?F[0-9]{1,3})*\)|\bF[0-9]{1,3}[-/]F[0-9]{1,3}\b|FFXiMain\.dll[,;)]?[[:space:]]+F[0-9]{1,3}\b)'

# A section reference into a session artifact ("plan §2.3", "the handoff
# section 4"). Keyed on the ANTECEDENT word, never on the section sign alone,
# so a published citation (Ericson §5.1.3, "Real-Time Collision Detection"
# §1.3.6) keeps passing while the unopenable one fails.
CR_RE_PRIVATE_PLAN='(^|[^A-Za-z0-9_])([Tt]he[[:space:]]+)?([Pp]lan|[Ww]riteup|[Ww]rite-up|[Hh]andoff|[Pp]roposal)[[:space:]]*(§|[Ss]ection[[:space:]]|[Ss]ec\.)'

# A bare ordinal label opening a comment ("Piece 3:", "Phase 2:"). It numbers
# the step of a session's work plan, which no reader of the merged tree can
# order or open. Anchored at the comment opener so an ordinal used inside a
# sentence (a zone's "step 42") still passes.
CR_RE_STEP_LABEL='//[/!]?[[:space:]]*(-[[:space:]]+)?([Pp]iece|[Pp]hase|[Ss]tep|[Ss]tage|[Pp]art)[[:space:]]+[0-9]+[[:space:]]*[:.)]'

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

# cr_allowed_block_strip: read source lines on stdin, print the comment
# lines that survive the allowed carve-out, judged per comment BLOCK instead
# of per line: a citation on one line (SAFETY, vendor/research path, bare
# decompile filename) scopes the whole contiguous run it sits in, so a block
# that cites on its header line is not re-flagged line by line. A block is a
# maximal run of adjacent lines containing `//`; a URL line keeps the block
# open but is never printed; non-comment lines break blocks.
cr_allowed_block_strip() {
  local text marked
  text=$(cat)
  [ -z "$text" ] && return 0
  # grep owns the carve-out regex (its \b word boundaries are GNU-grep
  # extensions the awk below cannot see); awk owns the block logic.
  marked=$(printf '%s\n' "$text" | grep -nE "$CR_RE_ALLOWED" | cut -d: -f1 | paste -sd, - || true)
  printf '%s\n' "$text" | awk -v marked="$marked" '
    BEGIN { n = split(marked, m, ","); for (i = 1; i <= n; i++) ismarked[m[i] + 0] = 1 }
    {
      if (index($0, "//") > 0) {
        if ($0 !~ /https?:\/\//) buf[++nb] = $0
        if (NR in ismarked) blockmarked = 1
      } else if (nb > 0) {
        if (!blockmarked) for (i = 1; i <= nb; i++) print buf[i]
        nb = 0; blockmarked = 0
      }
    }
    END { if (nb > 0 && !blockmarked) for (i = 1; i <= nb; i++) print buf[i] }
  '
}

# scan_comment_rot: read plain source text on stdin, print labeled
# findings to stdout (one per line, capped per category), and return 0
# if anything matched, 1 if clean. URLs and the allowed carve-out are
# excluded up front (per comment block — see cr_allowed_block_strip); the
# worst families get a sharp label and everything else remaining falls
# through to a generic [comment] flag.
scan_comment_rot() {
  local text comments flaggable found=1 hits matched rest

  # Every line comment, minus URLs (so prose links don't trip).
  text=$(cat)
  [ -z "$text" ] && return 1
  comments=$(printf '%s\n' "$text" | grep -E '//' | grep -vE 'https?://' || true)
  [ -z "$comments" ] && return 1

  # Line-pinned vendor citations are flagged from the FULL comment set: the
  # allow-list below is precisely what exempts them, so this must run first.
  local pinned
  pinned=$(printf '%s\n' "$comments" | grep -vE "$CR_RE_VERSIONED" | grep -oE "$CR_RE_CITE_LINE" | sort -u | head -4 || true)
  if [ -n "$pinned" ]; then
    printf '%s\n' "$pinned" | sed -E 's#^#  [citation pinned to a line number] #'
    found=0
  fi

  local dangling
  dangling=$(printf '%s\n' "$comments" | grep -E "$CR_RE_ELIDED_PATH|$CR_RE_FINDING_ID|$CR_RE_PRIVATE_PLAN|$CR_RE_STEP_LABEL" | head -4 || true)
  if [ -n "$dangling" ]; then
    printf '%s\n' "$dangling" | sed -E 's#^[[:space:]]*#  [citation nobody can open: elided path / finding id / private plan section / ordinal step label] #'
    found=0
  fi

  # Drop the allowed carve-out before flagging anything, per comment block.
  flaggable=$(printf '%s\n' "$text" | cr_allowed_block_strip || true)
  [ -z "$flaggable" ] && return $found

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
