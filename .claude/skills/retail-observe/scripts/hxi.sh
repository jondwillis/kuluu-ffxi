#!/usr/bin/env bash
# hxi.sh — drive/observe the retail FFXI client (HorizonXI) running in the
# Parallels "Windows 11" VM, entirely from the macOS host.
#
# Why host-side only: this machine runs Parallels STANDARD — `prlctl exec`
# and `prlctl capture` are Pro/Business features. So we go through the
# macOS window server instead: CGWindowList to find the VM window,
# `screencapture -l` for pixels, CGEvent posts for keys/mouse. All via
# stock osascript JXA — no cliclick, no guest-side agent.
#
#   hxi.sh status                  VM run state + window visibility
#   hxi.sh window                  JSON of VM windows {id,name,x,y,w,h}
#   hxi.sh show                    focus + raise the VM window (input needs this)
#   hxi.sh capture [out.png]       screenshot the VM window (window-cropped)
#   hxi.sh [--bg] key <name|code> [secs]   press (or hold) a key: w a s d enter esc ...
#   hxi.sh type <text>             type a string (login fields, chat)
#   hxi.sh [--bg] click <x> <y> [right|double]   click at WINDOW-RELATIVE POINTS
#   hxi.sh [--bg] move <x> <y>     move mouse (hover) at window-relative points
#
# --bg (or HXI_BG=1): deliver key/click/move to the VM process via
#   CGEventPostToPid instead of the global focused tap — drives the guest
#   WITHOUT stealing host focus, sidestepping the focus race with other apps.
#   `type` still needs focus. Experimental: confirm your Parallels build
#   forwards guest input while unfocused (the retail-observe live test).
#
# Coordinates: `click`/`move` take points relative to the VM window's
# top-left, in macOS POINTS. Screenshots are Retina PIXELS — divide pixel
# coords by the scale factor `capture` prints before clicking on something
# you located in a screenshot.
#
# Permissions (one-time, host): the terminal running this needs
# Screen Recording (for capture) and Accessibility (for CGEvent input) in
# System Settings → Privacy & Security. Failures show as black/empty
# captures and silently-dropped input respectively.

set -uo pipefail

VM_NAME="${HXI_VM_NAME:-Windows 11}"
# Coherence-mode windows are owned by the guest exe (e.g. horizon-loader.exe),
# not prl_vm_app — [.]exe$ catches those.
OWNER_RE="${HXI_OWNER_RE:-prl_vm_app|Parallels|[.]exe\$}"
GAME_RE="${HXI_GAME_RE:-FINAL FANTASY}"

die() { printf 'hxi: %s\n' "$*" >&2; exit 1; }

vm_pid() { pgrep -x prl_vm_app | head -1; }

# post_prelude: emit the JS defining POST(event). Default routes through the
# global HID tap (input goes to the frontmost window — needs `show` first).
# Under --bg/HXI_BG it targets the VM process via CGEventPostToPid, so input
# reaches the guest WITHOUT stealing host focus. (Whether a given Parallels
# build forwards guest input while unfocused is what the live test confirms.)
post_prelude() {
  if [ -n "${BG:-}" ]; then
    local pid; pid=$(vm_pid)
    [ -n "$pid" ] || die "--bg: prl_vm_app is not running"
    printf 'ObjC.bindFunction("CGEventPostToPid",["void",["unsigned int","void *"]]); function POST(e){ $.CGEventPostToPid(%s, e); }' "$pid"
  else
    printf 'function POST(e){ $.CGEventPost($.kCGHIDEventTap, e); }'
  fi
}

# --- window discovery (CGWindowList via JXA; CFMakeCollectable bridges the
# CFArrayRef so deepUnwrap can convert it) --------------------------------
windows_json() {
  osascript -l JavaScript -e '
    ObjC.import("CoreGraphics");
    ObjC.bindFunction("CFMakeCollectable", ["id", ["void *"]]);
    const opts = $.kCGWindowListOptionOnScreenOnly | $.kCGWindowListExcludeDesktopElements;
    const wins = ObjC.deepUnwrap($.CFMakeCollectable(
      $.CGWindowListCopyWindowInfo(opts, $.kCGNullWindowID))) || [];
    const re = new RegExp('"'"$OWNER_RE"'"');   // shell splices OWNER_RE in *with* quotes
    const out = wins
      .filter(w => re.test(w.kCGWindowOwnerName || "") && (w.kCGWindowLayer|0) === 0)
      .map(w => ({ id: w.kCGWindowNumber, name: w.kCGWindowName || "",
                   owner: w.kCGWindowOwnerName,
                   x: w.kCGWindowBounds.X, y: w.kCGWindowBounds.Y,
                   w: w.kCGWindowBounds.Width, h: w.kCGWindowBounds.Height }))
      .filter(w => w.w > 200 && w.h > 200)    // skip toolbars/thumbnails
      .filter(w => !/Control Center/i.test(w.name));  // Parallels CC is not a VM
    JSON.stringify(out);' 2>/dev/null
}

# vm_window: prefer the game window itself (Coherence mode titles it after
# the game, with trailing NULs), then the VM console window, then largest.
vm_window() {
  local all
  all=$(windows_json)
  [ -n "$all" ] && [ "$all" != "[]" ] || return 1
  printf '%s' "$all" | jq -c --arg vm "$VM_NAME" --arg game "$GAME_RE" '
    (map(select(.name | test($game; "i"))) | first) //
    (map(select(.name | test($vm; "i"))) | first) //
    (sort_by(-(.w * .h)) | first)'
}

need_window() {
  WIN=$(vm_window) || die "no visible VM window (owner ~ /$OWNER_RE/). VM state:
$(prlctl list "$VM_NAME" 2>&1)
If running: the window is closed/minimized or on another Space — run \`hxi.sh show\`, or reopen it from Parallels Desktop (Window → $VM_NAME)."
  WID=$(jq -r .id   <<<"$WIN")
  WX=$(jq  -r .x    <<<"$WIN"); WY=$(jq -r .y <<<"$WIN")
  WW=$(jq  -r .w    <<<"$WIN"); WH=$(jq -r .h <<<"$WIN")
}

keycode() {  # macOS virtual keycodes (US layout); numeric passes through
  case "$1" in
    a) echo 0;; s) echo 1;; d) echo 2;; f) echo 3;; h) echo 4;; g) echo 5;;
    z) echo 6;; x) echo 7;; c) echo 8;; v) echo 9;; b) echo 11;; q) echo 12;;
    w) echo 13;; e) echo 14;; r) echo 15;; y) echo 16;; t) echo 17;;
    1) echo 18;; 2) echo 19;; 3) echo 20;; 4) echo 21;; 6) echo 22;; 5) echo 23;;
    9) echo 25;; 7) echo 26;; 8) echo 28;; 0) echo 29;;
    o) echo 31;; u) echo 32;; i) echo 34;; p) echo 35;;
    enter|return) echo 36;; l) echo 37;; j) echo 38;; k) echo 40;;
    n) echo 45;; m) echo 46;; tab) echo 48;; space) echo 49;;
    backspace) echo 51;; esc|escape) echo 53;;
    f1) echo 122;; f2) echo 120;; f3) echo 99;; f4) echo 118;; f5) echo 96;;
    f6) echo 97;; f7) echo 98;; f8) echo 100;; f9) echo 101;; f10) echo 109;;
    f11) echo 103;; f12) echo 111;;
    left) echo 123;; right) echo 124;; down) echo 125;; up) echo 126;;
    ''|*[!0-9]*) return 1;;
    *) echo "$1";;
  esac
}

BG="${HXI_BG:-}"
[ "${1:-}" = "--bg" ] && { BG=1; shift; }
cmd=${1:-}; shift || true
case "$cmd" in

  status)
    prlctl list "$VM_NAME" 2>&1 || true
    win=$(vm_window || true)
    if [ -n "${win:-}" ]; then printf 'window: %s\n' "$win"
    else printf 'window: none visible on this Space (closed/minimized/other Space)\n'; fi
    ;;

  window)
    all=$(windows_json) || die "CGWindowList query failed (JXA error — rerun windows_json without 2>/dev/null to see why)"
    printf '%s\n' "${all:-[]}" | jq .
    ;;

  show)
    # Coherence windows report the guest exe (e.g. horizon-loader.exe) as
    # CGWindow owner, but that's cosmetic — the real process is prl_vm_app,
    # which exposes NO AX windows in Coherence. `set frontmost` on it loses
    # the focus race against other host apps (a running remake client will
    # snatch focus straight back). Opening the guest's Parallels app STUB
    # reliably wins and holds focus; frontmost then reads as WinAppHelper
    # (the Parallels input shim). Accept that or prl_vm_app as success.
    owner=$(vm_window 2>/dev/null | jq -r '.owner // empty' 2>/dev/null)
    case ${owner:-} in
      *.exe)
        stub=$(ls -d ~/"Applications (Parallels)"/*Applications.localized/"$owner.app" 2>/dev/null | head -1)
        if [ -n "$stub" ]; then open "$stub" 2>/dev/null
        else osascript -e 'tell application "System Events" to set frontmost of process "prl_vm_app" to true' >/dev/null 2>&1; fi
        sleep 0.8
        front=$(osascript -e 'tell application "System Events" to get name of first process whose frontmost is true' 2>/dev/null)
        case ${front:-} in
          WinAppHelper|prl_vm_app) : ;;
          *) die "could not focus guest app '$owner' (frontmost=${front:-?}). Another host app is grabbing focus — a running remake client (\`ffxi-client play\`) is the usual culprit; quit or minimize it, then retry." ;;
        esac
        ;;
      *)
        osascript -e '
          tell application "System Events" to tell process "prl_vm_app"
            set frontmost to true
            if (count of windows) > 0 then perform action "AXRaise" of window 1
          end tell' >/dev/null 2>&1 \
          || open -a "Parallels Desktop"   # last resort: bring the app up
        ;;
    esac
    sleep 0.5
    vm_window | jq . 2>/dev/null || echo "window still not visible — open it in Parallels Desktop"
    ;;

  capture)
    out=${1:-"artifacts/retail/$(date +%Y%m%d-%H%M%S).png"}
    need_window
    mkdir -p "$(dirname "$out")"
    screencapture -x -o -l "$WID" "$out" || die "screencapture failed (Screen Recording permission?)"
    [ -s "$out" ] || die "empty capture — grant Screen Recording to this terminal"
    px=$(sips -g pixelWidth "$out" 2>/dev/null | awk '/pixelWidth/{print $2}')
    scale=$(awk -v p="${px:-0}" -v w="$WW" 'BEGIN{ if (w>0) printf "%.2f", p/w; else print "?" }')
    printf '%s  window:%sx%spt  image:%spx-wide  scale:%sx (divide px coords by this for click)\n' \
      "$out" "$WW" "$WH" "${px:-?}" "$scale"
    ;;

  key)
    name=${1:?usage: hxi.sh [--bg] key <name|code> [hold-seconds]}; dur=${2:-0.05}
    code=$(keycode "$name") || die "unknown key '$name' (use a macOS virtual keycode number)"
    osascript -l JavaScript -e "
      ObjC.import('CoreGraphics');
      $(post_prelude)
      POST(\$.CGEventCreateKeyboardEvent(\$(), $code, true));
      delay($dur);
      POST(\$.CGEventCreateKeyboardEvent(\$(), $code, false));" \
      || die "key post failed (Accessibility permission?)"
    ;;

  type)
    text=${1:?usage: hxi.sh type <text>}
    [ -n "${BG:-}" ] && printf 'hxi: note: `type` uses focused input even under --bg — bring the game frontmost first (`show`).\n' >&2
    osascript -e 'on run argv
      tell application "System Events" to keystroke (item 1 of argv)
    end run' "$text" || die "keystroke failed (Accessibility permission?)"
    ;;

  click|move)
    x=${1:?usage: hxi.sh [--bg] $cmd <x> <y> [right|double]}; y=${2:?}; kind=${3:-left}
    need_window
    gx=$(awk -v a="$WX" -v b="$x" 'BEGIN{print a+b}')
    gy=$(awk -v a="$WY" -v b="$y" 'BEGIN{print a+b}')
    osascript -l JavaScript -e "
      ObjC.import('CoreGraphics');
      $(post_prelude)
      const p = { x: $gx, y: $gy };
      function post(t, btn) {
        const e = \$.CGEventCreateMouseEvent(\$(), t, p, btn);
        if ('$kind' === 'double') \$.CGEventSetIntegerValueField(e, \$.kCGMouseEventClickState, 2);
        POST(e);
      }
      post(\$.kCGEventMouseMoved, \$.kCGMouseButtonLeft);
      if ('$cmd' === 'click') {
        delay(0.05);
        const dn = ('$kind'==='right') ? \$.kCGEventRightMouseDown : \$.kCGEventLeftMouseDown;
        const up = ('$kind'==='right') ? \$.kCGEventRightMouseUp   : \$.kCGEventLeftMouseUp;
        const btn = ('$kind'==='right') ? \$.kCGMouseButtonRight   : \$.kCGMouseButtonLeft;
        post(dn, btn); delay(0.05); post(up, btn);
        if ('$kind'==='double') { delay(0.08); post(dn, btn); delay(0.05); post(up, btn); }
      }" || die "mouse post failed (Accessibility permission?)"
    ;;

  *)
    sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
    ;;
esac
