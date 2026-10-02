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

The normal check stages use `--no-default-features --features native-window`.
Match those flags for ad-hoc tests to reuse their build artifacts:

```bash
cargo run -p kuluu -- play                                   # native window
cargo run -p kuluu --no-default-features -- play --headless  # JSON event stream, no Bevy
cargo test -p ffxi-proto framing::tests::roundtrip --no-default-features --features native-window
```

Credentials come from `FFXI_USER`, `FFXI_PASS`, `FFXI_CHAR` and `FFXI_SERVER`;
the launcher prompts for any that are unset.

## Checks

`scripts/checks.sh` is the single source of truth for check commands. The
hooks and CI call it with different stage lists, so a green hook is not a
green CI:

```bash
scripts/checks.sh harness readme comments literals fmt contracts wasm install clippy # pre-push
scripts/checks.sh harness readme comments literals fmt clippy test enhanced wasm     # the CI gate
cargo fmt --all
```

`PREPUSH_FAST=1 git push` keeps the harness, README, comment, literal, formatting and
state-contract checks but skips the heavier build checks;
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

You can file an ordinary [GitHub issue](https://github.com/jondwillis/kuluu-ffxi/issues),
comment on an existing issue, or submit a fork PR. You do not need Beads or
permission to manage repository issues. Maintainers record accepted work in
the durable tracker and supply missing PR-linked issue projections. Discussion
stays in GitHub comments; generated issue titles and bodies come from Beads.

For maintainers, work is tracked in [beads](https://github.com/gastownhall/beads), a
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
  LSB protocol; `kuluu-*` crates are this client's own machinery. The LSB
  protocol boundary is a critical correctness surface. Source
  that crosses it cites the upstream file, and non-trivial edits there
  should go through the `protocol-conformance-reviewer` and
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
Explain the problem, who benefits, and why the change belongs in Kuluu.
Use the PR template and link the relevant issue. Client-behavior changes need
retail grounding; protocol changes need LSB evidence. Visible changes need
inspected screenshots or video accessible from the PR. The
[review skill](.agents/skills/kuluu-review/SKILL.md) describes the evidence
requirements, and the [sync skill](.agents/skills/beads-github-sync/SKILL.md)
covers maintainer publication of linked tasks.

## README editorial policy

The root README is the front door for FFXI players and technically curious
contributors. A reader should quickly understand what Kuluu is, whether they
can try it, how to start, and where to get help or contribute.

A detail earns space when it answers one of those questions for most readers
or prevents a common first-launch failure. A merged feature does not
automatically earn a paragraph. Prefer updating an existing sentence or
linking to the right guide over adding a section.

- Keep the project purpose, compatibility limits, download and first-launch
  path, game-file requirement, short source-build path, roadmap link, AI-code
  disclosure and license visible.
- Put contributor setup, optional builds, checks and maintainer procedures in
  this guide. Keep dependency details in `vendor/README.md`, artwork provenance
  beside the artwork, and retail evidence in the observation references.
  Track unfinished work in issues and completed fixes in commit history.
- Leave out per-feature inventories, implementation tours, packet offsets,
  investigation diaries, CI progress, task IDs and repeated command reference.
  Links should take the reader to one maintained explanation.

Write like a fellow FFXI enthusiast who knows the code: friendly, direct and
specific. Use ordinary words, active verbs and contractions where natural.
Explain an unfamiliar term when the reader needs it. Keep technical detail
when it changes a decision or helps someone complete a step. Be candid about
limits and distinguish an aspiration from something tested today. Avoid sales
copy, grand claims, canned transitions and a heading for every paragraph.

For example, prefer “Run `kuluu install get` to download and patch the game
files” to “Our seamless asset acquisition pipeline streamlines onboarding.”
Prefer “Matching retail is the goal; see the open roadmap issues” to “Full
retail fidelity.” Do not add a feature announcement just because its PR landed.

Before submitting a README edit, read the whole page and explain in the PR
which reader question the addition answers. Remove duplication, check commands
against their implementation, and follow the links. The automated `readme`
stage caps the root page at 1,200 whitespace-separated words after removing
HTML tags and link destinations, and 35 nonblank fenced-code lines. It also
rejects internal task IDs and broken local Markdown/HTML links in the public
guides. These are backstops; a short paragraph can still be irrelevant or badly
written. Do not raise the limits merely to fit an addition.

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

## Website deployment

`site/index.html` is a static landing page. The Pages workflow stages it with
the branding images and deploys changes from `main`. Before its first run, a
repository administrator must choose **GitHub Actions** under **Settings >
Pages > Build and deployment > Source**. The workflow token can deploy an
enabled site; it cannot enable Pages itself. Keep that setup in repository
settings rather than adding an administrative token to the workflow.

To preview locally, copy `site/index.html` into a temporary directory with an
`assets/` subdirectory containing `kuluu-32.png`, `kuluu-256.png` from
`kuluu/assets/branding/png/` and `github-preview.png` from
`kuluu/assets/branding/social/`. Serve that directory over HTTP and inspect
both a narrow and a wide viewport before changing the page.
