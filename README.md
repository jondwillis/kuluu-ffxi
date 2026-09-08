<h1 align="center">Kuluu</h1>

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
the build** — only cited in source comments for reference (`AltanaViewer`,
`XiEvents`, `XiPackets`, `XIClient`, `cexi-*`) — live under `research/`, not
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
commits them. The client reads them from `vendor/game-files/` by default
(gitignored), or wherever `FFXI_DAT_PATH` points.

Get an install one of these ways:

- **HorizonXI launcher (Windows):** install via <https://horizonxi.com>; its
  launcher downloads a full FFXI + Ashita tree.
- **Lutris (Linux):** <https://lutris.net/games/horizonxi/>. Files land under
  `~/Games/.../drive_c/.../SquareEnix/FINAL FANTASY XI/`.
- **Copy an existing install:** the PlayOnline tree from any retail/private-server
  install (`.../PlayOnline/SquareEnix/FINAL FANTASY XI/`).

Kuluu expects this layout (the parent of `SquareEnix/`):

```
vendor/game-files/
  SquareEnix/
    FINAL FANTASY XI/      <- FFXI_DAT_PATH points here
      VTABLE.DAT  FTABLE.DAT
      ROM/  ROM2/ … ROM9/
      sound/win/…
```

Then wire it up with the cross-platform helper, which detects an existing
install (HorizonXI / Lutris / Wine / CrossOver / PlayOnline), validates it, and
symlinks it into `vendor/game-files/`:

```bash
cargo xtask game                 # auto-detect
cargo xtask game "/path/to/..."  # or point it at a known install
cargo xtask game --copy          # copy instead of symlink
```

Don't have an install yet? The helper can also download Square Enix's **official**
client installer from the public PlayOnline CDN and launch it (opt-in and
confirmation-gated; runs under Wine on macOS/Linux). The download is free; a
registration code / subscription is needed to play on the official service:

```bash
cargo xtask game --download             # official US client; prompts first
cargo xtask game --download --region eu
```

Complete the installer GUI, then run `cargo xtask game` to wire it up. (This is
official-client only — HorizonXI and other flavors must be obtained through
their own launchers.)

Or do it by hand — drop/symlink your install at `vendor/game-files/`, or just
point the client at an existing copy:

```bash
export FFXI_DAT_PATH="/path/to/.../SquareEnix/FINAL FANTASY XI"
```

`FFXI_DAT_PATH` also overrides at runtime and can be set from the launcher's
settings UI, so you never have to move a large install to use it.

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
the source of truth for what's done. The [open issues](https://github.com/jondwillis/kuluu-ffxi/issues)
above are a generated projection of that backlog, published by
[`scripts/beads-github-publish.py`](scripts/beads-github-publish.py) — so the
live counts are not a hand-kept promise. Pick an
[open issue](https://github.com/jondwillis/kuluu-ffxi/issues) and open a PR.

## Contributing

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

## License & legal

Kuluu is licensed under **GPL-3.0-or-later** (see [LICENSE](LICENSE)) — the same
copyleft as the upstreams it derives compile-time data from (LandSandBoat, XIM).
[LEGAL.md](LEGAL.md) covers the no-asset-redistribution policy, trademark
disclaimer, and per-source attribution.

[lsb]: https://github.com/LandSandBoat/server
