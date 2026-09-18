# Ordinary PC model arrival: resource gate and opacity fade

Date: 2026-09-14. Initial method: original-client binary inspection with XIClient
as symbol/call-chain guidance. At that stage there was no live visual observation: `observe.sh status` reports
Windows 11 suspended; `observe.sh capture` returned no window. The VM was left
suspended and no game inputs were sent.

## Finding

The ordinary skeletal actor uses a model opacity fade, not an opaque placeholder
shape morph. Its fade starts at zero and advances only after its resource-ready
predicate succeeds. Each update adds `CheckTick() / 32`, capped at one.
`CheckTick()` uses elapsed time in nominal 60 Hz units (minimum one), making the
normal fade about **32/60 seconds (533 ms)**, or 16 updates at 30 FPS. Timer
smoothing, integer quantization and stalls mean this is a nominal duration, not
a measured wall-clock guarantee.

With no model part list, the actor draw marks itself hidden and returns. The
ordinary path provides no evidence for a loading orb, column or geometric
placeholder. The same loaded model is drawn with increasing alpha; a zero alpha
also suppresses model drawing. This supports hiding the loading placeholder and
fading the ready model for Kuluu's vanilla arrival policy. It is not a claim that
every special spawn, teleport, monster pop scheduler or cutscene uses this rule.

## Authoritative binary evidence

Build rows are from `ffxi-dat/src/client_profile.rs`; all addresses below are
**RVAs** relative to image base `0x10000000`.

`retail-2026-09` input:
`vendor/game-files/targets/retail/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`,
SHA-256 `f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
Its unpacked `.text` starts at RVA `0x1000`, SHA-256
`b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.

| Retail RVA | Independently checked behavior |
| --- | --- |
| `0xC5767` | Telemetry-backed skeletal constructor installs vtable, links itself at telemetry + `0xA0`, and calls Init at `0xC5D60`; its special `spop` color branch excludes actor type zero (PC). |
| `0xC5D60`, `0xC6110` | Init clears `ebx`, then writes zero into actor + `0x59C` (fade). |
| `0xC7676` | Calls readiness virtual at vtable + `0x1E4`; unsuccessful readiness bypasses fade advancement. Mounted cases also require the linked actor's readiness. |
| `0xC76C0`–`0xC76F0` | Reads fade, calls CheckTick, multiplies by `0.03125`, adds prior fade, caps at `1.0`, writes it back. Constants read from RVA `0x32B088` and `0x32961C`. |
| `0x14CF0` | CheckTick returns max(game + `0x28`, `1.0`). |
| `0x12B35`, `0x12C98` | Timer calculation uses `60.0 / measured_FPS`; final effective FPS is `60.0 / game_delta`. The `60.0` constant is RVA `0x329CE8`. Intermediate history averages four samples, truncates, and clamps to configured FPS divisor and 20. |
| `0xC7DED` | Fade multiplied by distance alpha determines normal vs depth-sorted actor render path. |
| `0xCBDE0`, `0xCBE1D`–`0xCBE58` | Draw gets actual model part list. Empty list marks hidden, runs camera bookkeeping, and returns. |
| `0xCBF27`–`0xCBF65` | Draw multiplies appearance alpha by fade, caps it, converts `128 * alpha` to the packed alpha byte, and bypasses model body drawing for near-zero alpha. |
| `0x331124`, `0xA47E0`, `0xD12D0` | Readiness vtable entry resolves through a jump to the resource-list predicate. It requires no pending read request, each resource reference ready, a required primary model reference, and optional supplementary resource readiness. |
| `0x71060` | Resource readiness requires a managed, non-null resource with zero outstanding dependency count. This is resource-level readiness; a separate modern GPU-upload fence was not established. |

The same fade arithmetic was independently found in `horizonxi-2023`:
ready gate RVA `0xC65E6`, fade accumulation `0xC6630`–`0xC6660`,
CheckTick `0x14340`, alpha multiplication `0xCAE97`. Its constant at
RVA `0x326F10` is also `0.03125`. Unpacked `.text` SHA-256:
`f6b48296b3f9e82a5ed73004e513cc69ded72bb407fb42872e5c9ff63725a527`.

Local reproducible dumps (not committed):

- `artifacts/verify/retail-pc-arrival-2026-09-14/binary-evidence.txt`
- `artifacts/verify/retail-pc-arrival-2026-09-14/vm-status.txt`
- Original unpacked blobs: `artifacts/verify/version-provenance/{retail-2026-09,horizonxi-2023}.text.bin`

## Community reconstruction and remaining limits

XIClient pin `aba6c816d25139f0a4fdb60d05bcec8af5f4d928` supplies symbol names:

- `research/XIClient/src/XIClient/source/World/Actor/SkeletalMeshActor.cpp`:
  constructors, `Init`, `UpdateModelFadeIn`, `IsReadCompleteResList`,
  `SelectRenderPath`, `Draw`.
- `research/XIClient/src/XIClient/source/Game/GameManager.cpp`:
  `UpdateTimers`, `CheckTick`.
- `research/XIClient/src/XIClient/source/Resource/ResourceManager.cpp`:
  `IsResourceReady`.

No live sequence was captured during the binary investigation. Binary verification establishes
the ordinary fade computation and readiness gate, but does not establish visible
appearance under the user's installed addons, all equipment swaps, unusual
network timing, or special arrival effects. XIM was not used as the authority.

## Grouped opacity compositing

Offscreen composition is independently confirmed in
`retail-2026-09` FFXiMain.dll, SHA-256
`f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
All addresses below are **RVAs**, image base `0x10000000`.

The concrete scope is one skeletal actor's **ModelInstance and its linked model
part instances**. It is not proof that independently linked actors (a mount,
other special attachment actors, or spell effects) join the same composite.

| Retail RVA | Binary-confirmed operation |
| --- | --- |
| `0x2B6E0` | ModelInstance draw entry, called by skeletal actor drawing at `0xCCDC1` and `0xCCDF5`. |
| `0x2B938`–`0x2B950` | Passes fade-derived packed alpha to setter `0x1D390`, then calls `0x1D240` on the model's blender at model + `0x74`. Setter stores color at global RVA `0x456D4C` and optional-effect flag at `0x456D48`. |
| `0x1D240`–`0x1D267` | Target preparation bypasses when the texture pointer is null, or when the optional-effect flag is false and alpha byte is exactly `0x80` (normal full opacity). |
| `0x1D2C0`–`0x1D309` | Uses blender + `0x08` texture, associated depth/stencil data, calls target selection `0x9E20`, clears target to zero, and sets the active flag. |
| `0x9F20`–`0x9F61` | Target-selection helper obtains the texture's level-zero surface and calls IDirect3DDevice8::SetRenderTarget (vtable + `0x7C`). This independently identifies an offscreen render target, rather than only inferring it from XIClient names. |
| `0x2BF6B`, `0x2C360`–`0x2C392` | After target setup, calls the part-list drawing routine. It walks linked model part instances and invokes each part's drawing before the composite. |
| `0x2C184`–`0x2C1D6` | Calls cleanup `0x1D350`, then the composite `0x1D3B0`, using the same blender instance. Cleanup returns to the preceding target through `0xA1D0`. |
| `0x1D3B0`–`0x1D3E7` | Composite also bypasses a missing target texture, ordinary full opacity, or inactive target flag. |
| `0x1D3ED`–`0x1D52A` | Builds four screen-space vertices from viewport width/height, UVs zero to one, and the stored fade-derived color. |
| `0x1D553`–`0x1D5B9` | Disables depth writes/testing; enables alpha blending; sets source blend to SRCALPHA and destination blend to INVSRCALPHA. Actual D3D8 render-state/value pairs are `(14,0)`, `(7,0)`, `(27,1)`, `(19,5)`, `(20,6)`. |
| `0x1D754`–`0x1D776` | Binds blender + `0x08` texture, then draws the screen-space quad as a two-primitive TRIANGLESTRIP. The draw helper calls IDirect3DDevice8::DrawPrimitiveUP at vtable + `0x120`. Stereo-related branches can perform additional draws. |
| `0x1D77B`–`0x1D7F7` | Restores blend/depth states and clears the active flag. |

D3D8 method slots and numeric enums were cross-checked against the local SDK
headers `research/XIClient/third_party/d3d8/include/d3d8.h` (`IDirect3DDevice8`)
and `d3d8types.h` (`D3DRENDERSTATETYPE`, `D3DBLEND`). The method-order calculation
is included in the dump. XIClient supplied names and search locations; the
call order, target selection, full-alpha bypass, texture bind, quad geometry,
and blending configuration were read from the actual binary.

**Conclusion:** ordinary partial opacity is applied through a grouped model
render target and its final alpha composite. Applying arrival opacity separately
to each Kuluu mesh is not the same rendering operation and can expose overlapping
armor/body surfaces differently. The existence of that mismatch is established;
its visible severity on particular models has not been measured. A practical
per-mesh fade can be shipped with that scope stated, while exact composite parity
remains open. This investigation does not claim a complete audit of intermediate
material alpha rules, stencil behavior, stereo rendering, target allocation, or
GPU cost.

Local evidence: `artifacts/verify/retail-pc-arrival-2026-09-14/composite-binary-evidence.txt`.
No live VM observation was performed for this follow-up.


## User-supplied gameplay recording

Later on 2026-09-14, the user supplied `Screen Recording 2026-09-14 at
4.47.12 PM.mov`. This adds direct visual evidence to the binary investigation;
no VM input or new login was needed. The clip shows Oldman in Lower Jeuno,
with the HorizonXI launcher/connection output and Ashita visible. The clip alone
does not identify the DLL hash, frame limiter, or complete addon configuration.

The source is 3456 x 2244, approximately 4.383 seconds, with variable capture
cadence (average about 56.5 frames/second). Local preserved copy:
`artifacts/retail/pc-arrival-user-2026-09-14/retail-arrival.mov`, SHA-256
`f56ce276c2212626501e4bbab8e6dd082e88a29d9895a3dad7896d02acb53210`.
The adjacent `retail-arrival.json` records the original path and ffprobe metadata.

Observed in the first 1.6 seconds, sampled at 100 ms intervals:

- Cylera's nameplate is visible before the character body is visible.
- Around 0.2-0.3 seconds into the clip, the body becomes faintly visible;
  opacity increases until it looks solid around 0.7-0.8 seconds.
- The character keeps normal proportions and position throughout the reveal.
  The background shows through the fading silhouette. There is no loading
  column, stretched placeholder, bright outline, or particle burst.
- A second nearby character appears later, with its nameplate also preceding
  the visible body. Actor arrivals are staggered in this recording; this does
  not establish the client's loading scheduler or per-frame work budget.

These approximate visual bounds are consistent with the binary-derived nominal
533 ms fade; they are not an independent exact timing measurement. The clip
confirms the intended visible effect, but does not uniquely prove an offscreen
compositor or quantify Kuluu's per-mesh overlap error. The binary evidence above
establishes the compositor. Exact parity remains tracked by `kuluu-ph59`.

Inspection artifacts (local only): `contact-sheet.png` covers the full clip at
5 samples/second; `arrival-detail.png` is a 4 x 4 crop sequence at 10
samples/second, read left-to-right then top-to-bottom. Its extraction filter is
`fps=10,crop=520:440:1450:890,tile=4x4`, limited to the first 1.6 seconds.
