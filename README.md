<p align="center">
  <img src="kuluu/assets/branding/social/github-preview.png" alt="Kuluu, an open-source FINAL FANTASY XI client. A moss-green curled tail bears three stars and an amber lantern." width="960">
</p>

<p align="center">
  <em>A faithful, open-source FINAL FANTASY XI client, rebuilt in Rust + Bevy
  for modern hardware.</em>
</p>

<p align="center">
  <a href="https://github.com/jondwillis/kuluu-ffxi/releases"><img alt="Latest release" src="https://img.shields.io/github/v/release/jondwillis/kuluu-ffxi?logo=GitHub&label=download"></a>
  <a href="https://discord.gg/5c8NK46SuD"><img alt="Discord" src="https://img.shields.io/badge/discord-join-5865F2.svg?logo=discord&logoColor=white"></a>
  <a href="https://github.com/sponsors/jondwillis"><img alt="Sponsor" src="https://img.shields.io/badge/sponsor-%E2%99%A5-ea4aaa.svg?logo=githubsponsors&logoColor=white"></a>
</p>
<p align="center">
  <a href="https://github.com/jondwillis/kuluu-ffxi/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/jondwillis/kuluu-ffxi/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/jondwillis/kuluu-ffxi/issues"><img alt="Open issues" src="https://img.shields.io/github/issues/jondwillis/kuluu-ffxi"></a>
  <a href="LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg"></a>
</p>

<p align="center">
  <strong>Play:</strong>
  <a href="#playing">Get started</a> &middot;
  <a href="https://github.com/jondwillis/kuluu-ffxi/releases">Downloads</a> &middot;
  <a href="https://discord.gg/5c8NK46SuD">Discord</a>
  <br>
  <strong>Hack:</strong>
  <a href="#building-from-source">Build from source</a> &middot;
  <a href="CONTRIBUTING.md">Contributing</a> &middot;
  <a href="#roadmap">Roadmap</a>
</p>

Kuluu is a fan-community game-preservation project: a cross-platform,
open-source client for the FINAL FANTASY XI network protocol. It connects to
community-run private servers such as LandSandBoat and Phoenix, not to the
official service.

FINAL FANTASY XI is a Square Enix property. **Kuluu has no affiliation with,
and no endorsement from, Square Enix, and ships no game assets.** If you enjoy
FFXI, please support the official service. See [LEGAL.md](LEGAL.md).

<p align="center">
  <video src="https://github.com/user-attachments/assets/8bc7375b-262c-4074-965e-f073b342430a" controls width="640"></video>
</p>

## Playing

Grab the archive for your platform from
[Releases](https://github.com/jondwillis/kuluu-ffxi/releases) and unpack it.

| Platform | Build | First launch |
| --- | --- | --- |
| Windows | x86_64 or ARM64 | Run `kuluu.exe` |
| macOS | Apple Silicon | Move `Kuluu.app` to Applications |
| Linux | x86_64 or ARM64 | Run `./install-local.sh` (needs Python 3) |
| Steam Deck | x86_64 Linux | See the Steam Deck steps below |

Starting Kuluu with no arguments opens the launcher, which asks for a server,
account and character and lists the account's characters by name. Stuck?
Ask in [Discord](https://discord.gg/5c8NK46SuD).

### Game files

Kuluu reads geometry, textures, audio and animation from a retail FFXI
install at runtime and never ships them. The launcher's **Get the official
client** button downloads Square Enix's free client from the public PlayOnline
CDN, patches it to the current version and selects it, asking before each
step. The same from a terminal:

```bash
kuluu install get            # download, patch and make it the default
kuluu install link hxi       # detect and register an existing install
kuluu install list           # every install Kuluu can see, with its client version
kuluu install use hxi        # pick the default
kuluu install which          # what will load, and why
```

The official client is free to download; an account is only needed to play
the official service. HorizonXI and other private-server flavors must come
from their own launchers ([horizonxi.com](https://horizonxi.com) on Windows,
[Lutris](https://lutris.net/games/horizonxi/) on Linux), then `link` finds
them. Installs live under your user data directory
(`~/Library/Application Support/kuluu/installs/` on macOS,
`~/.local/share/kuluu/installs/` on Linux), and `FFXI_DAT_PATH` overrides the
default for one run.

Retail keeps changing its DAT formats and private servers pin older clients.
The latest retail client is the primary target. Kuluu identifies known client
builds at startup; `install list` shows the version it recognizes. Support for
older builds depends on the formats they use.

<details>
<summary>Steam Deck</summary>

The Deck uses the x86_64 Linux build. With Steam fully closed, run this once
from Desktop mode, then launch Kuluu from Game Mode:

```bash
./kuluu steam-shortcut install
```

Pick the Gamepad template in Steam's controller settings. Desktop mode can
send keyboard inputs alongside gamepad inputs, so use Game Mode to play.
Rerun the command if you move the binary.

</details>

## Why Kuluu

- **Vanilla parity is the base.** The aim is close to 1:1 with the official
  client in default mode: same menus, same compass, same combat feel.
  Anything with no retail equivalent is an **Enhanced** feature, opt-in and
  never on by default.
- **Modernization layers on top.** Bevy and wgpu replace the legacy D3D8
  stack; the aim is to support extensions without patching the game client.
- **No asset redistribution.** Kuluu requires a user-provided retail install.
  Tables translated from LandSandBoat, POLUtils and similar sources are baked
  in as derived compile-time constants under the upstream license, never as
  game content. See [LEGAL.md](LEGAL.md).

## Building from source

Nightly Rust is required; `rust-toolchain.toml` pins it.

```bash
git clone https://github.com/jondwillis/kuluu-ffxi && cd kuluu-ffxi
git submodule update --init --depth 1 vendor/server vendor/POLUtils vendor/AltanaListener
cargo run -p kuluu -- play
```

The submodules supply build-time data; they are not needed to run a release.
For hooks, checks, architecture and optional DLSS builds, see
[CONTRIBUTING.md](CONTRIBUTING.md).

## Roadmap

[![open issues](https://img.shields.io/github/issues/jondwillis/kuluu-ffxi)](https://github.com/jondwillis/kuluu-ffxi/issues)
[![good first issues](https://img.shields.io/github/issues/jondwillis/kuluu-ffxi/good%20first%20issue?label=good%20first%20issue&color=7057ff)](https://github.com/jondwillis/kuluu-ffxi/issues?q=is%3Aopen+label%3A%22good+first+issue%22)

Follow the [roadmap issues](https://github.com/jondwillis/kuluu-ffxi/issues?q=is%3Aopen+label%3Aroadmap)
for remaining parity work. This is a work in progress; the goal of matching
retail is not a claim that every feature works today. Bug reports and pull
requests are welcome. See [Contributing](CONTRIBUTING.md) or
[Support](SUPPORT.md) to get involved.

## AI-generated code

Kuluu is, to a first approximation, **written by AI coding agents**. The
large majority of the code was generated by LLM agents, primarily
[Claude Code](https://claude.com/claude-code), under human direction, and
development continues that way. We'd rather state that plainly than have you
infer it.

- **Review it like any unfamiliar code.** AI-written code can be confidently
  wrong, and FFXI's wire protocol and coordinate math are easy to get subtly
  wrong even by hand.
- **Checks have limits.** The FFXI / LandSandBoat protocol boundary
  is audited against the upstream source by dedicated review agents and
  pinned with tests. Local hooks and CI check different parts of the build;
  they do not make the code independently audited.
- **No warranty.** Per the GPL-3.0 license, this software comes with none.

Contributions are welcome on the same terms, human-written or AI-assisted.

## License

Kuluu is licensed under **GPL-3.0-or-later** (see [LICENSE](LICENSE)), the
same copyleft as the upstreams it derives compile-time data from.
[LEGAL.md](LEGAL.md) covers the no-asset-redistribution policy, trademark
disclaimer and per-source attribution.
