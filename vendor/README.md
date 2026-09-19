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
| `AltanaListener/` | [voliathon/AltanaListener](https://github.com/voliathon/AltanaListener) | `track_names.json`: BGM track names |
| `DLSS/` | [NVIDIA/DLSS](https://github.com/NVIDIA/DLSS) | Optional. `update = none`; only `cargo xtask dlss` initializes it |

Initialize the three the build needs, shallowly:

```bash
git submodule update --init --depth 1 vendor/server vendor/POLUtils vendor/AltanaListener
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
  and `VER_LOCK`. A stock server at that pin admits exactly that client
  version while locked, so an install from another generation needs the lock
  off. The zone text ids under `scripts/zones/*/IDs.lua` are synced to the same
  client: the matching retail generation reads them as identity DAT indexes,
  and older clients go through the landmark reconciliation in `kuluu-session`.
- **POLUtils.** `ROMFileMappings.xml` was last edited in 2018 for the Unity
  dialog tables, and before that for the 2015 Reisenjima update. It keys on
  absolute file ids up to 86528, which still resolve on current installs, but
  its zone associations and map counts can be wrong. Dialog DAT ids therefore
  follow the client's zone formula through VTABLE/FTABLE, map selection uses
  the installed DLL's zone-map records, and autotranslate item and key-item
  names come from the installed DATs with the LSB dictionary as fallback.
- **AltanaListener.** `track_names.json` is hand-curated, not client-derived,
  so it has no build to match. The upstream repository is archived and the pin
  stays frozen at v1.0.4.

## Vendored crates

`recastnavigation-rs/`, `cab/` and `bevy_pbr/` are patched copies of crates.io
releases wired through `[patch.crates-io]` in the workspace `Cargo.toml`,
which states each patch's reason. Grep `KULUU PATCH` for the changed lines.
