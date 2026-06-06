#!/usr/bin/env bash
# Shared comment-rot heuristics for the PreToolUse inline nudge
# (comment-rot-reminder.sh) and the Stop self-review checkpoint
# (comment-review-stop.sh). Source this; do not execute it.
#
# These are deliberately HIGH-PRECISION regexes over comment lines —
# they cannot judge intent, only surface candidates. The agent makes
# the keep/delete call. Categories mirror the five rot families found
# in the codebase audit (narrative, de-sync, decoration, dead code,
# unenforced invariant). Tune the patterns here, in one place, so the
# two hooks never drift apart.

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

# scan_comment_rot: read plain source text on stdin, print labeled
# findings to stdout (one per line, capped per category), and return 0
# if anything matched, 1 if clean. URLs and non-comment lines are
# excluded up front so prose in code never trips the comment filter.
scan_comment_rot() {
  local comments found=1 hits
  comments=$(grep -E '//' | grep -vE 'https?://' || true)
  [ -z "$comments" ] && return 1

  _cr_emit() { # $1=label  $2=regex
    hits=$(printf '%s\n' "$comments" | grep -iE "$2" | head -4 || true)
    [ -z "$hits" ] && return 1
    printf '%s\n' "$hits" | sed -E "s#^[[:space:]]*#  [$1] #"
    return 0
  }

  _cr_emit 'narrative/history'    "$CR_RE_NARRATIVE" && found=0
  _cr_emit 'code/offset restated' "$CR_RE_DESYNC"    && found=0
  _cr_emit 'decoration'           "$CR_RE_DECORATION" && found=0
  _cr_emit 'commented-out code'   "$CR_RE_DEADCODE"   && found=0
  _cr_emit 'unenforced invariant' "$CR_RE_INVARIANT"  && found=0
  return $found
}
