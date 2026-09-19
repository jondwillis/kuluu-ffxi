# Linux backend: X11 (or Xwayland) via xdotool for input, maim/import for
# pixels, tesseract for OCR. Wine and Proton both present the client as an
# ordinary X window, so one path covers both.
#
# Verification status: written against documented xdotool/maim/tesseract
# behavior, not yet exercised against a running client. `observe.sh doctor`
# checks every prerequisite it relies on; fix what it reports rather than
# assuming the script is wrong.

KEY_COLUMN=3

default_window_owner() { printf '.\n'; }

default_runner() {
  if command -v umu-run >/dev/null 2>&1; then echo umu-run
  elif command -v wine >/dev/null 2>&1; then echo wine
  else echo ''; fi
}

x11_or_die() {
  [ -n "${DISPLAY:-}" ] || die "no DISPLAY. Under a pure Wayland session there is no per-window capture or input target; run the client under Xwayland or gamescope, then re-run with DISPLAY set (see references/host-linux.md)."
}

host_windows_json() {
  x11_or_die
  local id name pid owner
  {
    for id in $(xdotool search --onlyvisible --name '.' 2>/dev/null); do
      name=$(xdotool getwindowname "$id" 2>/dev/null) || continue
      pid=$(xdotool getwindowpid "$id" 2>/dev/null || echo 0)
      owner=$(cat "/proc/$pid/comm" 2>/dev/null || echo unknown)
      grep -qE "$WINDOW_OWNER" <<<"$owner" || continue
      unset X Y WIDTH HEIGHT
      eval "$(xdotool getwindowgeometry --shell "$id" 2>/dev/null)" || continue
      [ "${WIDTH:-0}" -gt 200 ] && [ "${HEIGHT:-0}" -gt 200 ] || continue
      jq -cn --argjson id "$id" --arg name "$name" --arg owner "$owner" \
        --argjson pid "${pid:-0}" --argjson x "${X:-0}" --argjson y "${Y:-0}" \
        --argjson w "$WIDTH" --argjson h "$HEIGHT" \
        '{id:$id,name:$name,owner:$owner,pid:$pid,x:$x,y:$y,w:$w,h:$h}'
    done
  } | jq -sc .
}

host_image_width() {
  if command -v identify >/dev/null 2>&1; then identify -format '%w' "$1" 2>/dev/null
  else python3 -c 'import struct,sys; f=open(sys.argv[1],"rb").read(33); print(struct.unpack(">I", f[16:20])[0])' "$1" 2>/dev/null
  fi
}

host_raise_hint() { return 1; }

host_no_window_help() {
  cat <<'EOF'
On Linux the client is an X window owned by wine/Proton. If nothing matches:
  - the client is not running        -> `observe.sh launch`, or start it by hand
  - a launcher window, not the game  -> FFXI_OBSERVE_WINDOW_TITLE='<regex>'
  - a pure Wayland session           -> no per-window target exists; run the
    client under Xwayland or gamescope (references/host-linux.md)
  - xdotool/maim/tesseract missing    -> `observe.sh doctor`
EOF
}

host_show() {
  x11_or_die
  xdotool windowactivate --sync "$WID" 2>/dev/null || xdotool windowraise "$WID"
  sleep 0.3
  printf 'active window: %s\n' "$(xdotool getactivewindow 2>/dev/null) (wanted $WID)"
}

host_capture() {
  local wid=$1 out=$2
  x11_or_die
  if command -v maim >/dev/null 2>&1; then maim -i "$wid" "$out"
  elif command -v import >/dev/null 2>&1; then import -window "$wid" "$out"
  else die "no window capture tool: install maim (preferred) or ImageMagick"
  fi
  [ -s "$out" ] || die "empty capture from window $wid"
}

# xdotool has two delivery modes and the difference decides whether a game
# reacts at all: XTEST (real input to the focused window) or XSendEvent
# (--window, synthetic). Wine passes synthetic events to DirectInput apps
# inconsistently, so the default activates the window and uses XTEST; --bg
# takes the synthetic path for the cases where stealing focus is worse.
host_key() {
  local code=$1 dur=$2
  x11_or_die
  if [ -n "${BG:-}" ]; then
    xdotool keydown --window "$WID" "$code" && sleep "$dur" && xdotool keyup --window "$WID" "$code"
    warn "--bg sends synthetic key events; a DirectInput client often ignores them. If nothing happened, retry without --bg."
  else
    xdotool windowactivate --sync "$WID" 2>/dev/null
    xdotool keydown "$code" && sleep "$dur" && xdotool keyup "$code"
  fi || die "xdotool key failed"
}

host_type() {
  x11_or_die
  xdotool windowactivate --sync "$WID" 2>/dev/null
  xdotool type --delay 40 -- "$1" || die "xdotool type failed"
}

host_click() {
  local gx=$1 gy=$2 kind=$3 press=$4 btn=1
  x11_or_die
  case $kind in right) btn=3 ;; esac
  xdotool windowactivate --sync "$WID" 2>/dev/null
  xdotool mousemove --sync "$gx" "$gy" || die "xdotool mousemove failed"
  [ "$press" = click ] || return 0
  xdotool click "$btn"
  [ "$kind" = double ] && { sleep 0.08; xdotool click "$btn"; }
  return 0
}

host_drag() {
  local x1=$1 y1=$2 x2=$3 y2=$4 kind=$5 steps=$6 btn=3 i
  x11_or_die
  case $kind in left) btn=1 ;; esac
  xdotool windowactivate --sync "$WID" 2>/dev/null
  xdotool mousemove --sync $((WX + x1)) $((WY + y1)) mousedown "$btn"
  for i in $(seq 1 "$steps"); do
    xdotool mousemove --sync $((WX + x1 + (x2 - x1) * i / steps)) $((WY + y1 + (y2 - y1) * i / steps))
    sleep 0.02
  done
  xdotool mouseup "$btn"
}

# tesseract's TSV is word-level; join words back into lines so a click-text
# regex can match a phrase ("Select Character") the way it reads on screen.
host_ocr() {
  command -v tesseract >/dev/null 2>&1 || die "tesseract not installed (needed for ocr/click-text)"
  tesseract "$1" stdout tsv 2>/dev/null | awk -F'\t' '
    NR == 1 { next }
    $1 == 5 && $12 != "" {
      key = $2 "-" $3 "-" $4 "-" $5
      if (!(key in text)) { l[key] = $7; t[key] = $8; r[key] = $7 + $9; b[key] = $8 + $10; text[key] = $12 }
      else {
        text[key] = text[key] " " $12
        if ($7 < l[key]) l[key] = $7
        if ($8 < t[key]) t[key] = $8
        if ($7 + $9 > r[key]) r[key] = $7 + $9
        if ($8 + $10 > b[key]) b[key] = $8 + $10
      }
    }
    END { for (k in text) printf "%s\t%d\t%d\n", text[k], (l[k] + r[k]) / 2, (t[k] + b[k]) / 2 }'
}

host_launch() {
  local install loader args
  install=$(resolve_install)
  [ -n "$install" ] || die "launch needs an install: set FFXI_OBSERVE_INSTALL=/path or kuluu:NAME"
  [ -n "${LOADER:-}" ] || die "launch needs a loader: set FFXI_OBSERVE_LOADER (e.g. _bootloader/xiloader.exe)"
  loader="$install/$LOADER"
  [ -f "$loader" ] || die "loader not found: $loader"
  args="$LOADER_ARGS"
  [ -n "${SERVER:-}" ] && args="$args --server $SERVER"
  printf 'observe: %s %s %s\n' "${RUNNER:-}" "$loader" "$args"
  ( cd "$(dirname "$loader")" && exec ${RUNNER:-} "$loader" $args )
}

host_doctor() {
  local ok=0 t
  printf 'host: %s\n' "$(uname -sr)"
  if [ -n "${DISPLAY:-}" ]; then printf '  [ok]   DISPLAY=%s\n' "$DISPLAY"
  elif [ -n "${WAYLAND_DISPLAY:-}" ]; then
    printf '  [FAIL] Wayland session with no DISPLAY: no per-window capture or input target.\n'
    printf '         Run the client under Xwayland or gamescope (references/host-linux.md).\n'; ok=1
  else printf '  [FAIL] neither DISPLAY nor WAYLAND_DISPLAY is set\n'; ok=1; fi
  for t in jq xdotool; do
    if command -v "$t" >/dev/null 2>&1; then printf '  [ok]   %s\n' "$t"
    else printf '  [FAIL] %s missing (input and window discovery need it)\n' "$t"; ok=1; fi
  done
  if command -v maim >/dev/null 2>&1 || command -v import >/dev/null 2>&1; then
    printf '  [ok]   window capture (%s)\n' "$(command -v maim || command -v import)"
  else printf '  [FAIL] no maim and no ImageMagick import -> capture impossible\n'; ok=1; fi
  if command -v tesseract >/dev/null 2>&1; then printf '  [ok]   tesseract\n'
  else printf '  [warn] no tesseract -> ocr/click-text unavailable; blind clicks only\n'; fi
  if command -v umu-run >/dev/null 2>&1; then printf '  [ok]   umu-run (Proton without Steam bookkeeping)\n'
  elif command -v wine >/dev/null 2>&1; then printf '  [ok]   wine %s\n' "$(wine --version 2>/dev/null)"
  else printf '  [warn] neither umu-run nor wine on PATH; launch will need a runner\n'; fi
  return $ok
}
