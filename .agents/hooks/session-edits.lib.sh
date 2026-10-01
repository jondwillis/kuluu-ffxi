#!/usr/bin/env bash
# Shared accessors for the per-session edit ledger. Source this; do not
# execute it.
#
# The ledger answers "which paths did THIS session write?" — the question
# a SessionStart porcelain snapshot cannot answer, because "dirty now but
# not dirty at session start" also captures every concurrent writer in a
# shared checkout. Stop checks intersect their dirty set against it so a
# neighbouring session's edits can never be attributed here.
#
# Ledger path: $TMPDIR/claude-session-edits/<session_id>.paths
# One repo-relative path per line, append-only, deduped on read.
#
# Bash attribution is inference, not a payload field, so it needs two more
# pieces the Edit/Write recorder does not:
#
#   <session_id>.suspect   a path whose content changed inside a Bash
#                          command's window that the command neither names
#                          nor has a writer form for. Almost always a peer
#                          session's concurrent write, so it is logged for a
#                          human to read rather than credited to the ledger.
#
#   ledger_forget          the escape hatch when attribution is wrong anyway:
#                          drops a path from ONE session's ledger and leaves
#                          the tree red for whoever really owns it. Reachable
#                          by hand through session-edits-forget.sh, which the
#                          commit nudge names.
#
# Content signatures are git blob hashes, not mtime+size: BSD stat reports
# whole seconds, so a same-second in-place edit of equal length (sed -i on
# one word) is invisible to mtime+size.

# shasum is absent on some git-bash installs (Windows); sha256sum is always
# there. Both print "<hash>  -" for stdin, so the key derivation below is
# identical either way.
if command -v shasum >/dev/null 2>&1; then
  SESSION_EDITS_DIGEST='shasum -a 256'
else
  SESSION_EDITS_DIGEST='sha256sum'
fi

# Above this many paths the pre-command signature snapshot is skipped, and
# attribution falls back to the porcelain delta of paths the command names.
SESSION_EDITS_MAX_SIG_PATHS_DEFAULT=200
SESSION_EDITS_MAX_SIG_PATHS="${SESSION_EDITS_MAX_SIG_PATHS:-$SESSION_EDITS_MAX_SIG_PATHS_DEFAULT}"
# Sentinel for "no file on disk", distinct from any blob hash.
SESSION_EDITS_ABSENT_SIG="absent"
# A snapshot this old lost its PostToolUse call, so it describes some other
# command's window and attributing from it would credit whatever ran since.
SESSION_EDITS_SNAP_TTL_DEFAULT=3600
SESSION_EDITS_SNAP_TTL="${SESSION_EDITS_SNAP_TTL:-$SESSION_EDITS_SNAP_TTL_DEFAULT}"
# A non-numeric override reaches arithmetic and `[ -gt ]`, both of which would
# write to stderr, and a hook may never speak. Every numeric knob is guarded.
case "$SESSION_EDITS_SNAP_TTL" in
  ''|*[!0-9]*) SESSION_EDITS_SNAP_TTL=$SESSION_EDITS_SNAP_TTL_DEFAULT ;;
esac
# A session's ledger and suspect log outlive its last tool call by no more
# than this. Every hook touch refreshes the mtime, so the TTL only ever reaps
# a session that has stopped calling tools; it is session scale, not the
# command-window scale of the snapshot TTL, and a live multi-hour session can
# never reach it.
SESSION_EDITS_LEDGER_TTL_DEFAULT=604800
SESSION_EDITS_LEDGER_TTL="${SESSION_EDITS_LEDGER_TTL:-$SESSION_EDITS_LEDGER_TTL_DEFAULT}"
case "$SESSION_EDITS_LEDGER_TTL" in
  ''|*[!0-9]*) SESSION_EDITS_LEDGER_TTL=$SESSION_EDITS_LEDGER_TTL_DEFAULT ;;
esac
case "$SESSION_EDITS_MAX_SIG_PATHS" in
  ''|*[!0-9]*) SESSION_EDITS_MAX_SIG_PATHS=$SESSION_EDITS_MAX_SIG_PATHS_DEFAULT ;;
esac
SESSION_EDITS_SNAP_KEY_CHARS=16

# Why a path was withheld from the ledger. The suspect log records this and the
# path, never the command text: a command line can carry a credential.
SESSION_EDITS_SUSPECT_UNNAMED="unnamed-by-a-non-writer-command"
SESSION_EDITS_SUSPECT_CAPPED="unnamed-and-too-many-paths-to-sign"

# Where a program name can begin: start of text, a command separator, a new
# line, or right after a launcher that runs its argument (xargs, find -exec,
# sudo), optionally past leading env assignments. A bare space is deliberately
# NOT a boundary: with one, `kuluu install list` reads as install(1) and
# `git stash list` as a stash write, and both are commands this repo documents
# as read-only, so every peer write in their window would be credited here.
SESSION_EDITS_CMD_START_RE=$'((^|[;&|(\n])[[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+)*((xargs|sudo|time|command|env|then|do|else)[[:space:]]+)*|[[:space:]]-exec(dir)?[[:space:]]+)'

# Programs whose invocation is by itself evidence the command writes files.
# Redirects and tee are NOT here: they are matched through cmd_write_targets
# so that `ls > /dev/null` does not become a licence to credit a peer's write.
# A form that takes an operand must be shown one: `rm` and `git checkout` with
# nothing after them wrote nothing, and matching them anyway hands the whole
# command window to the ledger.
SESSION_EDITS_WRITER_OPERAND="[[:space:]]+(-[^[:space:]]+[[:space:]]+)*[^-[:space:]]"
SESSION_EDITS_WRITER_RE="${SESSION_EDITS_CMD_START_RE}((sed|perl)[[:space:]]+(-[^[:space:]]+[[:space:]]+)*-[a-zA-Z]*i|(mv|cp|rm|touch|install|patch|rmcm|rustfmt)${SESSION_EDITS_WRITER_OPERAND}|cargo[[:space:]]+(fmt|fix)([[:space:]]|\$)|cargo[[:space:]]+clippy[[:space:]].*--fix|git[[:space:]]+(apply|checkout|restore|mv|rm)${SESSION_EDITS_WRITER_OPERAND}|git[[:space:]]+stash([[:space:]]+(push|save|pop|apply|drop|clear)([[:space:]]|\$)|[[:space:]]*\$))"

# The cargo fmt/fix arm of the writer form, on its own: these shapes carry no
# file operand, so the writer bit must not credit every dirty path in their
# window (a peer write during a long cargo fmt --all is not this session's).
# Spelled as a separate pattern because the writer form's alternation cannot
# be tested for one arm in isolation.
SESSION_EDITS_CARGO_FMT_RE='cargo[[:space:]]+(fmt|fix)([[:space:]]|\$)'

# Tools that write files they never name on the command line, paired with the
# directory each one owns. Crediting is scoped to that directory, so the
# window of a tool nobody can predict the outputs of still withholds a peer's
# concurrent write elsewhere.
SESSION_EDITS_OWNED_WRITES='bd:.beads/'

# Programs that only read the paths they name: while one of these runs, a
# peer's concurrent write to a named path is a suspect, not a ledger line.
# The list is an allowlist - a program missing from it keeps the naming arm
# alive, because an unknown tool may write from its code (python, perl, awk,
# find -delete), and miscrediting one peer write is cheaper than silencing
# every real edit an unusual tool makes.
SESSION_EDITS_READONLY_PROGRAMS='basename cat comm cmp cut date df du dirname echo egrep file fgrep grep head less ls md5sum more nproc od printf pwd readlink realpath sha256sum shasum stat strings tail tr uname wc which'
# git writes through most of its subcommands, so it is judged by the first
# bare argument after the global options (-C, -c, --git-dir, --work-tree).
# `stash` additionally needs its own sub-argument: bare `git stash` pushes.
SESSION_EDITS_GIT_READONLY_SUBCMDS='blame cat-file describe diff grep hash-object log ls-files ls-tree rev-parse shortlog show status var help'

ledger_dir() { printf '%s/claude-session-edits' "${TMPDIR:-/tmp}"; }

ledger_path() {
  [ -n "${1:-}" ] || return 1
  printf '%s/%s.paths' "$(ledger_dir)" "$1"
}

suspect_path() {
  [ -n "${1:-}" ] || return 1
  printf '%s/%s.suspect' "$(ledger_dir)" "$1"
}

# The pre/post snapshot pair, keyed by session and command hash so parallel
# Bash calls in one turn do not clobber each other's baseline. Both hooks
# derive the name from here: one byte of drift and post misses its own
# snapshot, attributing nothing while still looking healthy.
snap_dir() { printf '%s/bashpre' "$(ledger_dir)"; }

snap_key() {
  printf '%s' "${1:-}" | $SESSION_EDITS_DIGEST | cut -c"1-$SESSION_EDITS_SNAP_KEY_CHARS"
}

# snap_path <session_id> <cmd>
snap_path() {
  [ -n "${1:-}" ] || return 1
  printf '%s/%s.%s' "$(snap_dir)" "$1" "$(snap_key "${2:-}")"
}

# file_mtime <path>: epoch seconds, 0 when unknown. BSD stat has no -c and
# GNU stat has no -f format, so a single spelling reports nothing on the
# other platform — and a snapshot that reports no mtime reads as infinitely
# stale, silently disabling Bash attribution there.
file_mtime() {
  local m
  m=$(stat -f %m "${1:-}" 2>/dev/null) || m=""
  case "$m" in ''|*[!0-9]*) m=$(stat -c %Y "${1:-}" 2>/dev/null) || m="" ;; esac
  case "$m" in ''|*[!0-9]*) m=0 ;; esac
  printf '%s' "$m"
}

# snap_sweep: a denied or interrupted Bash call runs pre with no post, so its
# pair is stranded until the OS reclaims the temp dir. find's minute
# granularity is rounded UP, so the sweep can only ever run later than the TTL
# asks, never early enough to reap a live command's own snapshot.
snap_sweep() {
  local dir
  dir=$(snap_dir)
  [ -d "$dir" ] || return 0
  find "$dir" -type f -mmin "+$(((SESSION_EDITS_SNAP_TTL + 59) / 60))" -delete 2>/dev/null
  return 0
}

# ledger_sweep: reap ledger and suspect logs whose owner has made no tool call
# for the ledger TTL. A stale <sid>.suspect must not keep counting into the
# commit nudge if the session id is reused. find's minute granularity is
# rounded UP, as in snap_sweep, so the sweep can only ever run later than the
# TTL asks.
ledger_sweep() {
  local dir
  dir=$(ledger_dir)
  [ -d "$dir" ] || return 0
  find "$dir" -maxdepth 1 -type f \( -name '*.paths' -o -name '*.suspect' \) \
    -mmin "+$(((SESSION_EDITS_LEDGER_TTL + 59) / 60))" -delete 2>/dev/null
  return 0
}

# repo_root <dir>: the worktree root. Every ledger line, signature and
# porcelain line is relative to it, never to the session's cwd, which may be a
# subdirectory: `git hash-object --stdin-paths` resolves its input against the
# root whatever -C it was handed, and `git status --porcelain` prints root-
# relative paths, so a cwd-relative ledger would agree with neither.
repo_root() {
  local d="${1:-.}" root
  root=$(git -C "$d" rev-parse --show-toplevel 2>/dev/null) || root=""
  [ -n "$root" ] || root="$d"
  printf '%s' "$root"
}

# subdir_prefix <root> <cwd>: the cwd's path below the root with a trailing
# slash, empty at the root itself. Command text names files relative to the
# cwd, ledger lines are relative to the root; this is the difference.
subdir_prefix() {
  case "${2:-}" in
    "$1") printf '' ;;
    "$1"/*) printf '%s/' "${2#"$1"/}" ;;
    *) printf '' ;;
  esac
}

# porcelain_paths: `git status --porcelain` lines on stdin, paths out. The
# status columns and any rename arrow are dropped. Four call sites compare
# their own output against each other, so the transform is single-sourced
# here: a change to git's porcelain quoting must land in exactly one place.
porcelain_paths() {
  sed -E 's/^.{3}//; s/^"(.*)"$/\1/; s/.* -> //'
}

# _rel <cwd> <path>: repo-relative form, the shape every ledger line takes.
_rel() {
  local rel="$2"
  case "$rel" in "$1"/*) rel="${rel#"$1"/}" ;; esac
  printf '%s' "$rel"
}

# ledger_add <session_id> <cwd> <abs-or-relative-path>...
ledger_add() {
  local sid="$1" cwd="$2" file
  shift 2
  [ -n "$sid" ] || return 0
  local out
  out=$(ledger_path "$sid") || return 0
  mkdir -p "$(ledger_dir)" || return 0
  for file in "$@"; do
    [ -n "$file" ] || continue
    printf '%s\n' "$(_rel "$cwd" "$file")" >> "$out"
  done
}

# ledger_touch <session_id>: refresh the mtime of this session's ledger and
# suspect log on every hook call, so the sweep's TTL measures time since the
# session last called a tool, not since it last wrote a file: a live session
# in a long read-only phase must not lose its ledger.
ledger_touch() {
  local f
  [ -n "${1:-}" ] || return 0
  for f in "$(ledger_path "$1")" "$(suspect_path "$1")"; do
    [ -f "$f" ] && touch "$f" 2>/dev/null
  done
  return 0
}

# ledger_read <session_id>: sorted unique paths, empty when absent.
ledger_read() {
  local f
  f=$(ledger_path "${1:-}") || return 0
  [ -f "$f" ] && sort -u "$f" || true
}

# ledger_exists <session_id>
ledger_exists() {
  local f
  f=$(ledger_path "${1:-}") || return 1
  [ -f "$f" ]
}

# ledger_forget <session_id> <cwd> <path>...: drop paths from one session's
# ledger. Whole-line fixed-string match, so forgetting `mod.rs` never scrubs
# `hud/mod.rs`.
ledger_forget() {
  local sid="$1" cwd="$2" file f tmp
  shift 2
  [ -n "$sid" ] || return 0
  f=$(ledger_path "$sid") || return 0
  [ -f "$f" ] || return 0
  local -a args=()
  for file in "$@"; do
    [ -n "$file" ] || continue
    args+=(-e "$(_rel "$cwd" "$file")")
  done
  [ "${#args[@]}" -gt 0 ] || return 0
  tmp="${f}.forget.$$"
  grep -vxF "${args[@]}" "$f" > "$tmp" 2>/dev/null
  mv -f "$tmp" "$f" 2>/dev/null || rm -f "$tmp"
}

# suspect_add <session_id> <root> <path> <reason>
suspect_add() {
  local sid="$1" root="$2" file="$3" reason="${4:-}" out
  [ -n "$sid" ] || return 0
  out=$(suspect_path "$sid") || return 0
  mkdir -p "$(ledger_dir)" || return 0
  printf '%s\t%s\n' "$(_rel "$root" "$file")" "$reason" >> "$out"
}

# sig_snapshot <root> <outfile>: read root-relative paths on stdin, write
# "<signature>\t<path>" lines. One hash-object process for the whole set, not
# one per path.
sig_snapshot() {
  local root="$1" out="$2" p want hashes n
  want=$(mktemp "${TMPDIR:-/tmp}/sesig.XXXXXX") || return 0
  hashes=$(mktemp "${TMPDIR:-/tmp}/sesig.XXXXXX") || { rm -f "$want"; return 0; }
  local all
  all=$(mktemp "${TMPDIR:-/tmp}/sesig.XXXXXX") || { rm -f "$want" "$hashes"; return 0; }
  grep -v '^$' | sort -u > "$all"
  n=$(grep -c . "$all" || true)
  if [ "${n:-0}" -gt "$SESSION_EDITS_MAX_SIG_PATHS" ]; then
    rm -f "$want" "$hashes" "$all"
    return 0
  fi
  : > "$out"
  while IFS= read -r p; do
    if [ -f "$root/$p" ]; then
      printf '%s\n' "$p" >> "$want"
    else
      printf '%s\t%s\n' "$SESSION_EDITS_ABSENT_SIG" "$p" >> "$out"
    fi
  done < "$all"
  if [ -s "$want" ]; then
    git -C "$root" hash-object --stdin-paths < "$want" 2>/dev/null \
      | grep -v '^$' > "$hashes" || true
    # hash-object aborts on a path that vanished mid-run and emits only a
    # prefix, so pair by index over the lines it actually produced. Zero is
    # reachable (the first path went away) and must stay silent: BSD head
    # rejects `-n 0` outright, which would both speak and leave the whole
    # command unsigned, turning the content gate into a no-op.
    local emitted
    emitted=$(grep -c . "$hashes" || true)
    if [ "${emitted:-0}" -gt 0 ]; then
      paste "$hashes" <(sed -n "1,${emitted}p" "$want") >> "$out"
    fi
  fi
  rm -f "$want" "$hashes" "$all"
}

# cmd_path_tokens <cmd> <cwd> <root>: whitespace tokens of the command that
# name an existing regular file inside the repo, root-relative. Tokens are
# resolved against the cwd because that is what the command's own shell did.
cmd_path_tokens() {
  local cmd="$1" cwd="$2" root="${3:-$2}" t abs globbing=1
  case $- in *f*) globbing=0 ;; esac
  set -f
  # shellcheck disable=SC2086
  for t in $cmd; do
    t="${t%\'}"; t="${t#\'}"
    t="${t%\"}"; t="${t#\"}"
    t="${t%;}"; t="${t%,}"
    case "$t" in ''|-*) continue ;; esac
    case "$t" in
      /*) abs="$t" ;;
      *) abs="$cwd/${t#./}" ;;
    esac
    [ -f "$abs" ] || continue
    case "$abs" in "$root"/*) ;; *) continue ;; esac
    printf '%s\n' "${abs#"$root"/}"
  done
  [ "$globbing" = 1 ] && set +f
  return 0
}

# _is_path_char <char>: this character can belong to a path token, so it is not
# a boundary. Empty (start or end of the command) is a boundary.
_is_path_char() {
  case "${1:-}" in
    [A-Za-z0-9_.~/-]) return 0 ;;
  esac
  return 1
}

# cmd_mentions <cmd> <needle>: the needle appears in the command as a whole
# path token. A bare substring test credits `mod.rs` to a command that named
# `hud/mod.rs`, and `hud/mod.rs` to one that named `mod.rs`.
cmd_mentions() {
  local cmd="$1" n="${2:-}" rest pre post head
  [ -n "$n" ] || return 1
  rest="$cmd"
  while :; do
    case "$rest" in *"$n"*) ;; *) return 1 ;; esac
    pre="${rest%%"$n"*}"
    post="${rest#*"$n"}"
    if ! _is_path_char "${post:0:1}"; then
      head="$pre"
      case "$head" in *./) head="${head%./}" ;; esac
      _is_path_char "${head: -1}" || return 0
    fi
    rest="$post"
  done
}

# git_subcommand <cmd>: the first bare argument after git's global options,
# or empty when the command runs no git at a command boundary. The walk is
# boundary-aware, so a `git` that is only an argument of another program
# (echo git status) never starts it, and -C/-c and friends consume their
# value so `git -C repo stash list` reads as stash.
git_subcommand() {
  local cmd="$1" t seen=0 skip=0 sub=""
  set -f
  for t in $cmd; do
    if [ "$skip" = 1 ]; then skip=0; continue; fi
    case "$t" in
      ';'*) seen=0; skip=0; case "$t" in *git) [ "${t#*;}" = "git" ] && seen=1 ;; esac; continue ;;
    esac
    if [ "$seen" = 1 ]; then
      case "$t" in
        -C|-c|--git-dir|--work-tree|--namespace|--super-prefix) skip=1; continue ;;
        -*) continue ;;
        *) sub="$t"; break ;;
      esac
    fi
    case "$t" in
      git) seen=1 ;;
    esac
  done
  set +f
  printf '%s' "$sub"
}

# git_stash_subarg <cmd>: the first bare argument after `stash` in the first
# git invocation, empty when the command has no `git stash ...`. Bare `git
# stash` pushes, so only list/show keep the command read-only.
git_stash_subarg() {
  local cmd="$1" t seen=0 skip=0 in_stash=0 arg=""
  set -f
  for t in $cmd; do
    if [ "$skip" = 1 ]; then skip=0; continue; fi
    case "$t" in
      ';'*) seen=0; skip=0; in_stash=0; case "$t" in *git) [ "${t#*;}" = "git" ] && seen=1 ;; esac; continue ;;
    esac
    if [ "$in_stash" = 1 ]; then
      case "$t" in
        -*) continue ;;
        *) arg="$t"; break ;;
      esac
    fi
    if [ "$seen" = 1 ]; then
      case "$t" in
        -C|-c|--git-dir|--work-tree|--namespace|--super-prefix) skip=1; continue ;;
        -*) continue ;;
        stash) in_stash=1 ;;
      esac
    fi
    case "$t" in
      git) seen=1 ;;
    esac
  done
  set +f
  printf '%s' "$arg"
}

# in_word_list <word> <list>: the word equals one of the list's words. A case
# pattern would read the unquoted list as a single space-joined pattern, so
# membership is a word loop over the list's words.
in_word_list() {
  local w="$1" e
  shift
  for e in "$@"; do
    [ "$e" = "$w" ] && return 0
  done
  return 1
}

# cmd_readonly <cmd>: every program the command runs is read-only, so a path
# it names is being read, not written. git counts by subcommand; a program
# missing from the lists fails the check and the naming arm stays alive. The
# walk is bash =~, not grep: the boundary class carries a literal newline,
# which grep's ERE reads as backslash-n and the extraction would mis-parse.
cmd_readonly() {
  local cmd="$1" rest="$1" pat pat_mid prog sub m invocation first=1 matched=0
  [ -n "$cmd" ] || return 1
  pat="${SESSION_EDITS_CMD_START_RE}([A-Za-z_/][A-Za-z0-9_/+.-]*)"
  # The ^ alternative is only a boundary at the true start of the text; after
  # the first match it would read a consumed command's first argument as a new
  # program (git diff -> diff), so later passes use the boundary-class form.
  pat_mid="((${SESSION_EDITS_CMD_START_RE:4}([A-Za-z_/][A-Za-z0-9_/+.-]*)"
  while :; do
    if [ "$first" = 1 ]; then
      [[ "$rest" =~ $pat ]] || break
      first=0
    else
      [[ "$rest" =~ $pat_mid ]] || break
    fi
    matched=1
    m="${BASH_REMATCH[0]}"
    invocation="${rest#*"$m"}"
    prog="${BASH_REMATCH[7]##*/}"
    rest="${rest/"$m"/ }"
    case "$prog" in
      git)
        sub=$(git_subcommand "git $invocation")
        case "$sub" in
          '') return 1 ;;
          stash) [ "$(git_stash_subarg "git $invocation")" = list ] || [ "$(git_stash_subarg "git $invocation")" = show ] || return 1 ;;
          *) in_word_list "$sub" $SESSION_EDITS_GIT_READONLY_SUBCMDS || return 1 ;;
        esac
        ;;
      *) in_word_list "$prog" $SESSION_EDITS_READONLY_PROGRAMS || return 1 ;;
    esac
  done
  [ "$matched" = 1 ]
}

# cmd_names_path <cmd> <root> <subdir-prefix> <root-relative-path>: the command
# text names this path as a whole token. Three spellings are tried, since a
# session working in a subdirectory writes `a.txt` for the ledger's
# `sub/a.txt`, and an absolute path has the root in front of it. A read-only
# command names its paths to read them, so the naming arm is withheld for it
# and a peer's concurrent write to a named path lands in the suspect log.
cmd_names_path() {
  local cmd="$1" root="$2" prefix="${3:-}" p="${4:-}" rel target
  [ -n "$p" ] || return 1
  while IFS= read -r target; do
    cmd_mentions "$target" "$p" && return 0
    [ -n "$root" ] && cmd_mentions "$target" "$root/$p" && return 0
    if [ -n "$prefix" ]; then
      case "$p" in "$prefix"*)
        rel="${p#"$prefix"}"
        [ -n "$rel" ] && cmd_mentions "$target" "$rel" && return 0
        ;;
      esac
    fi
  done < <(cmd_write_targets "$cmd")
  cmd_readonly "$cmd" && return 1
  cmd_mentions "$cmd" "$p" && return 0
  [ -n "$root" ] && cmd_mentions "$cmd" "$root/$p" && return 0
  [ -n "$prefix" ] || return 1
  case "$p" in "$prefix"*) rel="${p#"$prefix"}" ;; *) return 1 ;; esac
  [ -n "$rel" ] || return 1
  cmd_mentions "$cmd" "$rel"
}

# cmd_owns_path <cmd> <root-relative-path>: a known tool is running and this
# path is inside the directory it owns. The arm that keeps a session's own
# `bd` writes to .beads/ - the export that has to cross into git - out of the
# suspect log, since bd names no file on its command line.
cmd_owns_path() {
  local cmd="$1" p="${2:-}" entry tool dir
  [ -n "$p" ] || return 1
  for entry in $SESSION_EDITS_OWNED_WRITES; do
    tool="${entry%%:*}"
    dir="${entry#*:}"
    case "$p" in "$dir"*) ;; *) continue ;; esac
    [[ "$cmd" =~ ${SESSION_EDITS_CMD_START_RE}${tool}([[:space:]]|$) ]] && return 0
  done
  return 1
}

# cmd_write_targets <cmd>: the operands of every output redirect and tee.
cmd_write_targets() {
  printf '%s\n' "$1" | awk '
    {
      for (i = 1; i <= NF; i++) {
        t = $i
        if (t ~ /^[0-9&]*>>?$/) { if (i < NF) print $(i + 1); continue }
        if (t ~ /^[0-9&]*>>?./) { sub(/^[0-9&]*>>?/, "", t); print t; continue }
        if (t == "tee") { j = i + 1; while (j <= NF && $j ~ /^-/) j++; if (j <= NF) print $j }
      }
    }'
}

# cmd_is_writer_form <cmd>: the command shape alone says it writes files.
cmd_is_writer_form() {
  local cmd="$1" t
  # Bash's own =~, not a grep pipeline: under pipefail a grep -q that exits on
  # first match can SIGPIPE its feeder and turn a match into a non-zero status.
  [[ "$cmd" =~ $SESSION_EDITS_WRITER_RE ]] && return 0
  while IFS= read -r t; do
    t="${t%\'}"; t="${t#\'}"
    t="${t%\"}"; t="${t#\"}"
    [ -n "$t" ] || continue
    # Scratch targets, including the unexpanded forms — the hook sees the
    # command text, so `> "$TMPDIR/hits"` never arrives already expanded.
    case "$t" in
      '&'*|/dev/*|/tmp/*|/var/folders/*|'$TMPDIR'*|'${TMPDIR'*|"${TMPDIR:-/tmp}"*) continue ;;
    esac
    return 0
  done < <(cmd_write_targets "$cmd")
  return 1
}

# cmd_writer_plausible <cmd> <cwd> <root> <root-relative-path>: for a command
# whose shape is a writer form, is THIS path inside the tool's plausible file
# set? The blanket licence is withheld for the shapes that carry no file
# operand: cargo fmt/fix can only touch tracked *.rs under the workspace, and
# rmcm only the operands it was pointed at (a directory operand reaches into
# its tree). Every other writer form names its operands, so the naming arm and
# the blanket backstop stay as they are.
cmd_writer_plausible() {
  local cmd="$1" cwd="$2" root="$3" p="${4:-}"
  [ -n "$p" ] || return 1
  if [[ "$cmd" =~ $SESSION_EDITS_CARGO_FMT_RE ]]; then
    case "$p" in
      *.rs) git -C "$root" ls-files --error-unmatch -- "$p" >/dev/null 2>&1 ;;
      *) return 1 ;;
    esac
    return $?
  fi
  if [[ "$cmd" =~ ${SESSION_EDITS_CMD_START_RE}rmcm([[:space:]]|$) ]]; then
    local t abs prefix root_phys
    # The payload's root may be in drive-letter form (C:/...) while `pwd -P`
    # reports MSYS physical form (/...); normalize root into the same form as
    # `abs` so the prefix match below cannot miss on the form alone.
    root_phys=$(cd "$root" && pwd -P) || return 1
    set -f
    # shellcheck disable=SC2086
    for t in $cmd; do
      t="${t%\'}"; t="${t#\'}"
      t="${t%\"}"; t="${t#\"}"
      t="${t%;}"; t="${t%,}"
      case "$t" in ''|-*) continue ;; esac
      case "$t" in
        /*) abs="$t" ;;
        *) abs="$cwd/${t#./}" ;;
      esac
      [ -e "$abs" ] || continue
      if [ -d "$abs" ]; then
        abs=$(cd "$abs" && pwd -P) || continue
      else
        abs="$(cd "$(dirname "$abs")" && pwd -P)/$(basename "$abs")"
      fi
      case "$abs" in "$root_phys"|"$root_phys"/*) ;; *) continue ;; esac
      if [ -d "$abs" ]; then
        [ "$abs" = "$root_phys" ] && return 0
        prefix="${abs#"$root_phys"/}"
        case "$p" in "$prefix"/*) return 0 ;; esac
      elif [ "$abs" = "$root_phys/$p" ]; then
        return 0
      fi
    done
    set +f
    return 1
  fi
  [[ "$cmd" =~ $SESSION_EDITS_WRITER_RE ]]
}
