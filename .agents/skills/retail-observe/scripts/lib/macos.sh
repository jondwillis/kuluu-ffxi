# macOS backend: reach the client through the window server, whatever is
# hosting it — Wine (FFXI-on-Mac, Whisky, CrossOver, plain wine) or a VM
# (Parallels, VMware, UTM). Nothing here talks to a guest agent, so the same
# code path works for a Wine window and for a VM console window.
#
# CGWindowList finds the window, `screencapture -l` takes its pixels, CGEvent
# posts input, Vision does OCR. All through stock osascript JXA and swift.

KEY_COLUMN=2

default_window_owner() { printf '.\n'; }   # the window TITLE is the real filter

default_runner() {
  if command -v wine >/dev/null 2>&1; then echo wine; else echo ''; fi
}

# --bg posts straight to the client's process instead of the global HID tap, so
# a drive loop does not have to win a focus fight with every other app. Whether
# the host forwards input to an unfocused window is per-Wine-build and
# per-VM-build, which is why it is opt-in rather than the default.
post_prelude() {
  if [ -n "${BG:-}" ]; then
    [ -n "${WPID:-}" ] || die "--bg needs the client window's pid; run without --bg or check \`observe.sh window\`"
    printf 'ObjC.bindFunction("CGEventPostToPid",["void",["unsigned int","void *"]]); function POST(e){ $.CGEventPostToPid(%s, e); }' "$WPID"
  else
    printf 'function POST(e){ $.CGEventPost($.kCGHIDEventTap, e); }'
  fi
}

# CFMakeCollectable bridges the CFArrayRef so deepUnwrap can convert it. The
# owner regex arrives as argv, never spliced into the program text: a regex
# containing a space would otherwise word-split into two osascript arguments
# and every window query would fail, which reads as "the window is missing".
host_windows_json() {
  osascript -l JavaScript -e '
    function run(argv) {
    ObjC.import("CoreGraphics");
    ObjC.bindFunction("CFMakeCollectable", ["id", ["void *"]]);
    const opts = $.kCGWindowListOptionOnScreenOnly | $.kCGWindowListExcludeDesktopElements;
    const wins = ObjC.deepUnwrap($.CFMakeCollectable(
      $.CGWindowListCopyWindowInfo(opts, $.kCGNullWindowID))) || [];
    const re = new RegExp(argv[0]);
    return JSON.stringify(wins
      .filter(w => re.test(w.kCGWindowOwnerName || "") && (w.kCGWindowLayer|0) === 0)
      .map(w => ({ id: w.kCGWindowNumber, name: w.kCGWindowName || "",
                   owner: w.kCGWindowOwnerName, pid: w.kCGWindowOwnerPID,
                   x: w.kCGWindowBounds.X, y: w.kCGWindowBounds.Y,
                   w: w.kCGWindowBounds.Width, h: w.kCGWindowBounds.Height }))
      .filter(w => w.w > 200 && w.h > 200)
      .filter(w => !/Control Center/i.test(w.name)));
    }' "$WINDOW_OWNER" 2>/dev/null
}

host_image_width() { sips -g pixelWidth "$1" 2>/dev/null | awk '/pixelWidth/{print $2}'; }

# Focus and the active Space snap back to the terminal the moment a shell
# invocation ends, so raising has to happen in the SAME invocation as the act
# that needs it. Every verb that needs focus calls need_window, which calls
# this — never assume a window stayed raised across two calls.
host_raise_hint() {
  local p
  if [ -n "${VM_NAME:-}" ]; then
    for p in "Parallels Desktop" "prl_client_app" "prl_vm_app" "VMware Fusion" "UTM"; do
      osascript -e 'tell application "System Events" to tell process "'"$p"'"
        set frontmost to true
        click menu item "'"$VM_NAME"'" of menu "Window" of menu bar 1
      end tell' >/dev/null 2>&1 && return 0
    done
    for p in "prl_client_app" "prl_vm_app"; do
      osascript -e 'tell application "System Events" to tell process "'"$p"'"
        set frontmost to true
        if (count of windows) > 0 then perform action "AXRaise" of window "'"$VM_NAME"'"
      end tell' >/dev/null 2>&1 && return 0
    done
  fi
  return 1
}

host_no_window_help() {
  cat <<'EOF'
On macOS the client window is either a Wine window (its owner is the Wine
process or the wrapper app) or a VM window. If nothing matches:
  - the client is not running       -> `observe.sh launch`, or start it by hand
  - it is running but titled differently (a launcher, a config tool)
                                    -> FFXI_OBSERVE_WINDOW_TITLE='<regex>'
  - it is a VM console, not the game -> set vm_name / FFXI_OBSERVE_VM_NAME
  - the terminal lacks Screen Recording -> `observe.sh doctor`
EOF
}

# Focus cascade, cheapest first. The window's owning pid is not always an app
# System Events can address (a guest exe under a VM, a Wine child process), so
# fall back to the owner NAME, then to whatever the host offers for raising a
# VM console. A Parallels guest app stub wins the focus race where plain
# `set frontmost` loses it to any app that wants focus back — a running remake
# client is the usual thief.
host_show() {
  local owner front stub
  owner=$(jq -r '.owner // empty' <<<"${WIN:-}" 2>/dev/null)
  stub=$(ls -d ~/"Applications (Parallels)"/*Applications.localized/"$owner.app" 2>/dev/null | head -1)
  if [ -n "$stub" ]; then
    open "$stub" 2>/dev/null
  else
    osascript -e 'tell application "System Events" to set frontmost of (first process whose unix id is '"${WPID:-0}"') to true' >/dev/null 2>&1 \
      || osascript -e 'tell application "System Events" to set frontmost of (first process whose name is "'"$owner"'") to true' >/dev/null 2>&1 \
      || host_raise_hint
  fi
  sleep 0.5
  front=$(osascript -e 'tell application "System Events" to get name of first process whose frontmost is true' 2>/dev/null)
  printf 'frontmost: %s\n' "${front:-?}"
  # Parallels routes guest input through a shim, so frontmost reading as
  # WinAppHelper is success, not failure.
  case ${front:-} in
    WinAppHelper|prl_vm_app) printf 'observe: that shim holds focus for the guest; input still lands\n' ;;
  esac
}

host_capture() {
  local wid=$1 out=$2
  caffeinate -u -t 2   # an asleep display captures black
  screencapture -x -o -l "$wid" "$out" || die "screencapture failed — grant Screen Recording to this terminal (\`observe.sh doctor\`)"
  [ -s "$out" ] || die "empty capture — grant Screen Recording to this terminal"
}

host_key() {
  local code=$1 dur=$2
  osascript -l JavaScript -e "
    ObjC.import('CoreGraphics');
    $(post_prelude)
    POST(\$.CGEventCreateKeyboardEvent(\$(), $code, true));
    delay($dur);
    POST(\$.CGEventCreateKeyboardEvent(\$(), $code, false));" \
    || die "key post failed — grant Accessibility to this terminal (\`observe.sh doctor\`)"
}

host_type() {
  osascript -e 'on run argv
    tell application "System Events" to keystroke (item 1 of argv)
  end run' "$1" || die "keystroke failed — grant Accessibility to this terminal"
}

host_click() {
  local gx=$1 gy=$2 kind=$3 press=$4
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
    if ('$press' === 'click') {
      delay(0.05);
      const dn = ('$kind'==='right') ? \$.kCGEventRightMouseDown : \$.kCGEventLeftMouseDown;
      const up = ('$kind'==='right') ? \$.kCGEventRightMouseUp   : \$.kCGEventLeftMouseUp;
      const btn = ('$kind'==='right') ? \$.kCGMouseButtonRight   : \$.kCGMouseButtonLeft;
      post(dn, btn); delay(0.05); post(up, btn);
      if ('$kind'==='double') { delay(0.08); post(dn, btn); delay(0.05); post(up, btn); }
    }" || die "mouse post failed — grant Accessibility to this terminal"
}

host_drag() {
  local x1=$1 y1=$2 x2=$3 y2=$4 kind=$5 steps=$6
  osascript -l JavaScript -e "
    ObjC.import('CoreGraphics');
    $(post_prelude)
    const right = '$kind' === 'right';
    const btn  = right ? \$.kCGMouseButtonRight : \$.kCGMouseButtonLeft;
    const down = right ? \$.kCGEventRightMouseDown : \$.kCGEventLeftMouseDown;
    const up   = right ? \$.kCGEventRightMouseUp   : \$.kCGEventLeftMouseUp;
    const drag = right ? \$.kCGEventRightMouseDragged : \$.kCGEventLeftMouseDragged;
    function at(t, x, y) { POST(\$.CGEventCreateMouseEvent(\$(), t, {x: x, y: y}, btn)); }
    const ox = $WX, oy = $WY, n = $steps;
    at(\$.kCGEventMouseMoved, ox + $x1, oy + $y1); delay(0.05);
    at(down, ox + $x1, oy + $y1); delay(0.08);
    for (let i = 1; i <= n; i++) {
      at(drag, ox + $x1 + ($x2 - $x1) * i / n, oy + $y1 + ($y2 - $y1) * i / n);
      delay(0.02);
    }
    delay(0.08);
    at(up, ox + $x2, oy + $y2);" || die "drag post failed — grant Accessibility to this terminal"
}

host_ocr() { swift "$SCRIPT_DIR/lib/ocr-macos.swift" "$1"; }

host_launch() {
  local install loader args
  install=$(resolve_install)
  if [ -n "${VM_NAME:-}" ]; then
    command -v prlctl >/dev/null 2>&1 && prlctl start "$VM_NAME" 2>&1 | tail -1
    printf 'observe: VM %s asked to start. Open the client inside the guest (or its host app stub) and re-run `observe.sh status`.\n' "$VM_NAME"
    return 0
  fi
  [ -n "$install" ] || die "launch needs an install: set FFXI_OBSERVE_INSTALL=/path or kuluu:NAME"
  [ -n "${LOADER:-}" ] || die "launch needs a loader: set FFXI_OBSERVE_LOADER (e.g. _bootloader/xiloader.exe)"
  loader="$install/$LOADER"
  [ -f "$loader" ] || die "loader not found: $loader"
  args="$LOADER_ARGS"
  [ -n "${SERVER:-}" ] && args="$args --server $SERVER"
  printf 'observe: %s %s %s\n' "${RUNNER:-}" "$loader" "$args"
  # cd into the install: loaders resolve DATs, Ashita config and their own
  # DLLs relative to the working directory, not to argv[0].
  ( cd "$(dirname "$loader")" && exec ${RUNNER:-} "$loader" $args )
}

host_doctor() {
  local ok=0
  printf 'host: macOS %s\n' "$(sw_vers -productVersion 2>/dev/null)"
  for t in jq osascript screencapture sips swift; do
    if command -v "$t" >/dev/null 2>&1; then printf '  [ok]   %s\n' "$t"
    else printf '  [FAIL] %s missing\n' "$t"; ok=1; fi
  done
  local perms
  perms=$(osascript -l JavaScript -e '
    ObjC.import("CoreGraphics"); ObjC.import("ApplicationServices");
    ObjC.bindFunction("CGPreflightScreenCaptureAccess", ["bool", []]);
    ObjC.bindFunction("AXIsProcessTrusted", ["bool", []]);
    `${$.CGPreflightScreenCaptureAccess()} ${$.AXIsProcessTrusted()}`' 2>/dev/null)
  case $perms in
    "true true") printf '  [ok]   Screen Recording + Accessibility granted to this terminal\n' ;;
    "true false") printf '  [FAIL] Accessibility NOT granted -> keys/clicks are silently dropped (System Settings > Privacy & Security > Accessibility)\n'; ok=1 ;;
    "false true") printf '  [FAIL] Screen Recording NOT granted -> captures come back black (System Settings > Privacy & Security > Screen Recording)\n'; ok=1 ;;
    *) printf '  [warn] could not read permission state (%s)\n' "${perms:-no output}" ;;
  esac
  if [ -n "${VM_NAME:-}" ]; then
    command -v prlctl >/dev/null 2>&1 && printf '  [ok]   prlctl present; VM %s: %s\n' "$VM_NAME" "$(prlctl list "$VM_NAME" 2>&1 | tail -1)" \
      || printf '  [warn] no prlctl; start the VM from its GUI\n'
  else
    if command -v wine >/dev/null 2>&1; then
      printf '  [ok]   wine %s\n' "$(wine --version 2>/dev/null)"
      printf '         a Wine build must be able to open a macOS window at all: if a GUI\n'
      printf '         app under it produces no window here, the Wine build is the problem,\n'
      printf '         not this script (see references/host-macos.md)\n'
    else
      printf '  [warn] no wine on PATH; a wrapper app (FFXI-on-Mac, Whisky, CrossOver) brings its own\n'
    fi
  fi
  return $ok
}
