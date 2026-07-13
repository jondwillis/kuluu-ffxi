#!/usr/bin/env bash
# Build a release ffxi-client and install it for local use on this machine
# (Steam Deck or otherwise): copies the binary to ~/.local/bin and adds a
# .desktop entry so it shows up in the app menu / can be added as a Steam
# non-Steam-game shortcut. Credentials and FFXI_DAT_PATH are left for the
# launcher's own interactive prompts (see AGENTS.md "Running") rather than
# baked into the desktop entry.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

echo "building release binary (LLVM backend, this takes a while)..."
cargo build -p ffxi-client --release --features native-window

bin_dir="$HOME/.local/bin"
mkdir -p "$bin_dir"
install -m 755 target/release/ffxi-client "$bin_dir/ffxi-client"
echo "installed: $bin_dir/ffxi-client"

desktop_dir="$HOME/.local/share/applications"
mkdir -p "$desktop_dir"
desktop_file="$desktop_dir/kuluu-ffxi.desktop"
cat > "$desktop_file" <<EOF
[Desktop Entry]
Type=Application
Name=Kuluu FFXI
Comment=Open-source FFXI client for LandSandBoat/Phoenix private servers
Exec=$bin_dir/ffxi-client play
Terminal=false
Categories=Game;
EOF
echo "installed: $desktop_file"

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "note: $bin_dir is not on PATH — add it to your shell profile to run 'ffxi-client' directly" ;;
esac

echo "done. Launch via the app menu, or 'ffxi-client play' if $bin_dir is on PATH."
