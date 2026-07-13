#!/usr/bin/env bash
# Build a release ffxi-client and drop it into the existing local install
# layout (~/.local/share/kuluu-ffxi/bin/ffxi-client), which the
# ~/.local/bin/kuluu-ffxi wrapper script and its Steam non-Steam-game
# shortcut already point at. Only updates the binary in place — doesn't
# touch the wrapper script, .desktop entry, or Steam shortcut config, since
# those already exist and work.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

echo "building release binary (LLVM backend, this takes a while)..."
cargo build -p ffxi-client --release --features native-window

install_dir="$HOME/.local/share/kuluu-ffxi/bin"
mkdir -p "$install_dir"
install -m 755 target/release/ffxi-client "$install_dir/ffxi-client"
echo "installed: $install_dir/ffxi-client"

wrapper="$HOME/.local/bin/kuluu-ffxi"
if [ -x "$wrapper" ]; then
    echo "existing launcher wrapper found: $wrapper (unchanged)"
else
    echo "note: no launcher wrapper at $wrapper — launch $install_dir/ffxi-client directly, or create one"
fi
