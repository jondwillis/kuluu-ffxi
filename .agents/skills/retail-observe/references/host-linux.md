# Host: Linux (Wine, Proton, Steam Deck)

Use `scripts/observe.sh`; it selects the Linux backend automatically. Input goes
through **xdotool**, pixels through **maim** (or ImageMagick `import`), OCR
through **tesseract**.

**Verification status:** written against documented tool behavior, not yet
exercised against a running client. `doctor` checks every prerequisite -- run it
first and fix what it reports rather than writing a parallel script.

Wine and Proton both present the client as an ordinary X window, so one code
path covers both. What actually differs is the display server and how you start
the client.

## X11 or Xwayland, not pure Wayland

Per-window capture and targeted input need an X window id. Under a pure Wayland
session there is no such handle: a compositor will not let an arbitrary client
screenshot another window or synthesize input into it. Three ways out, best
first:

1. **Run the client under gamescope.** It gives a stable, predictably sized
   window, and the client inside it is an X client, so xdotool and maim work
   normally. This is also the Steam Deck's native arrangement.
2. **Run it under Xwayland** and export `DISPLAY` for the shell driving
   `observe.sh`.
3. **Wayland-native tooling** -- `grim`/`slurp` for pixels, `ydotool` for input
   (which needs `uinput` permissions and a running daemon). There is no
   per-window targeting: you capture a region and input goes to whatever has
   focus, so every recipe becomes coordinate-fragile. Treat this as a last
   resort and say so in any observation record produced that way.

`doctor` fails loudly with this guidance when it sees `WAYLAND_DISPLAY` and no
`DISPLAY`, rather than producing empty captures.

## Synthetic vs real input

xdotool has two delivery modes and the difference decides whether the game
reacts at all:

- **XTEST** (default here) -- real input to the focused window. `observe.sh`
  activates the client window, then sends. This is what works with a
  DirectInput client under Wine.
- **XSendEvent** (`--window`, what `--bg` uses) -- synthetic events addressed to a
  window without focusing it. Wine passes these to DirectInput apps
  inconsistently, so `--bg` warns and is only worth trying when stealing focus
  is worse than a dropped key.

If a key silently does nothing, this is the first thing to check -- not the key
table.

## Running the client

Plain `wine` is the simple path and needs no Steam. For Proton without Steam's
bookkeeping, **umu-launcher** (`umu-run`) is the modern generic answer: it sets
up the Proton runtime and prefix for a non-Steam executable, which is exactly
the "what does Steam do" plumbing you would otherwise hand-assemble
(`STEAM_COMPAT_DATA_PATH` and `STEAM_COMPAT_CLIENT_INSTALL_PATH` around
`proton run`). `observe.sh` prefers `umu-run` when present, else `wine`;
override with `FFXI_OBSERVE_RUNNER`.

On a **Steam Deck**, add the loader as a non-Steam game so Proton and gamescope
wrap it, then drive it from Desktop Mode or over SSH with `DISPLAY` pointing at
the session. The client is 32-bit and DirectX 8, so the same wrapper concerns
apply as on macOS; nothing about `observe.sh` changes.

The rest of the launch story -- loaders, pointing the client at a private server
or a local LandSandBoat stack, resolution and config traps, credentials -- is in
[client-and-server.md](client-and-server.md).

## OCR

`tesseract` is required for `ocr`/`click-text`. Its TSV output is word-level, so
the backend groups words back into visual lines before matching, which is what
lets a regex match a phrase the way it reads on screen. Expect worse accuracy
than macOS Vision on small UI text; when a match keeps failing, read the capture
rather than loosening the regex until it matches something wrong.

## Troubleshooting

| Symptom | Cause -> fix |
|---|---|
| `no DISPLAY` | Pure Wayland session -> gamescope/Xwayland, per above |
| No window matches but the client is running | Launcher window titled differently -> `targets`, then `FFXI_OBSERVE_WINDOW_TITLE` |
| Keys do nothing | `--bg` synthetic events being ignored by DirectInput -> drop `--bg` |
| Capture is empty or black | No maim/ImageMagick, or a compositor redirecting the window -> `doctor`; try capturing with the window raised |
| Clicks land off by a constant offset | Window geometry read before a move/resize -> re-run, since coordinates do not survive it |
| `ocr` refuses | tesseract missing -> install it |
| Everything works, then stops after the client changes resolution | Window id survives but geometry changed -> nothing cached is valid; each verb re-resolves, so just re-run |
