#!/usr/bin/env bash
# Build `ffxi-client --features native-window` for x86_64 Linux (Steam Deck)
# from an Apple-Silicon Mac, using an emulated linux/amd64 container.
#
# Usage:  docker/build-linux.sh [cargo-args...]
#   default: build --release --locked -p ffxi-client --features native-window
#
# Why this shape:
#  * Target is x86_64 (Steam Deck) but the host is aarch64, so every docker
#    step runs with --platform linux/amd64. The toolchain inside the container
#    is therefore a NATIVE x86_64 rustc+clang, which sidesteps all the
#    cxx/Recast cross-compilation pain.
#  * REQUIRES ROSETTA, not plain QEMU. colima's bundled qemu-x86_64 deadlocks
#    multi-threaded rustc/cargo (mis-emulated futex) partway through a build
#    this size. Start colima with Rosetta first:
#        colima stop && colima start --vz-rosetta
#    (vmType must be `vz`; Apple Silicon only). Rosetta also compiles much
#    faster — a full build is ~10-15 min, incremental rebuilds far less.
#  * The host Docker runtime here is colima, whose host bind-mounts are
#    unreliable, so we never bind-mount the repo. Instead the build-relevant
#    source subset (everything EXCEPT the 19 GB vendor/game-files, target/, .git,
#    and cite-only vendor dirs no build.rs reads) is streamed via tar into a
#    Docker NAMED VOLUME that lives inside the VM. The crate cache and a
#    Linux-only target dir are likewise named volumes, so the host's macOS
#    target/ is never touched.
#  * CXXFLAGS is forced empty to neutralise the macOS-SDK isysroot in
#    .cargo/config.toml (cargo's non-forcing [env] yields to an already-set
#    process var), letting the container's clang use its native header paths.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="ffxi-linux-build"
PLATFORM="linux/amd64"
STAGE="${FFXI_LINUX_STAGE:-$HOME/.cache/ffxi-linux-src}"
SRC_VOL="ffxi-linux-src"
CARGO_VOL="ffxi-linux-cargo"
TARGET_VOL="ffxi-linux-target"

CARGO_ARGS=("$@")
if [ "${#CARGO_ARGS[@]}" -eq 0 ]; then
    CARGO_ARGS=(build --release --locked -p ffxi-client --features native-window)
fi

echo ">> [1/5] staging build source into $STAGE (excluding vendor/game-files, target, .git)..."
mkdir -p "$STAGE"
rsync -a --delete \
    --exclude='/target/' --exclude='/dist/' --exclude='/.git/' \
    --exclude='/vendor/game-files/' --exclude='/vendor/Phoenix/' \
    --exclude='/vendor/xi-tinkerer/' --exclude='/vendor/RZN-mapviewer/' \
    --exclude='/vendor/AltanaViewer/' \
    --exclude='/.omc/' --exclude='/.omo/' --exclude='/.claude/' \
    --exclude='*.gz' --exclude='.DS_Store' --exclude='._*' \
    "$REPO/" "$STAGE/"

echo ">> [2/5] building amd64 toolchain image ($IMAGE)..."
docker build --platform "$PLATFORM" \
    -f "$REPO/docker/Dockerfile.linux-build" -t "$IMAGE" "$REPO/docker"

echo ">> [3/5] loading source into named volume ($SRC_VOL) via tar stream..."
docker volume create "$SRC_VOL" >/dev/null
# Wipe any stale contents, then extract the fresh tar streamed over stdin.
# COPYFILE_DISABLE=1 stops macOS bsdtar from fabricating `._*` AppleDouble
# companion files for any source carrying extended attributes — otherwise
# Recast's build globs `*.cpp` and tries to compile the junk `._Foo.cpp`,
# failing with "source file is not valid UTF-8". The post-extract `find`
# is belt-and-suspenders against any that slip through.
# `._*` AppleDouble entries are SYNTHESISED by macOS bsdtar from each file's
# extended attributes — they are not real files on disk, so a host-side
# rsync/find exclude can't catch them and COPYFILE_DISABLE is unreliable here.
# The robust fix is on the EXTRACT side: GNU tar in the Linux container honours
# `--exclude='._*'` no matter what the archive contains. The trailing find is a
# final backstop.
COPYFILE_DISABLE=1 tar -C "$STAGE" -cf - . \
  | docker run --rm -i --platform "$PLATFORM" -v "$SRC_VOL":/src "$IMAGE" \
        bash -euo pipefail -c 'find /src -mindepth 1 -delete 2>/dev/null || true; tar -C /src --exclude="._*" -xf -; find /src -name "._*" -delete'

echo ">> [4/5] cargo ${CARGO_ARGS[*]}  (emulated x86_64 — this is the slow part)"
docker run --rm -t --platform "$PLATFORM" \
    -v "$SRC_VOL":/src \
    -v "$CARGO_VOL":/opt/cargo/registry \
    -v "$TARGET_VOL":/target \
    -e CARGO_TARGET_DIR=/target \
    -e CXXFLAGS= \
    -e CARGO_NET_GIT_FETCH_WITH_CLI=true \
    -w /src \
    "$IMAGE" \
    bash -euo pipefail -c 'cargo "$@"; ls -lh /target/release/ffxi-client' _ "${CARGO_ARGS[@]}"

echo ">> [5/5] extracting binary to dist/ffxi-client..."
mkdir -p "$REPO/dist"
docker run --rm --platform "$PLATFORM" -v "$TARGET_VOL":/target "$IMAGE" \
    cat /target/release/ffxi-client > "$REPO/dist/ffxi-client"
chmod +x "$REPO/dist/ffxi-client"

echo ">> done. Binary: $REPO/dist/ffxi-client"
ls -lh "$REPO/dist/ffxi-client"
