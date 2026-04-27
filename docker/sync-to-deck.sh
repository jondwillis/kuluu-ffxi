#!/usr/bin/env bash
# Ergonomic "build → Deck" in one command, over Syncthing.
#
# The Deck has no sshd (we can't sudo to start it), but Syncthing is already
# paired and syncing the `ffxi-deck` folder (~/Sync-ffxi-deck on this Mac →
# the Deck). So the fast loop is: build the x86_64 binary, strip it, drop it
# into that folder, and Syncthing pushes it to the Deck automatically — over a
# relay, so it works on any network (hotspot included).
#
# Usage:
#   docker/sync-to-deck.sh              # build (incremental) + strip + sync + confirm
#   docker/sync-to-deck.sh --no-build   # just re-sync the current dist/ binary
#   docker/sync-to-deck.sh --no-strip   # push the full 159 MB binary
#   docker/sync-to-deck.sh --no-wait    # don't block waiting for Deck delivery
#
# Env overrides:
#   FFXI_SYNC_DIR   the Syncthing-shared folder (default ~/Sync-ffxi-deck)
#   ST_FOLDER       Syncthing folder id used for the delivery check (default ffxi-deck)
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="ffxi-linux-build"
TARGET_VOL="ffxi-linux-target"
SYNC_DIR="${FFXI_SYNC_DIR:-$HOME/Sync-ffxi-deck}"
ST_FOLDER="${ST_FOLDER:-ffxi-deck}"

do_build=1 do_strip=1 do_wait=1
for a in "$@"; do case "$a" in
    --no-build) do_build=0 ;;
    --no-strip) do_strip=0 ;;
    --no-wait)  do_wait=0 ;;
    -h|--help)  sed -n '2,22p' "$0"; exit 0 ;;
    *) echo "unknown arg: $a" >&2; exit 2 ;;
esac; done

if [[ $do_build == 1 ]]; then
    echo ">> building x86_64 binary (incremental, Rosetta)..."
    "$REPO/docker/build-linux.sh"
fi
[[ -f "$REPO/dist/ffxi-client" ]] || { echo "!! no dist/ffxi-client — drop --no-build." >&2; exit 1; }

SRC="$REPO/dist/ffxi-client"
if [[ $do_strip == 1 ]]; then
    echo ">> stripping (x86_64 strip, in container)..."
    docker run --rm --platform linux/amd64 -v "$TARGET_VOL":/target "$IMAGE" \
        strip -s /target/release/ffxi-client -o /target/ffxi-client.stripped
    docker run --rm --platform linux/amd64 -v "$TARGET_VOL":/target "$IMAGE" \
        cat /target/ffxi-client.stripped > "$REPO/dist/ffxi-client.stripped"
    SRC="$REPO/dist/ffxi-client.stripped"
fi

mkdir -p "$SYNC_DIR"
echo ">> dropping $(basename "$SRC") ($(du -h "$SRC" | cut -f1)) into $SYNC_DIR/ffxi-client"
# Write to a temp name then rename, so Syncthing never publishes a half-copied
# binary (it would happily sync a truncated file mid-cp).
cp -f "$SRC" "$SYNC_DIR/.ffxi-client.tmp"
mv -f "$SYNC_DIR/.ffxi-client.tmp" "$SYNC_DIR/ffxi-client"

if [[ $do_wait == 0 ]]; then
    echo ">> queued for Syncthing. (skipped delivery wait)"; exit 0
fi

# Confirm the Deck actually received it, via the Syncthing REST API.
python3 - "$ST_FOLDER" <<'PY' || { echo ">> (couldn't query Syncthing; the file is queued and will sync regardless)"; exit 0; }
import json, os, re, sys, time, urllib.request
folder = sys.argv[1]
cfg = os.path.expanduser("~/Library/Application Support/Syncthing/config.xml")
apikey = re.search(r'<apikey>(.*?)</apikey>', open(cfg).read()).group(1)
B = "http://127.0.0.1:8384"
def get(p):
    r = urllib.request.Request(B+p, headers={"X-API-Key": apikey})
    return json.load(urllib.request.urlopen(r, timeout=5))
# Remote devices sharing this folder (everything except ourselves).
me = get("/rest/system/status")["myID"]
fol = next((f for f in get("/rest/config/folders") if f["id"] == folder), None)
if not fol:
    print(f">> no Syncthing folder '{folder}'; file is in place locally.", flush=True); sys.exit(0)
remotes = [d["deviceID"] for d in fol["devices"] if d["deviceID"] != me]
if not remotes:
    print(">> folder isn't shared with any device yet.", flush=True); sys.exit(0)
print(">> waiting for Syncthing to deliver to the Deck...", flush=True)
for _ in range(60):
    worst = 100.0
    for dev in remotes:
        c = get(f"/rest/db/completion?folder={folder}&device={dev}")
        worst = min(worst, c.get("completion", 0))
    if worst >= 100.0:
        print(">> delivered to Deck (100%). Re-run ./ffxi-client on the Deck.", flush=True); sys.exit(0)
    print(f"   {worst:5.1f}% ...", flush=True); time.sleep(3)
print(">> still syncing after 3 min — it'll finish in the background.", flush=True)
PY
