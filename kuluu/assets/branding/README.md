# Branding

The Southern Watcher mark combines a curled green tail, an amber lantern and
three stars. It is original AI-generated artwork inspired by FFXI's Tonberry
lantern and constellation lore, not an official crest and not an extracted
game asset. The star arrangement is an interpretation.

## Files

- `kuluu-master.png` is the transparent source. `generation.json` holds its
  prompt and provenance.
- `png/`, `kuluu.ico`, `kuluu.icns` and `kuluu.rc` are the generated sizes for
  Linux desktop entries, the Windows executable and the macOS bundle. They are
  committed so builds need no image tools.
- `social/` holds the shared artwork. The banner and splash share a lantern-lit
  jungle sanctuary; their prompts are in `social/generation.json`.

| Asset | Size |
| --- | --- |
| [GitHub preview](social/github-preview.png) | 1280 x 640 |
| [Discord avatar](social/discord-avatar.png) | 512 x 512, transparent |
| [Discord server banner](social/discord-banner.png) | 960 x 540 |
| [Discord invite splash](social/discord-invite-splash.png) | 1920 x 1080 |

## Regenerating

On macOS, `bash scripts/export-icons.sh` (requires `sips` and Python 3)
rewrites the PNG sizes, the Windows ICO, the macOS ICNS and the browser viewer
copies from the master.

## Where the mark ships

Linux release archives carry `install-local.sh`, which installs the desktop
icon with the binary. macOS archives carry `Kuluu.app`. Windows executables
embed the icon through `kuluu.rc`. The browser viewer uses it for its favicon
and web manifest. `kuluu steam-shortcut install` sets it as the default Steam
shortcut icon while preserving a custom one. Android and iOS packages do not
exist yet; their adaptive icons should derive from the master with
platform-specific backgrounds and safe-area padding.
