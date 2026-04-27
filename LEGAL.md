# Legal notice & attribution

Kuluu is a **fan-made, non-commercial game-preservation project**. This file
documents what Kuluu is (and is not), how it avoids redistributing copyrighted
material, the licenses it operates under, and credit to the upstream projects it
builds on. It is informational, not legal advice.

## 1. No affiliation; trademarks

FINAL FANTASY, FINAL FANTASY XI, FFXI, Vana'diel, PlayOnline, and all related
characters, music, art, names, and marks are trademarks and copyrights of
**Square Enix Holdings Co., Ltd.** Kuluu is an independent project with **no
affiliation with, sponsorship by, or endorsement from Square Enix**. Those
marks are used only nominatively, to describe what Kuluu is compatible with.

If you enjoy FINAL FANTASY XI, please support the official service.

## 2. No game assets are redistributed

Kuluu ships **zero** Square Enix game content. It contains no model, texture,
audio, animation, map, or other asset extracted from the FINAL FANTASY XI
client, and none has ever entered this repository's git history.

To run the renderer you must supply your own **legitimately obtained** retail
install (HorizonXI, a PlayOnline install, etc.). Kuluu reads those DAT files
from your local disk at runtime; it never bundles, hosts, uploads, or
redistributes them. The directory they live in (`vendor/game-files/`) is
git-ignored. See the README's "Getting the game files" section.

### Build-time derived constants are not game content

Some crates translate small, factual tables out of **open-source community
projects** (not out of SE asset files) at build time, baking the *values* into
the compiled binary as Rust constants. No copyrighted asset bytes are produced
or embedded by this process — only derived data (enum IDs, message/string
tables, format mappings) that already lives in those open-source repos under
their own licenses. The full map:

| Crate | Reads (build time) | Upstream | Upstream license |
| --- | --- | --- | --- |
| `ffxi-proto` | `blowfish.cpp`, `compress.dat`, `decompress.dat`, `msg_basic.h`, `msg.lua`, `effect.lua`, `job_name.lua`, `spell_list.sql`, `abilities.sql`, `item_basic.sql`, `item_equipment.sql` | [LandSandBoat/server][lsb] | GPL-3.0 |
| `ffxi-dat` | `zone_settings.sql` | [LandSandBoat/server][lsb] | GPL-3.0 |
| `ffxi-dat` | `ROMFileMappings.xml` | [Windower/POLUtils][pol] | Apache-2.0 |
| `ffxi-nav` | `zonelines.sql` | [LandSandBoat/server][lsb] | GPL-3.0 |
| `ffxi-audio` | `track_names.json` | [voliathon/AltanaListener][al] | (no declared license — see §6) |

`ffxi-nav-recast` additionally fetches navmesh data from
[LandSandBoat/xiNavmeshes][xinav] (GPL-2.0) **on demand at runtime**; that data
is not committed to this repository.

## 3. License

Kuluu is licensed under the **GNU General Public License, version 3 or later
(GPL-3.0-or-later)** — see [LICENSE](LICENSE).

GPL-3.0 is required (not merely chosen): Kuluu compiles derived data from
[LandSandBoat/server][lsb] (GPL-3.0) into its binaries, and studies the
behavior of [XIM][xim] (GPL-3.0). The strongest copyleft in the dependency
graph governs the combined work, so Kuluu and all of its first-party crates are
GPL-3.0-or-later.

This program is distributed in the hope that it will be useful, but **WITHOUT
ANY WARRANTY**; without even the implied warranty of MERCHANTABILITY or FITNESS
FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

## 4. Third-party components & attribution

Beyond the build-time tables above, Kuluu links or vendors:

| Component | Upstream | License | Use |
| --- | --- | --- | --- |
| recastnavigation-rs (vendored, patched) | `vendor/recastnavigation-rs` | MPL-2.0 | Navmesh build/query (Recast/Detour) |
| Bevy, wgpu, and the wider Rust crate graph | crates.io | MIT / Apache-2.0 | Engine, rendering, async, parsing — see `Cargo.lock` |

The recastnavigation-rs fork carries a one-line patch (documented in the root
`Cargo.toml`); its `LICENSE.txt` is retained under `vendor/recastnavigation-rs/`.

## 5. Reference-only material (`research/`, and cited upstreams)

Kuluu re-implements FFXI client behavior in its own code. Several community
projects are consulted **as references only** — their behavior is studied and
re-expressed; their source is **not** copied, pasted, linked, or redistributed
by this repository. They are either submodule pointers (URL + commit, no
upstream bytes committed) or fetched locally and git-ignored.

| Project | Role | License / status |
| --- | --- | --- |
| [XIM][xim] | Vanilla feature behavior reference (git-ignored, fetched locally) | GPL-3.0 |
| [phoenixffxi/Phoenix][phx] | Server-protocol reference (submodule pointer) | GPL-3.0 |
| [voliathon/AltanaViewer][av] | DAT/format behavior reference (submodule pointer) | No declared license — treated as all-rights-reserved; reference only |
| teschnei/lotus-ffxi | FFXI DAT / skeleton / audio **format** reference | No declared license — treated as all-rights-reserved; reference only (see §6) |

See [`research/README.md`](research/README.md) for the reference-only policy.

## 6. Provenance notes on unlicensed upstreams

Two of the sources above declare **no software license**. Under default
copyright that makes them all-rights-reserved, so Kuluu is deliberately
conservative with both:

- **teschnei/lotus-ffxi** — consulted only as a *reference* for publicly
  observable FFXI file formats (DAT chunk layout, skeleton/motion IDs, audio
  container/codec structure). Kuluu's `ffxi-audio` and DAT parsers are
  independent implementations of those formats; no lotus-ffxi source code is
  copied into, linked by, or redistributed by this repository.
- **voliathon/AltanaListener** — `ffxi-audio` bakes its `track_names.json` (a
  community-curated mapping of music-track IDs to descriptive track names) into
  a constant at build time. These are short factual/descriptive labels for
  in-game tracks, not creative content or game assets, and the source is
  attributed here and in the generated file. AltanaListener's own README
  likewise disclaims containing any Square Enix copyrighted material.

If you are a rights-holder for either project and would prefer a different
arrangement, please reach out (see §8) and we will adjust or remove the
reference promptly.

## 7. Not for the retail service

Kuluu is built to connect to **community-run FFXI-protocol servers**
(LandSandBoat, Phoenix). It makes no attempt to honor the retail service's
anti-cheat or Terms of Service and must not be pointed at the official servers.
Each community server has its own rules — read and follow them before you log
in.

## 8. Takedown / contact

This project respects intellectual-property rights and acts in good faith for
preservation and interoperability. If you believe something here infringes your
rights, please open an issue at
<https://github.com/jondwillis/kuluu-ffxi/issues> or reach a maintainer on the
[Discord](https://discord.gg/5c8NK46SuD), and we will respond promptly.

[lsb]: https://github.com/LandSandBoat/server
[xinav]: https://github.com/LandSandBoat/xiNavmeshes
[pol]: https://github.com/Windower/POLUtils
[al]: https://github.com/voliathon/AltanaListener
[av]: https://github.com/voliathon/AltanaViewer
[phx]: https://github.com/phoenixffxi/Phoenix
[xim]: https://xim.pages.dev/
