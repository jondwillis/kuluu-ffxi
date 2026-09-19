#!/usr/bin/env bash
# observe.sh — drive and observe a retail FFXI client from the host, whatever
# is running it (Wine on macOS or Linux, Proton on Linux, a Windows VM) and
# whatever server it talks to (a private server, or a local LandSandBoat stack).
# On native Windows use observe.ps1, which implements the same verbs.
#
#   observe.sh doctor              check this host's prerequisites and permissions
#   observe.sh targets             every window this host reports (find yours here)
#   observe.sh status              which window resolves as the client, and where
#   observe.sh window              JSON of the matched window {id,name,owner,pid,x,y,w,h}
#   observe.sh show                raise/focus it (input needs this)
#   observe.sh capture [out.png]   screenshot the client window, cropped to it
#   observe.sh ocr                 capture + OCR: TEXT<TAB>x<TAB>y in window units
#   observe.sh keys                the logical key names and what they do in game
#   observe.sh [--bg] key <name|code> [secs]      press or hold a key
#   observe.sh type <text>                        type a string
#   observe.sh [--bg] click <x> <y> [right|double]        WINDOW-RELATIVE units
#   observe.sh [--bg] move <x> <y>                        hover
#   observe.sh [--bg] drag <x1> <y1> <x2> <y2> [left|right] [steps]
#   observe.sh [--bg] click-text <regex> [right|double]   OCR, then click the
#       matched text's centre. Self-verifying, and it refuses outright while an
#       elevation/consent dialog is on screen: that consent is a human's.
#   observe.sh launch [--server HOST]             start the client from a profile
#   observe.sh profile-template <name>            write a starting config
#
# Configuration: observing a client that is already running needs none — the
# client titles its window "FINAL FANTASY XI" on every host. Everything is
# overridable per invocation:
#   FFXI_OBSERVE_PROFILE       named config in ~/.config/ffxi-observe/<name>.conf
#   FFXI_OBSERVE_HOST          macos | linux (default: this machine)
#   FFXI_OBSERVE_WINDOW_TITLE  window-title regex (default: FINAL FANTASY)
#   FFXI_OBSERVE_WINDOW_OWNER  owning-process regex (default: any)
#   FFXI_OBSERVE_VM_NAME       match a VM console window, for a client in a VM
#   FFXI_OBSERVE_INSTALL       client directory, or kuluu:NAME to ask the registry
#   FFXI_OBSERVE_RUNNER        wine | umu-run | '' for native | any command
#   FFXI_OBSERVE_LOADER        loader exe relative to the install
#   FFXI_OBSERVE_LOADER_ARGS   extra loader arguments
#   FFXI_OBSERVE_SERVER        server address to launch against
#   FFXI_OBSERVE_ARTIFACTS     capture directory (default: artifacts/retail)
#   --bg / FFXI_OBSERVE_BG=1   post input to the client process instead of the
#                              focused window, so other apps keep their focus
#
# Coordinates: click/move/drag take units relative to the client window's
# top-left. Captures are in device pixels, which on a HiDPI display is a larger
# number: `capture` prints the scale factor, and any coordinate read off a
# screenshot must be divided by it first.

set -uo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$SCRIPT_DIR/lib/common.sh"

usage() { sed -n '2,50p' "$0" | sed 's/^# \{0,1\}//'; }

BG="${FFXI_OBSERVE_BG:-${HXI_BG:-}}"
[ "${1:-}" = "--bg" ] && { BG=1; shift; }
cmd=${1:-}; shift || true
[ -n "$cmd" ] || { usage; exit 1; }

resolve_host
case $HOST in
  macos|darwin) . "$SCRIPT_DIR/lib/macos.sh" ;;
  linux|x11)    . "$SCRIPT_DIR/lib/linux.sh" ;;
  windows)      die "host is windows: run observe.ps1 in PowerShell instead" ;;
  *)            die "unknown host '$HOST' (expected macos, linux or windows)" ;;
esac
resolve_config

command -v jq >/dev/null 2>&1 || die "jq is required (\`observe.sh doctor\`)"

# Focus returns to whatever the host prefers the moment a shell invocation
# ends, so a verb that delivers focused input has to raise the client in the
# SAME invocation. Skipping this does not merely fail: the keystrokes land in
# whatever window is frontmost, which is usually the terminal driving the run.
ensure_focus() {
  [ -n "${BG:-}" ] && return 0
  host_show >/dev/null 2>&1
}

capture_to() {
  local out=${1:-"$ARTIFACTS/$(date +%Y%m%d-%H%M%S).png"}
  mkdir -p "$(dirname "$out")"
  host_capture "$WID" "$out"
  printf '%s\n' "$out"
}

case $cmd in

  doctor)
    printf 'profile: %s\n' "${PROFILE_NAME:-none (zero-config: matching window title only)}"
    printf 'window match: title /%s/  owner /%s/%s\n' "$WINDOW_TITLE" "$WINDOW_OWNER" \
      "$([ -n "${VM_NAME:-}" ] && printf '  vm /%s/' "$VM_NAME")"
    host_doctor; rc=$?
    if win=$(select_window 2>/dev/null); then
      printf '  [ok]   client window found: %s\n' "$win"
    else
      printf '  [warn] no client window matches right now (that is fine if it is not running)\n'
    fi
    exit $rc
    ;;

  targets)
    # Discovery aid: everything the host reports, so you can see what to match
    # instead of guessing a title regex.
    host_windows_json | jq -r '.[] | "\(.id)\t\(.owner)\t\(.w)x\(.h)\t\(.name)"'
    ;;

  window)
    need_window
    printf '%s\n' "$WIN" | jq .
    ;;

  status)
    printf 'host: %s   profile: %s\n' "$HOST" "${PROFILE_NAME:-none}"
    if win=$(select_window 2>/dev/null); then printf 'client window: %s\n' "$win"
    else printf 'client window: none matching title /%s/\n%s\n' "$WINDOW_TITLE" "$(host_no_window_help)"; fi
    ;;

  show)
    need_window
    host_show
    printf '%s\n' "$WIN" | jq -c .
    ;;

  capture)
    need_window
    out=$(capture_to "${1:-}")
    report_capture "$out"
    ;;

  keys)
    list_keys
    ;;

  ocr|click-text)
    if [ "$cmd" = click-text ]; then
      pat=${1:?usage: observe.sh [--bg] click-text <regex> [right|double]}; kind=${2:-left}
    fi
    need_window
    tmp="${TMPDIR:-/tmp}/observe-ocr-$$.png"
    host_capture "$WID" "$tmp"
    report_capture "$tmp" >/dev/null
    ocr=$(run_ocr "$tmp")
    if [ "$cmd" = ocr ]; then rm -f "$tmp"; printf '%s\n' "$ocr"; exit 0; fi
    refuse_if_consent_dialog "$ocr" "$tmp"
    hit=$(grep -iE "$pat" <<<"$ocr" | head -1) \
      || die "no OCR text matching /$pat/ (capture kept: $tmp; run \`observe.sh ocr\` to see what is on screen)"
    rm -f "$tmp"
    tx=$(cut -f2 <<<"$hit"); ty=$(cut -f3 <<<"$hit")
    printf 'click-text: "%s" at %s,%s (window units)\n' "$(cut -f1 <<<"$hit")" "$tx" "$ty"
    exec "$0" ${BG:+--bg} click "$tx" "$ty" "$kind"
    ;;

  key)
    name=${1:?usage: observe.sh [--bg] key <name|code> [hold-seconds]}; dur=${2:-0.05}
    code=$(key_code "$name") || die "unknown key '$name' — \`observe.sh keys\` lists the logical names"
    # A hold shorter than the client's input poll is invisible to it, so a
    # movement key needs a duration, not a tap.
    need_window
    ensure_focus
    host_key "$code" "$dur"
    ;;

  type)
    text=${1:?usage: observe.sh type <text>}
    [ -n "${BG:-}" ] && warn "type uses focused input even under --bg; raising the client anyway"
    need_window
    host_show >/dev/null 2>&1
    host_type "$text"
    ;;

  click|move)
    x=${1:?usage: observe.sh [--bg] $cmd <x> <y> [right|double]}; y=${2:?}; kind=${3:-left}
    need_window
    ensure_focus
    gx=$(awk -v a="$WX" -v b="$x" 'BEGIN{print a+b}')
    gy=$(awk -v a="$WY" -v b="$y" 'BEGIN{print a+b}')
    host_click "$gx" "$gy" "$kind" "$cmd"
    ;;

  drag)
    x1=${1:?usage: observe.sh [--bg] drag <x1> <y1> <x2> <y2> [left|right] [steps]}
    y1=${2:?}; x2=${3:?}; y2=${4:?}; kind=${5:-right}; steps=${6:-20}
    need_window
    ensure_focus
    host_drag "$x1" "$y1" "$x2" "$y2" "$kind" "$steps"
    ;;

  launch)
    [ "${1:-}" = "--server" ] && { SERVER=${2:?--server needs an address}; shift 2; }
    host_launch
    ;;

  profile-template)
    name=${1:?usage: observe.sh profile-template <name>}
    path=$(profile_path "$name")
    [ -e "$path" ] && die "$path already exists"
    mkdir -p "$(dirname "$path")"
    cat > "$path" <<EOF
# ffxi-observe profile '$name'. Only \`launch\` needs these; observing a
# running client works with no profile at all.
host=$HOST
# window_title=FINAL FANTASY
# vm_name=                      # set only when the client runs inside a VM
install=
# install=kuluu:NAME            # ask the kuluu install registry for the path
runner=$(default_runner "$HOST")
loader=_bootloader/xiloader.exe
# loader_args=
# server=127.0.0.1              # a local LandSandBoat stack, or a private server
EOF
    printf 'wrote %s\n' "$path"
    ;;

  help|-h|--help) usage ;;
  *) usage; exit 1 ;;
esac
