# research/

Read-only third-party material used while re-implementing FFXI client
behavior. **Nothing under here is redistributed by this repo** — it is either
a submodule pointer (URL + commit, no upstream bytes committed) or fetched on
demand and gitignored.

Treat everything here as **reference only**: study behavior and re-express it
in our own code. Do not copy, paste, or link third-party source into the
workspace crates.

## Contents

- `Phoenix/`, `AltanaViewer/` — submodule pointers to upstream repos cited in
  source comments. Deinitialized by default; populate on demand with
  `git submodule update --init research/<name>`.
- `xim/` — the XIM browser FFXI client (**gitignored**, fetched locally). See
  below.

## XIM

[XIM](https://xim.pages.dev/) is a from-scratch browser FFXI client PoC. It's a
useful reference for vanilla feature behavior — actor/animation handling,
packet flow, DAT parsing — when filling in the parity scoreboard.

- Live app:   <https://xim.pages.dev/>
- Source zip: <https://xim.pages.dev/source.zip>
- Docker:     <https://github.com/Masin-M/xim-docker>

**License: GPL-3.** Fetch a local copy with:

```bash
research/fetch-xim.sh
```

This downloads and extracts the source to `research/xim/`, which is gitignored
so the GPL-3 source never enters our history. Re-run the script to refresh.
