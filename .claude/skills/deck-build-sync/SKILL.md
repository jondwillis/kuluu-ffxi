---
name: deck-build-sync
description: >
  Build a release ffxi-client binary for x86_64 Linux and ship it to the
  paired Steam Deck over Syncthing. Use whenever the user asks to build a
  release/Deck/Linux binary, "push to the deck", "sync to my steam deck",
  test on the Deck, or update the build the Deck is running — even if they
  just say "build and sync" or "get the latest onto the deck". Covers the
  Apple-Silicon-host-to-x86_64-Linux-target cross build via Rosetta-backed
  Docker/colima; do not hand-roll a fresh cross-compile setup, the scripts
  already exist.
---

# Building and syncing to the Steam Deck

This is a thin orchestration skill: the actual build and sync logic lives in
`docker/build-linux.sh` and `docker/sync-to-deck.sh`, already in this repo.
Never re-derive or duplicate that logic — just drive it correctly and
recognize its failure modes.

## Why this isn't a normal cross-compile

The Mac host is aarch64 (Apple Silicon); the Steam Deck target is x86_64
Linux. Rather than cross-compile (painful for the cxx/Recast C++ bridge),
`docker/build-linux.sh` runs a **native x86_64 toolchain inside an emulated
linux/amd64 container**. That emulation must be backed by Rosetta, not QEMU.

## Step 1 — colima must be running with Rosetta

```bash
colima status
```

If it's not running, or you're not sure what backs its emulation, start (or
restart) it explicitly:

```bash
colima stop 2>/dev/null; colima start --vz-rosetta
```

**This is the most common failure mode.** colima's bundled `qemu-x86_64`
mis-emulates futexes and will silently deadlock a multi-threaded `rustc`/cargo
build partway through — not a clean error, just a hang. `--vz-rosetta`
requires `vmType: vz` (the colima default on Apple Silicon) and only works on
Apple Silicon hosts. If a build hangs with no progress for several minutes
with high CPU but no compiler output, suspect this before anything else —
kill it, restart colima with `--vz-rosetta`, retry.

## Step 2 — run the build+sync script

From the repo root:

```bash
docker/sync-to-deck.sh
```

Run this **in the background** (it's a real build, not a quick check) —
cold builds take ~40 minutes; incremental rebuilds are much faster since the
crate cache and target dir persist in named Docker volumes across runs. Don't
poll it; wait for it to finish and then read its output.

What it does, in order (see the script's own header comments for the full
rationale — Docker named volumes instead of bind-mounts, because colima's
host bind-mounts are unreliable):

1. Builds `ffxi-client --release --locked --features native-window` for
   x86_64 Linux via `docker/build-linux.sh`.
2. Strips the binary inside the container.
3. Drops it into the Syncthing-shared folder (`~/Sync-ffxi-deck` by default),
   which is already paired with the Deck and pushes automatically — works
   over a relay, so it doesn't need both devices on the same network.
4. Polls the Syncthing REST API until it confirms 100% delivery to the Deck,
   using the API key from `~/Library/Application Support/Syncthing/config.xml`.

Useful flags (pass through to the script):

| Flag | Effect |
|---|---|
| `--no-build` | Skip the build, just re-sync the current `dist/ffxi-client` |
| `--no-strip` | Push the full unstripped ~235MB binary instead of the ~144MB stripped one |
| `--no-wait` | Queue the sync and return immediately, without blocking on Syncthing delivery confirmation |

Env overrides if the user's setup differs from defaults:
`FFXI_SYNC_DIR` (Syncthing folder path, default `~/Sync-ffxi-deck`),
`ST_FOLDER` (Syncthing folder id, default `ffxi-deck`).

## Step 3 — confirm and hand off

The script's last line is the confirmation to look for:

```
>> delivered to Deck (100%). Re-run ./ffxi-client on the Deck.
```

Report that back to the user. On the Deck side there is nothing further for
you to do — the user just re-launches `./ffxi-client` there themselves.

If the Syncthing REST query fails (e.g. the API key/config path differs, or
Syncthing isn't running locally), the script degrades gracefully with a
"couldn't query Syncthing; the file is queued and will sync regardless"
message — treat that as a soft success, not a failure, and say so plainly
rather than treating it as broken.

## Don't

- Don't write a new Dockerfile, cross-compile toolchain, or rsync/tar
  pipeline — `docker/Dockerfile.linux-build`, `docker/build-linux.sh`, and
  `docker/sync-to-deck.sh` already solve this.
- Don't try to SSH into the Deck — it has no sshd the user can start without
  sudo access they don't want to grant; Syncthing is the intended delivery
  path.
- Don't run the build in the foreground and block the conversation for 40
  minutes; background it.
