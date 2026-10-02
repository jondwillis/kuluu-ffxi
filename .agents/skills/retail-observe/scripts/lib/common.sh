# Shared front-end logic for observe.sh: profile resolution, window selection,
# logical key lookup, coordinate conversion. Host backends (lib/macos.sh,
# lib/linux.sh) implement the same verb contract against this.

die() { printf 'observe: %s\n' "$*" >&2; exit 1; }
warn() { printf 'observe: %s\n' "$*" >&2; }

SCRIPT_DIR=${SCRIPT_DIR:?}
KEYS_TSV="$SCRIPT_DIR/lib/keys.tsv"

# --- profile ---------------------------------------------------------------
# A profile only matters for `launch`. Observing a client that is ALREADY
# running needs no configuration: the client titles its window "FINAL FANTASY
# XI" on every host, which is the default match.
PROFILE_FIELDS="host window_title window_owner install runner loader loader_args server vm_name"

profile_path() {
  local name=$1 base
  base=${XDG_CONFIG_HOME:-$HOME/.config}
  printf '%s/ffxi-observe/%s.conf\n' "$base" "$name"
}

load_profile() {
  local name=${FFXI_OBSERVE_PROFILE:-} path key val
  [ -n "$name" ] || return 0
  path=$(profile_path "$name")
  [ -f "$path" ] || die "profile '$name' has no config at $path (run \`observe.sh profile-template $name\` to write a starting point)"
  while IFS= read -r line; do
    case $line in ''|'#'*) continue;; esac
    key=${line%%=*}; val=${line#*=}
    key=$(printf '%s' "$key" | tr -d '[:space:]')
    case " $PROFILE_FIELDS " in
      *" $key "*) eval "P_$key=\$val" ;;
      *) warn "profile '$name': ignoring unknown field '$key'" ;;
    esac
  done < "$path"
  PROFILE_NAME=$name
}

# The host decides which backend to source, so it resolves before anything
# that asks the backend for a default.
resolve_host() {
  load_profile
  HOST=${FFXI_OBSERVE_HOST:-${P_host:-$(detect_host)}}
}

# Env wins over the config file so a one-off drive never needs an edit.
# The HXI_* names are the pre-rename spelling kept working for the recipes
# quoted in references/.
resolve_config() {
  WINDOW_TITLE=${FFXI_OBSERVE_WINDOW_TITLE:-${HXI_GAME_RE:-${P_window_title:-FINAL FANTASY}}}
  WINDOW_OWNER=${FFXI_OBSERVE_WINDOW_OWNER:-${HXI_OWNER_RE:-${P_window_owner:-$(default_window_owner "$HOST")}}}
  INSTALL=${FFXI_OBSERVE_INSTALL:-${P_install:-}}
  RUNNER=${FFXI_OBSERVE_RUNNER:-${P_runner:-$(default_runner "$HOST")}}
  LOADER=${FFXI_OBSERVE_LOADER:-${P_loader:-}}
  LOADER_ARGS=${FFXI_OBSERVE_LOADER_ARGS:-${P_loader_args:-}}
  SERVER=${FFXI_OBSERVE_SERVER:-${P_server:-}}
  VM_NAME=${FFXI_OBSERVE_VM_NAME:-${HXI_VM_NAME:-${P_vm_name:-}}}
  # Anchor captures to the repository rather than the caller's directory, so a
  # burst driven from the scripts directory does not scatter PNGs into the skill.
  ARTIFACTS=${FFXI_OBSERVE_ARTIFACTS:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)/artifacts/retail}
}

detect_host() {
  case $(uname -s) in
    Darwin) echo macos ;;
    Linux)  echo linux ;;
    *) die "unsupported host '$(uname -s)'. On Windows use observe.ps1 instead." ;;
  esac
}

# An install path may name a kuluu registry entry rather than a directory, so a
# drive script never hardcodes where someone keeps 30 GB of client files.
resolve_install() {
  case ${INSTALL:-} in
    '') echo '' ;;
    kuluu:*) kuluu install path "${INSTALL#kuluu:}" 2>/dev/null \
               || cargo run -q -p kuluu -- install path "${INSTALL#kuluu:}" \
               || die "cannot resolve install '${INSTALL}'" ;;
    *) printf '%s\n' "$INSTALL" ;;
  esac
}

# --- logical keys ----------------------------------------------------------
# KEY_COLUMN is set by the backend: 2 = macOS virtual keycode, 3 = X11 keysym.
lookup_key() {
  local want=$1 col=${KEY_COLUMN:?}
  awk -F'\t' -v k="$want" -v c="$col" '!/^#/ && $1 == k { print $c; found=1; exit }
    END { exit !found }' "$KEYS_TSV" && return 0
  return 1
}

# Unknown names pass through so a key the table does not cover yet is still
# reachable with a raw host code — but say so, because that is unportable.
key_code() {
  local name=$1 code
  if code=$(lookup_key "$name"); then printf '%s\n' "$code"; return 0; fi
  case ${KEY_COLUMN:?} in
    2) case $name in ''|*[!0-9]*) return 1;; *) warn "key '$name' is a raw macOS keycode, not in keys.tsv — add it there if the recipe is worth keeping"; printf '%s\n' "$name";; esac ;;
    *) warn "key '$name' is not in keys.tsv — passing it to the backend verbatim"; printf '%s\n' "$name" ;;
  esac
}

list_keys() {
  local col
  case ${KEY_COLUMN:-2} in
    2) col='macOS keycode' ;;
    3) col='X11 keysym' ;;
    *) col="column ${KEY_COLUMN:-2}" ;;
  esac
  printf '%-17s %-14s %s\n' 'logical key' "$col" 'in-game meaning'
  awk -F'\t' -v c="${KEY_COLUMN:-2}" '!/^#/ && NF>1 { printf "%-17s %-14s %s\n", $1, $c, $5 }' "$KEYS_TSV"
}

# --- window selection ------------------------------------------------------
# Prefer the window whose TITLE matches (the client itself), then one titled
# after a VM (a console window with the guest desktop inside it). There
# is deliberately no "largest window" fallback: on a host also running the
# remake, a browser and an editor, guessing would drive the wrong application,
# and a drive loop cannot tell that apart from a bad click.
select_window() {
  local all hit
  all=$(host_windows_json) || return 1
  [ -n "$all" ] && [ "$all" != "[]" ] || return 1
  hit=$(printf '%s' "$all" | jq -c --arg t "$WINDOW_TITLE" --arg vm "${VM_NAME:-}" '
    (map(select(.name | test($t; "i"))) | first) //
    (if ($vm | length) > 0 then (map(select(.name | test($vm; "i"))) | first) else null end)
    | select(. != null)')
  [ -n "$hit" ] || return 1
  printf '%s\n' "$hit"
}

# Backends override this where the host can be in a state that accepts
# captures but silently discards synthesized input. Defaulting to "fine" keeps
# a backend that cannot detect such a state from blocking every drive loop.
host_input_or_die() { :; }

need_window() {
  WIN=$(select_window) || {
    host_raise_hint
    local i
    for i in 1 2 3 4 5 6 7 8; do
      sleep 1
      WIN=$(select_window) && break
    done
  }
  [ -n "${WIN:-}" ] || die "no client window matching title /$WINDOW_TITLE/ owner /$WINDOW_OWNER/.
$(host_no_window_help)
Run \`observe.sh targets\` to see every window this host reports, and \`observe.sh doctor\` to check prerequisites."
  WID=$(jq -r .id <<<"$WIN")
  WPID=$(jq -r '.pid // empty' <<<"$WIN")
  WX=$(jq -r .x <<<"$WIN"); WY=$(jq -r .y <<<"$WIN")
  WW=$(jq -r .w <<<"$WIN"); WH=$(jq -r .h <<<"$WIN")
}

# Captures are in device pixels; clicks are in the host's logical units. Every
# coordinate a drive script measures off a screenshot has to cross this, and
# getting it wrong is the single most common way a click lands on wallpaper.
report_capture() {
  local out=$1 px
  px=$(host_image_width "$out")
  SCALE=$(awk -v p="${px:-0}" -v w="$WW" 'BEGIN{ if (w>0) printf "%.4f", p/w; else print 1 }')
  printf '%s  window:%sx%s  image:%spx-wide  scale:%sx (divide px coords by this before click)\n' \
    "$out" "$WW" "$WH" "${px:-?}" "$SCALE"
}

run_ocr() {
  local img=$1 raw
  raw=$(host_ocr "$img") || die "OCR failed on this host — see \`observe.sh doctor\`"
  awk -F'\t' -v s="${SCALE:-1}" 's>0 && NF>=3 {printf "%s\t%d\t%d\n", $1, $2/s, $3/s}' <<<"$raw"
}

# Consent/elevation prompts are the human's to answer, on every host: a UAC
# dialog in a VM, a macOS permission sheet, a polkit prompt. Refusing at the
# click layer means no drive loop can talk itself into clicking one.
CONSENT_RE='User Account Control|make changes to your device|Verified publisher|Allow .* to control|Authentication Required|polkit'
refuse_if_consent_dialog() {
  local ocr=$1 keep=$2
  if grep -qiE "$CONSENT_RE" <<<"$ocr"; then
    die "refusing to click: an elevation/consent dialog is on screen and that consent is human-only (capture kept: $keep)"
  fi
}
