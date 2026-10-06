# vendor/

Build-time inputs only. `build.rs` in `ffxi-proto`, `ffxi-vocab`, `ffxi-dat`,
`kuluu-nav` and `ffxi-audio` (through the shared `lsb-scrape` helper) read
data files out of these trees and emit compile-time Rust constants. Nothing
under here is needed at runtime, and no game assets pass through it. Never
hand-copy a value from here into source: bump the pin and let the build
regenerate it (the
[vendor-scrape skill](../.agents/skills/vendor-scrape/SKILL.md)).

## Submodules

| Path | Upstream | What the build reads |
| --- | --- | --- |
| `server/` | [LandSandBoat/server](https://github.com/LandSandBoat/server) | SQL, headers, lua and YAML: blowfish subkeys, zlib tables, message/effect/job/spell/item names, zone text ids, login settings |
| `POLUtils/` | [Windower/POLUtils](https://github.com/Windower/POLUtils) | `ROMFileMappings.xml`: ROM file mappings and zone-DAT id formulas |
| `DLSS/` | [NVIDIA/DLSS](https://github.com/NVIDIA/DLSS) | Optional. `update = none`; only `cargo xtask dlss` initializes it |

Initialize the two the build needs, shallowly:

```bash
git submodule update --init --depth 1 vendor/server vendor/POLUtils
```

`--depth 1` works only while the pinned commit is still reachable from its
tracked branch tip. If an upstream force-push moves it out of reach, re-run
without `--depth` for that submodule.

## Client eras

Each pin carries its own client generation, and it is not the installed
client's. Installed clients are identified at startup against `KNOWN_CLIENTS`
in `ffxi-dat/src/client_profile.rs`; `kuluu install list` shows each install's
row. Read a pin's date with `git -C vendor/<name> log -1 --format=%cs`.

- **server.** The pin's `settings/default/login.lua` declares `CLIENT_VER`
  and `VER_LOCK`. Strict mode (`1`) requires an exact version match; the
  default mode (`2`) allows matching or newer versions, and `0` disables the
  version check. Read the pinned settings before choosing a client. The zone text ids under `scripts/zones/*/IDs.lua` are synced to the same
  client: the matching retail generation reads them as identity DAT indexes,
  and older clients go through the landmark reconciliation in `kuluu-session`.
- **POLUtils.** `ROMFileMappings.xml` was last edited in 2018 for the Unity
  dialog tables, and before that for the 2015 Reisenjima update. It keys on
  absolute file ids up to 86528, which still resolve on current installs, but
  its zone associations and map counts can be wrong. Dialog DAT ids therefore
  follow the client's zone formula through VTABLE/FTABLE, map selection uses
  the installed DLL's zone-map records, and autotranslate item and key-item
  names come from the installed DATs with the LSB dictionary as fallback.
- **AltanaListener (no submodule).** `track_names.json` is hand-curated, not
  client-derived, so it has no build to match. Upstream archived the repo on
  2026-08-27 in favour of vekien/xi-model-viewer and it later went private,
  so the submodule could not be cloned; its one build input is committed as
  regular content at the frozen pin (v1.0.4, commit
  `055fc2a26d4f0f8ef8bbb9b5f341f3bc232950c6`). Updating it means replacing
  the file.

## Vendored crates

`recastnavigation-rs/`, `cab/` and `bevy_pbr/` are patched copies of crates.io
releases wired through `[patch.crates-io]` in the workspace `Cargo.toml`,
which states each patch's reason. Grep `KULUU PATCH` for the changed lines.
