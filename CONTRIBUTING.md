# Contributing

Thanks for looking. This is the short path from a clone to a merged pull
request. [AGENTS.md](AGENTS.md) is the in-depth orientation: architecture,
conventions and the reasoning behind the gates. It is written for coding
agents and humans alike, so read it once before touching anything
cross-cutting.

New here? Say hi in [Discord](https://discord.gg/5c8NK46SuD).

## Build

Nightly Rust is required; `rust-toolchain.toml` pins the date. The dev
profile uses the Cranelift backend, so a stable cargo errors out.

```bash
git clone https://github.com/jondwillis/kuluu-ffxi && cd kuluu-ffxi
git submodule update --init --depth 1 vendor/server vendor/POLUtils vendor/AltanaListener
cargo build
cargo xtask install-hooks          # once per clone: pre-commit and pre-push gates
```

The submodules are build-time only; [vendor/README.md](vendor/README.md)
lists what each one feeds and the shallow-clone caveat. Running the client
also needs a retail install, which the README's
[Game files](README.md#game-files) section covers.

Everything compiles under one feature set, `--features native-window`. Match
it for ad-hoc runs so artifacts are reused across the check stages:

```bash
cargo run -p kuluu -- play                                   # native window
cargo run -p kuluu --no-default-features -- play --headless  # JSON event stream, no Bevy
cargo test -p ffxi-proto framing::tests::roundtrip --features native-window
```

Credentials come from `FFXI_USER`, `FFXI_PASS`, `FFXI_CHAR` and `FFXI_SERVER`;
the launcher prompts for any that are unset.

## Checks

`scripts/checks.sh` is the single source of truth for check commands. The
hooks and CI call it with different stage lists, so a green hook is not a
green CI:

```bash
scripts/checks.sh harness comments literals fmt contracts wasm install clippy # pre-push
scripts/checks.sh harness comments literals fmt clippy test enhanced wasm     # the CI gate
cargo fmt --all
```

`PREPUSH_FAST=1 git push` runs only fmt and the state contracts;
`git push --no-verify` skips the hook for one push. Tests that need a live
server self-skip when none is reachable. Two rules the gates enforce that
surprise newcomers:

- **No magic numbers, no re-typed constants.** A meaningful literal gets a
  named `const`; a value LSB or POLUtils already defines is scraped at build
  time, never copied. `checks.sh literals` fails a line that re-types a value
  some constant already names.
- **No narrative comments.** Keep a comment only for a WHY the code cannot
  encode, a citation to a vendor or research source, or a `SAFETY` note.
  `checks.sh comments` fails on bead ids, line-number citations and unscoped
  binary addresses. The
  [comment-discipline skill](.agents/skills/comment-discipline/SKILL.md) has
  the full rule.

## Backlog

Work is tracked in [beads](https://github.com/gastownhall/beads), a
git-backed issue tracker. `.beads/issues.jsonl` is the export that crosses
into git; review it like code. GitHub Issues are a generated projection of
it, not a second source of truth.

```bash
bd ready                # unblocked work
bd show <id>
bd update <id> --claim
bd close <id>
```

Parity work carries the `roadmap` label plus `vanilla` or `enhanced` and an
area label such as `hud` or `combat-action`. Two rules shape every change:

- **Vanilla parity is the default.** Before changing client behavior, ground
  the rule in retail evidence with the
  [retail-grounding skill](.agents/skills/retail-grounding/SKILL.md). Dated
  retail observations live under
  [`.agents/skills/retail-observe/references/`](.agents/skills/retail-observe/references/).
- **Anything with no retail equivalent is Enhanced.** It carries the
  `enhanced` label, lives on the `kuluu-*` side, and stays behind an opt-in
  gate so it is never on in a default build.

## Where things live

- `ffxi-*` crates are domain truth about the game, its file formats and the
  LSB protocol; `kuluu-*` crates are this client's own machinery. The wire
  boundary between them is the most correctness-sensitive surface in the
  tree. Source that crosses it cites the upstream file, and non-trivial edits
  there should go through the `protocol-conformance-reviewer` and
  `lsb-invariant-prober` review agents described in AGENTS.md.
- `vendor/` is read by the compiler. `research/` is read by people:
  read-only references, studied and re-expressed, never copied in.
  [research/README.md](research/README.md) ranks which to trust for what.
- Prose has a home that keeps it honest: open work is a bead, retail behavior
  is a dated observation record, a recurring task is a skill under
  `.agents/skills/`, and a fixed bug is a commit message. Please do not add
  free-floating markdown notes.

## Pull requests

Pick an open [issue](https://github.com/jondwillis/kuluu-ffxi/issues) or a
`bd ready` bead, keep the commit history coherent, and open a PR. Most of the
existing code was written by AI coding agents under human direction, and
contributions are welcome on the same terms, human-written or AI-assisted.
Either way, expect the review to read the diff against the LSB source and
the retail evidence rather than trusting that it compiles.

## Optional DLSS and Neural Uplift builds

DLSS Super Resolution is an opt-in enhancement, absent from standard builds
and off in Graphics settings until selected. It requires an NVIDIA RTX GPU
and Vulkan on x86_64 Windows or Linux; macOS and browser builds do not
support it.

The optional `vendor/DLSS` submodule pins NVIDIA's SDK v310.5.3, matching
[`dlss_wgpu` 4.0.0](https://github.com/bevyengine/dlss_wgpu/tree/323ba14a80b26718093ca4bebe9f6c1b6fef5e57).
Normal submodule setup skips it. Install the Vulkan SDK and libclang, set
`VULKAN_SDK` to the SDK root, and set `LIBCLANG_PATH` if automatic discovery
fails. Then:

```bash
cargo xtask dlss check   # verifies SDK files and Vulkan headers, no download or build
cargo xtask dlss build   # initializes the pinned SDK and builds the release client with `dlss`
```

The build stages the matching SR runtime, license and programming guide
(including upstream attribution notices) beside the executable under
`target/<host-target>/release/` or `CARGO_TARGET_DIR`. `DLSS_SDK` overrides
the pinned SDK directory. The SDK and its runtime remain subject to
[NVIDIA's license](https://github.com/NVIDIA/DLSS/blob/v310.5.3/LICENSE.txt).
An installed SDK never changes the normal check gate; include SR explicitly
with `KULUU_CHECK_DLSS=1 scripts/checks.sh clippy test build`.

In the client, enable DLSS in Graphics and choose its quality in
`DLSS Config`. It owns anti-aliasing and render resolution while active.

**Neural Uplift** is a separate experimental Windows-only enhancement gated by
`enhanced-neural-uplift`, off by default. Its runtime is not in the SDK and
not downloaded by the build helper. Stage SR as above, then build the client
and forwarder into the same directory:

```powershell
if (-not $env:DLSS_SDK) { $env:DLSS_SDK = (Resolve-Path vendor/DLSS).Path }
cargo build -p kuluu -p kuluu-ngx-fwd --locked --release --target x86_64-pc-windows-msvc --features native-window,enhanced-neural-uplift
Copy-Item target/x86_64-pc-windows-msvc/release/kuluu_ngx_fwd.dll target/x86_64-pc-windows-msvc/release/nvngx.dll_kuluu.dll
```

Supply `nvngx_dlssnr.dll` beside the executable and enable `Neural Uplift`
in `DLSS Config` with DLSS active. The forwarder's staged filename must
remain `nvngx.dll_kuluu.dll`. Camera-motion quality still needs validation;
the current NR path supplies zero motion vectors.
