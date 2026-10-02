<p align="center">
  <img src="kuluu/assets/branding/social/github-preview.png" alt="Kuluu — an open-source FINAL FANTASY XI client. A moss-green curled tail bears three stars and an amber lantern." width="960">
</p>

<p align="center">
  <em>A faithful, open-source FINAL FANTASY XI client — rebuilt in Rust + Bevy,
  running on a modern engine at 60+ FPS.</em>
</p>

<p align="center">
  <a href="https://discord.gg/5c8NK46SuD"><img alt="Discord" src="https://img.shields.io/badge/discord-join-5865F2.svg?logo=discord&logoColor=white"></a>
  <a href="LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg"></a>
</p>
<p align="center">
  <a href="https://github.com/jondwillis/kuluu-ffxi/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/jondwillis/kuluu-ffxi/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/jondwillis/kuluu-ffxi/releases"><img alt="GitHub Release" src="https://img.shields.io/github/v/release/jondwillis/kuluu-ffxi?logo=GitHub"></a>
</p>

<p align="center">
  <a href="#setup--first-build">Get started</a> &middot;
  <a href="https://github.com/jondwillis/kuluu-ffxi/releases">Downloads</a> &middot;
  <a href="#project-goals">Project goals</a> &middot;
  <a href="#roadmap">Roadmap</a> &middot;
  <a href="https://discord.gg/5c8NK46SuD">Community</a>
</p>

Kuluu is a **fan-community game-preservation project**: a cross-platform, modern, extensible, open-source client for the
FINAL FANTASY XI network protocol.

FINAL FANTASY XI is a Square Enix property. **Kuluu has no affiliation with,
and no endorsement from, Square Enix, and ships no game assets.** If you enjoy
FFXI, please support the official service. See [LEGAL.md](LEGAL.md).

<p align="center">
  <video src="https://github.com/user-attachments/assets/8bc7375b-262c-4074-965e-f073b342430a" controls width="640"></video>
</p>

## Project goals

- **Vanilla parity is the base.** The aim is close to 1:1 with the official
  FFXI client in default mode — same menus, same compass, same combat feel.
  Anything with no retail equivalent is an **Enhanced / addon** feature (the
  `enhanced` label in the [backlog](#roadmap)), gated behind a feature flag or
  build flavor so it never compromises vanilla by default.
- **Modernization layers on top, opt-in.** Bevy + wgpu replace the legacy
  D3D8/D3D11 stack; a planned plugin/extension API aims to obviate
  Windower/Ashita.
- **No asset redistribution.** Kuluu requires a user-provided retail install.
  Tables translated from LandSandBoat, POLUtils, etc. are stored as derived
  compile-time constants under the upstream license — never as game content.
  See [LEGAL.md](LEGAL.md).

## Setup / first build

The workspace builds from a clean checkout once the few **build-time** vendor
submodules are present. Build scripts read data files out of them and translate
the values into compile-time Rust constants; no copyrighted asset bytes leave
the user's machine, and the submodules are *not* needed at runtime.

```bash
git clone https://github.com/jondwillis/kuluu-ffxi && cd kuluu-ffxi

# Init only the submodules the build actually reads (shallow — history trimmed).
# `server` is large; --depth 1 keeps the working tree without the full history.
git submodule update --init --depth 1 \
  vendor/server vendor/POLUtils vendor/AltanaListener

# recastnavigation-rs is vendored in-tree (no submodule).
cargo build
```

**Enable the git hooks (once per clone).** A `pre-push` gate runs the same
fmt + clippy as CI so a red build is caught before you push. It's off until you
opt in (git won't let a repo auto-enable its own hooks):

```bash
cargo xtask install-hooks          # sets core.hooksPath=.githooks
cargo xtask install-hooks --check  # verify it's active (non-zero exit if not)
```

Bypass a single push with `git push --no-verify`; `PREPUSH_FAST=1 git push`
runs fmt only. (`scripts/install-hooks.sh` does the same thing without a build.)

That's everything the compiler needs. Upstream repos that are **not used by
the build** — only cited in source comments for reference (`xi-model-viewer`,
`XiEvents`, `XiPackets`, `XIClient`, `xi-tools`) — live under `research/`, not
`vendor/`. They stay deinitialized; `git submodule update --init
research/<name>` populates one if you want to read the upstream sources.
Phoenix is **not** a submodule: it is private, so clone it yourself into
`research/Phoenix` (git-ignored) if you want the server-side divergence
signal.

To actually *run* the client you also need a user-provided retail install
(~19G, never committed — see [Getting the game files](#getting-the-game-files)).

> **Shallow-clone caveat:** `--depth 1` works only while the pinned submodule
> commit is still reachable from its tracked branch tip. If an upstream
> force-push moves it out of reach, re-run without `--depth` (or with a larger
> `--depth N`) for that submodule.

## Run

Native window (the default `play` mode; GUI ships by default):

```bash
cargo run -p kuluu -- play
```

Headless (JSON-line agent session, useful for protocol work and for driving the
client from an automation/LLM harness via the MCP bridge):

```bash
cargo run -p kuluu --no-default-features -- play --headless
```

If any credential env var is unset, the launcher prompts for it and lists
characters on the account so you can pick by name.

### Optional DLSS builds

DLSS Super Resolution is an opt-in enhancement, absent from standard builds
and off in Graphics settings until selected. It requires an NVIDIA RTX GPU
and Vulkan on x86_64 Windows or Linux; macOS and browser builds do not support it.

The optional `vendor/DLSS` submodule pins NVIDIA's SDK v310.5.3, matching
[`dlss_wgpu` 4.0.0](https://github.com/bevyengine/dlss_wgpu/tree/323ba14a80b26718093ca4bebe9f6c1b6fef5e57).
Normal submodule setup skips it. Install the Vulkan SDK and libclang, set
`VULKAN_SDK` to the SDK root, and set `LIBCLANG_PATH` if automatic discovery
fails. Then run:

```bash
cargo xtask dlss build
```

This initializes the pinned SDK, builds the release client with `dlss`, and
stages its matching SR runtime, license, and programming guide (including
upstream attribution notices) beside the executable under
`target/<host-target>/release/` (or `CARGO_TARGET_DIR`). It uses the Windows
MSVC or Linux GNU target. `DLSS_SDK` can override the pinned SDK directory;
`cargo xtask dlss check` checks SDK files and Vulkan headers without downloading
or building. The SDK and its runtime remain subject to
[NVIDIA's license](https://github.com/NVIDIA/DLSS/blob/v310.5.3/LICENSE.txt).

Enable DLSS in Graphics and choose its quality in `DLSS Config`. It owns
anti-aliasing and render resolution while active. An installed SDK never
changes the normal check gate; explicitly include SR with
`KULUU_CHECK_DLSS=1 scripts/checks.sh clippy test build`.

**Neural Uplift is a separate experimental Windows-only enhancement**, gated by
`enhanced-neural-uplift` and off by default. Its runtime is not included in the
SDK or downloaded by the build helper. To test it, first stage SR with the
helper above, then build the client and forwarder into that same directory:

```powershell
if (-not $env:DLSS_SDK) { $env:DLSS_SDK = (Resolve-Path vendor/DLSS).Path }
cargo build -p kuluu -p kuluu-ngx-fwd --locked --release --target x86_64-pc-windows-msvc --features native-window,enhanced-neural-uplift
Copy-Item target/x86_64-pc-windows-msvc/release/kuluu_ngx_fwd.dll target/x86_64-pc-windows-msvc/release/nvngx.dll_kuluu.dll
```

Supply `nvngx_dlssnr.dll` beside the executable and enable `Neural Uplift` in
`DLSS Config` with DLSS active. Adjust these paths when using `CARGO_TARGET_DIR`.
The forwarder's staged filename must remain `nvngx.dll_kuluu.dll`. Camera-motion
quality still needs validation; the current NR path supplies zero motion vectors.

### Getting the game files

The FFXI client DATs (geometry, textures, audio, animations) are Square Enix
copyrighted and must come from a **legitimate install** — Kuluu never ships or
commits them. Kuluu keeps a registry of named installs in your user data
directory (`~/Library/Application Support/kuluu/installs/NAME` on macOS,
`~/.local/share/kuluu/installs/NAME` on Linux and the Steam Deck) with a
one-line `default` file naming the one that loads. `FFXI_DAT_PATH` overrides
it for one run. That is the whole model; `kuluu install which` says which
applied and why.

Get an install one of these ways:

- **Square Enix's official client:** `kuluu install get` (or the launcher's
  "Get the official client" button) downloads, patches and selects it in one
  shot, asking before each step unless told otherwise. Free to download; a
  registration code / subscription is needed to play on the official service.
- **HorizonXI launcher (Windows):** install via <https://horizonxi.com>; its
  launcher downloads a full FFXI + Ashita tree.
- **Lutris (Linux):** <https://lutris.net/games/horizonxi/>. Files land under
  `~/Games/.../drive_c/.../SquareEnix/FINAL FANTASY XI/`.
- **Copy an existing install:** the PlayOnline tree from any retail/private-server
  install (`.../PlayOnline/SquareEnix/FINAL FANTASY XI/`).

Each registered install is a directory holding the parent of `SquareEnix/`:

```
installs/NAME/
  SquareEnix/
    FINAL FANTASY XI/      <- the DAT root, what FFXI_DAT_PATH would name
      VTABLE.DAT  FTABLE.DAT
      ROM/  ROM2/ … ROM9/
      sound/win/…
```

Register an install you already have under a name and make it the default.
With no path, `link` detects one (HorizonXI / Lutris / Wine / CrossOver /
PlayOnline), validates it, and symlinks it into the registry:

```bash
cargo run -p kuluu -- install link hxi                  # auto-detect
cargo run -p kuluu -- install link hxi "/path/to/..."   # or point it at a known install
cargo run -p kuluu -- install link hxi PATH --copy      # copy instead of symlink
cargo run -p kuluu -- install use hxi                   # make it the default
```

Don't have an install yet? `get` downloads Square Enix's **official** client
installer from the public PlayOnline CDN and unpacks it natively (opt-in and
confirmation-gated). The `ffxi-install` crate reads the RAR volumes, MSIs and
cabinets itself, decoding each cabinet while the next volume downloads, then
patches the 2019 base image to the current version by speaking the PlayOnline
patch protocol itself: it asks `pc001.pol.com` for the manifest, fetches only
the files whose checksums differ (whole images or delta chains, as the viewer
would), verifies each one, and writes the manifest as `patch.cfg`. No Wine,
no installer GUI, no viewer, no account, on any platform:

```bash
cargo run -p kuluu -- install get                                   # asks: name (retail), region (us), then downloads, patches, makes it the default
cargo run -p kuluu -- install get --name retail-eu --region eu --yes  # the same, unattended
cargo run -p kuluu -- install update retail                         # re-patch later (--verify re-checks every file)
```

`get` reuses an install that already carries the name rather than downloading
over it, and `update` refuses a known non-retail build (a private server's
pinned client, which retail patches would break). HorizonXI and other flavors
must be obtained through their own launchers.

For one run, point the client anywhere:

```bash
export FFXI_DAT_PATH="/path/to/.../SquareEnix/FINAL FANTASY XI"
```

### Client versions

Retail keeps changing its DAT formats (the September 2026 update, for one,
grew every item block from 0xC00 to 0x1400 bytes), and private servers pin
older clients. **The latest retail client is the primary target**; other
generations stay usable through the same mechanism, not through parallel code
paths. Kuluu identifies an install at startup (`ffxi_dat::ClientProfile`: the
FFXiMain.dll hash against `KNOWN_CLIENTS`, plus per-format probes such as the
item block layout) and logs it. Parsers that differ between generations
dispatch on those probed layouts, so an unmeasured build still gets the right
decoder or fails closed instead of reading garbage.

To keep more than one client around, register each under a name. One pointer
picks the active one: the `default` file that `kuluu install use NAME` writes,
which is what the launcher, `kuluu play`, tests and examples all load.
`FFXI_DAT_PATH` wins over it for that one run, and `which` says which applied:

```bash
cargo run -p kuluu -- install link retail "/path/to/PlayOnline/SquareEnix/FINAL FANTASY XI"
cargo run -p kuluu -- install list             # every install kuluu can see, with its client profile
cargo run -p kuluu -- install get              # download, patch and select the official client
cargo run -p kuluu -- install use retail       # the default
cargo run -p kuluu -- install which            # what will load, and why
cargo run -p kuluu -- install path hxi         # the DAT root, for scripts and agents
FFXI_DAT_PATH="$(cargo run -q -p kuluu -- install path hxi)" cargo run -p kuluu -- play   # one-off
cargo run -p ffxi-dat --example dat-client-profile -- "/path/to/FINAL FANTASY XI"
```

When you measure a new build, add its row to `KNOWN_CLIENTS` and cite that
row's name (not a date) next to any offset or constant verified on it.

The build-time vendor pins have generations too, and they are not the
client's. `vendor/server` (LandSandBoat, pinned 2026-09-11) declares
`CLIENT_VER = '30260904_1'` in `settings/default/login.lua` with
`VER_LOCK = 2`, so a stock server at that pin admits `retail-2026-09`
(`30260904_1`) exactly and refuses `horizonxi-2023` (`30230905_0`) unless the
lock is off; a pin bump moves it again. The same pin's zone text ids
(`scripts/zones/*/IDs.lua`) are synced to that client, so `retail-2026-09`
reads them as identity DAT indexes and `horizonxi-2023` through the landmark
reconciliation in `kuluu-session`.
`vendor/POLUtils`' `ROMFileMappings.xml` (pinned 2020-07-19; last edited
2018-08-18 for the Unity dialog tables, before that the 2015-11 Reisenjima
update) keys on absolute file ids up to 86528. Those ids still resolve on
both installs, but the old zone associations and map counts can be wrong.
Dialog DAT ids now follow the client's zone formula through VTABLE/FTABLE;
map selection uses the installed DLL's zone-map records. Item and key-item
autotranslate names come from the installed DATs, with the LSB dictionary
as a fallback when a name is unavailable. `vendor/AltanaListener`'s `track_names.json`
(v1.0.4, 2026-03-11) is a hand-curated 223-track name list, not a
client-derived table, so it has no build to match; the repository is archived
and the pin stays frozen.

### Steam Deck

The Deck runs the plain x86_64 Linux binary. Launch it from Game Mode, not
Desktop mode: Steam keeps its desktop controller layout active for anything
started outside Game Mode, so the d-pad and left stick arrive as arrow keys
on top of the gamepad. `kuluu steam-shortcut` registers the binary as a
non-Steam shortcut named Kuluu (with `play` as its launch options) so Game
Mode can start it under its own controller layout. Run it once from Desktop
mode with Steam fully quit; rerun it after moving the binary.

```bash
./kuluu steam-shortcut install                  # add or update the Kuluu shortcut (Steam must be closed)
./kuluu steam-shortcut install --layout deck.vdf  # also install a Steam Input layout for it
./kuluu steam-shortcut install --live           # hand the path to a running Steam instead (no rename/layout)
./kuluu steam-shortcut status                   # which Steam install and account, and whether the entry matches
./kuluu steam-shortcut remove
```

Without a layout file, pick the Gamepad template in the shortcut's
controller settings the first time you launch. In-game the pad follows
retail's Pattern E: A confirm, B cancel, X main menu, Y active window, LB
autorun, L3 heal/lock, R3 first person, d-pad targets in the field and moves
the cursor in menus, left stick moves, right stick is the camera.

## AI-generated code

Kuluu is, to a first approximation, **written by AI coding agents.** The large
majority of the code in this repository was generated by LLM agents (primarily
[Claude Code][cc]) under human direction, and development continues that way.
We'd rather state that plainly than have you infer it.

What that means for you:

- **Review it like any unfamiliar code.** Read it, run it, and don't assume
  correctness just because it compiles. AI-written code can be confidently
  wrong, and FFXI's wire protocol and coordinate math are easy to get subtly
  wrong even by hand.
- **Guardrails, not guarantees.** The most correctness-sensitive surface — the
  FFXI / LandSandBoat protocol boundary — is audited against the authoritative
  upstream by dedicated review agents and pinned with tests, and every push runs
  the same fmt + clippy + test gate as CI (see [Setup](#setup--first-build)).
  That catches a lot; it does not make the code independently audited.
- **No warranty.** Per the GPL-3.0 license, this software comes with none.
  Don't run it anywhere it matters without your own review.
- **Contributions are welcome on the same terms** — human-written or
  AI-assisted, open a PR and we'll review it.

[cc]: https://claude.com/claude-code

## Roadmap

[![open issues](https://img.shields.io/github/issues/jondwillis/kuluu-ffxi)](https://github.com/jondwillis/kuluu-ffxi/issues)
[![good first issues](https://img.shields.io/github/issues/jondwillis/kuluu-ffxi/good%20first%20issue?label=good%20first%20issue&color=7057ff)](https://github.com/jondwillis/kuluu-ffxi/issues?q=is%3Aopen+label%3A%22good+first+issue%22)

Progress is tracked honestly against retail in [beads](.beads/) — a git-backed
issue tracker checked into the repo (`.beads/issues.jsonl`). Parity work carries
the `roadmap` label, plus `vanilla`/`enhanced` and an area label
(`hud`, `combat-action`, …); issue status (`open` / `in_progress` / `closed`) is
the source of truth for what's done. Beads-backed [GitHub issues](https://github.com/jondwillis/kuluu-ffxi/issues)
are generated projections of that backlog, published by
[`scripts/beads-github-publish.py`](scripts/beads-github-publish.py) — so the
live counts are not a hand-kept promise. Pick an
[open issue](https://github.com/jondwillis/kuluu-ffxi/issues) and open a PR.

## Contributing

You can file or link an ordinary [GitHub issue](https://github.com/jondwillis/kuluu-ffxi/issues),
comment on an existing issue, or submit a fork PR.
[Creating an issue requires read access when issues are enabled](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/creating-an-issue);
contributors do not need Beads or permission to manage repository labels/issues.
A maintainer records accepted work in the durable tracker and supplies any
missing PR-linked issue projection. Generated issues retain discussion in
comments; their title/body is maintained from Beads. The
[sync skill](.agents/skills/beads-github-sync/SKILL.md) documents the maintainer's
scoped publication process.

Pick an open [issue](https://github.com/jondwillis/kuluu-ffxi/issues) and open a
PR. The backlog lives in beads (`bd ready` in a clone, or browse the mirrored
GitHub issues); a feature that has no equivalent in the official FFXI client is
**Enhanced / addon** and must be gated behind a feature flag.

Remaining vanilla menu / target-interaction gaps are tracked as `hud`- and
`world-interaction`-labelled beads (`bd list --label=hud`); the retail behavior
they're measured against is recorded under
[`.agents/skills/retail-observe/references/`](.agents/skills/retail-observe/references/).

For protocol questions, `play --headless` emits a JSON event stream that's easy
to inspect. For rendering work, the default `play` GUI window is the fast
iteration loop. New contributors: say hi in [Discord](https://discord.gg/5c8NK46SuD).

## Reference material (`research/`)

When re-implementing a feature it helps to read how other community clients
behave. Those upstreams live under `research/` as **read-only references** —
never redistributed by this repo (gitignored or submodule pointers). Study the
behavior and re-express it in our own code; don't copy source in. The most
useful one is [XIM](https://xim.pages.dev/), a from-scratch browser FFXI client
(GPL-3). See [`research/README.md`](research/README.md) for the full list and
the reference-only policy.

## Project artwork

The Southern Watcher mark combines a curled green tail, an amber lantern,
and three stars. It is original AI-generated artwork inspired by FFXI's
Tonberry lantern and constellation lore, not an official Kuluu crest or an
extracted game asset. The star arrangement is an interpretation.

The transparent source is [`kuluu-master.png`](kuluu/assets/branding/kuluu-master.png).
Its generation prompt and provenance are in [`generation.json`](kuluu/assets/branding/generation.json).
On macOS, regenerate the PNG sizes, Windows ICO, macOS ICNS, and browser
copies with `bash scripts/export-icons.sh` (requires `sips` and Python 3).
Generated files are committed so builds do not need image tools.

### Social artwork

| Asset | Size |
| --- | --- |
| [GitHub preview](kuluu/assets/branding/social/github-preview.png) | 1280 × 640 |
| [Discord avatar](kuluu/assets/branding/social/discord-avatar.png) | 512 × 512, transparent |
| [Discord server banner](kuluu/assets/branding/social/discord-banner.png) | 960 × 540 |
| [Discord invite splash](kuluu/assets/branding/social/discord-invite-splash.png) | 1920 × 1080 |

The banner and splash share a lantern-lit jungle sanctuary;
their [generation prompts](kuluu/assets/branding/social/generation.json) are included.

### Application icons

Linux release archives include `install-local.sh`; run it after extracting
to install the binary and desktop icon under `~/.local` (Python 3 required).
macOS archives include `Kuluu.app`, which can be moved to Applications, plus
the standalone command-line binary. Windows executables embed the icon.
Native builds open the launcher when started without arguments.
`kuluu steam-shortcut install` installs the default Steam shortcut icon
while preserving a custom icon; close Steam before running it.

The browser viewer uses the same mark for its favicon and web manifest.
Android and iOS native packages do not exist yet; their adaptive/layered
icons should derive from this master with platform-specific backgrounds
and safe-area padding.

## License & legal

Kuluu is licensed under **GPL-3.0-or-later** (see [LICENSE](LICENSE)) — the same
copyleft as the upstreams it derives compile-time data from (LandSandBoat, XIM).
[LEGAL.md](LEGAL.md) covers the no-asset-redistribution policy, trademark
disclaimer, and per-source attribution.

[lsb]: https://github.com/LandSandBoat/server
