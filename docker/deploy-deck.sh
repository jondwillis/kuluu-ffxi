#!/usr/bin/env bash
# Deploy the Mac-built ffxi-client to a Steam Deck over SSH — and optionally
# build first, strip the binary, push the FFXI install, and launch it on the
# Deck's screen.
#
# A Steam Deck does NOT present as USB storage or USB networking by default,
# so deployment goes over SSH/IP (Wi-Fi, or a USB-C dock's wired ethernet).
# One-time Deck setup (Desktop Mode → Konsole):
#     passwd                               # set a password for the `deck` user
#     sudo systemctl enable --now sshd     # start the SSH server
#     ip addr | grep 'inet '               # find the Deck's IP
# Then from the Mac, install your key for password-less pushes:
#     ssh-copy-id deck@<deck-ip>
# Recommended: add an alias to ~/.ssh/config so DECK_HOST=deck just works:
#     Host deck
#         HostName <deck-ip>
#         User deck
#
# Usage:
#     DECK_HOST=deck ./docker/deploy-deck.sh [--build] [--strip] [--assets] [--run ARGS...]
#
#   --build        run docker/build-linux.sh first (otherwise reuse dist/ffxi-client)
#   --strip        push a stripped copy (~40 MB vs ~159 MB); dist/ stays unstripped
#   --assets       rsync the FFXI retail install to the Deck (one-time, ~19 GB, resumable)
#   --run ARGS...  after pushing, launch `ffxi-client ARGS` on the Deck's display.
#                  e.g. --run model-viewer   (no server needed — local DATs only)
#                       --run native --server <mac-ip>
#
# Env overrides:
#   DECK_HOST      ssh target (user@host or ~/.ssh/config alias). default deck@steamdeck.local
#   DECK_DIR       remote install dir.                            default /home/deck/ffxi
#   DECK_DAT       remote FFXI install (becomes FFXI_DAT_PATH).   default $DECK_DIR/dats
#   FFXI_INSTALL   local FFXI install to push with --assets.
#   DECK_DISPLAY   X11 display for --run (Desktop Mode).          default :0
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="ffxi-linux-build"
TARGET_VOL="ffxi-linux-target"

DECK_HOST="${DECK_HOST:-deck@steamdeck.local}"
DECK_DIR="${DECK_DIR:-/home/deck/ffxi}"
DECK_DAT="${DECK_DAT:-$DECK_DIR/dats}"
FFXI_INSTALL="${FFXI_INSTALL:-$REPO/vendor/Game/SquareEnix/FINAL FANTASY XI}"
DECK_DISPLAY="${DECK_DISPLAY:-:0}"

do_build=0 do_strip=0 do_assets=0 do_run=0
run_args=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --build)  do_build=1; shift ;;
        --strip)  do_strip=1; shift ;;
        --assets) do_assets=1; shift ;;
        --run)    do_run=1; shift; run_args=("$@"); break ;;
        -h|--help) sed -n '2,40p' "$0"; exit 0 ;;
        *) echo "unknown arg: $1 (try --help)" >&2; exit 2 ;;
    esac
done

# Reuse one SSH connection across all the calls below (fast, single auth).
SSH_CTL="$HOME/.ssh/cm-ffxi-deck"
SSH_OPTS=(-o ControlMaster=auto -o "ControlPath=$SSH_CTL" -o ControlPersist=120)
ssh_deck()  { ssh "${SSH_OPTS[@]}" "$DECK_HOST" "$@"; }
rsync_deck() { rsync -e "ssh ${SSH_OPTS[*]}" "$@"; }

if [[ $do_build == 1 ]]; then
    echo ">> building x86_64 binary..."
    "$REPO/docker/build-linux.sh"
fi

[[ -f "$REPO/dist/ffxi-client" ]] || {
    echo "!! no dist/ffxi-client — run with --build first." >&2; exit 1; }

# Pick the artifact to push: stripped copy, or the binary as-is.
PUSH_BIN="$REPO/dist/ffxi-client"
if [[ $do_strip == 1 ]]; then
    echo ">> stripping a copy (x86_64 strip, inside the build container)..."
    docker run --rm --platform linux/amd64 -v "$TARGET_VOL":/target "$IMAGE" \
        bash -c 'strip -s /target/release/ffxi-client -o /target/ffxi-client.stripped && ls -lh /target/ffxi-client.stripped'
    docker run --rm --platform linux/amd64 -v "$TARGET_VOL":/target "$IMAGE" \
        cat /target/ffxi-client.stripped > "$REPO/dist/ffxi-client.stripped"
    chmod +x "$REPO/dist/ffxi-client.stripped"
    PUSH_BIN="$REPO/dist/ffxi-client.stripped"
fi

echo ">> ensuring remote dir $DECK_HOST:$DECK_DIR"
ssh_deck "mkdir -p '$DECK_DIR'"

echo ">> pushing $(basename "$PUSH_BIN") -> $DECK_HOST:$DECK_DIR/ffxi-client"
rsync_deck -avz --progress "$PUSH_BIN" "$DECK_HOST:$DECK_DIR/ffxi-client"
ssh_deck "chmod +x '$DECK_DIR/ffxi-client'"

if [[ $do_assets == 1 ]]; then
    [[ -d "$FFXI_INSTALL" ]] || { echo "!! FFXI_INSTALL not found: $FFXI_INSTALL" >&2; exit 1; }
    echo ">> pushing FFXI install -> $DECK_HOST:$DECK_DAT  (large; --partial = resumable)"
    ssh_deck "mkdir -p '$DECK_DAT'"
    rsync_deck -a --partial --info=progress2 "$FFXI_INSTALL/" "$DECK_HOST:$DECK_DAT/"
fi

if [[ $do_run == 1 ]]; then
    echo ">> launching on Deck: ffxi-client ${run_args[*]}  (DISPLAY=$DECK_DISPLAY)"
    # -t for a TTY so logs stream back and Ctrl-C stops it. Desktop Mode = X11 :0.
    ssh_deck -t "cd '$DECK_DIR' && DISPLAY='$DECK_DISPLAY' FFXI_DAT_PATH='$DECK_DAT' ./ffxi-client ${run_args[*]}"
fi

echo ">> done."
