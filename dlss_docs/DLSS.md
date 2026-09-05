# Kuluu — DLSS Super Resolution & Neural Uplift (DLSSNR)

Single complete doc for all DLSS work in Kuluu:

- **Part 1 — Current system**: what exists today, how to use / build / run it, known
  limitations, and the feature-gating map.
- **Part 2 — Neural Uplift research & implementation log**: the verified binary ground
  truth (reverse-engineered v310.8 runtime), the NR pipeline as built, official post-launch
  DLSS 5 knowledge, open risks, verification checklist, and the build history. Part 2 keeps
  its own §-numbering so internal cross-references (§2.9 item 11, §3.5, …) stay valid.

---

# Part 1 — Current system

NVIDIA DLSS SR support for the native viewer: upscaling + anti-aliasing in one
pass, driven from the same graphics menus as every other setting. OPT-IN via
the `dlss` kuluu feature (`cargo build -p kuluu --features dlss`): with it on,
the runtime plumbing compiles and the menu rows go live on capable hardware.
SDK-less environments (CI runners, release legs, Steam Deck docker) build with
`--no-default-features --features native-window`; there the plumbing is not
compiled in and the menu rows permanently read `N/A`.

Requires an RTX GPU on the Vulkan backend (Windows or Linux). Never available
on wasm.

## Using it

Two menu surfaces, same state underneath:

- **Anti-Aliasing cycler**: `DLSS` appears as one more slot (after TAA) while
  the runtime supports it. It is mutually exclusive with MSAA/TAA by
  construction — it IS the anti-aliasing.
- **DLSS row** (right under Anti-Aliasing): a plain On/Off mirror of the same
  state. Reads `N/A` and refuses to toggle while unsupported. Turning it off
  lands on AA `Off`; re-pick MSAA/TAA in the cycler if you want them back.
- **DLSS Config**: in-game it's the `DLSS Config` row near the bottom of the
  Graphics menu (opens a submenu); in the launcher it's the
  `> DLSS configuration` disclosure. Contents:
  - `DLSS Quality` — the live knob: Auto / DLAA / Quality / Balanced /
    Performance / Ultra Perf. Auto lets DLSS pick from the output resolution;
    DLAA is anti-aliasing only at native res.
  - `Neural Uplift` — master toggle for the NR pipeline (`graphics/dlss_nr.rs`).
    Live on dlss builds with an RTX GPU + `nvngx_dlssnr.dll` staged next to
    the exe; `N/A` otherwise. NR only evaluates while the AA mode is Dlss:
    cycling to MSAA/TAA stands it down entirely (the toggle persists as a
    setting, but nothing runs — see `GraphicsSettings::nr_active`).
  - `NR Intensity`, `NR Local Tone Strength`, `NR Structure Strength` — the
    addon's three knobs; live while supported.
  - `RR Preset`, `SR Preset`, `RR Responsivity`, `Sharpness` — inert
    placeholders, always `N/A` (see "Placeholders" below).
  - `Reset DLSS to defaults` — quality back to Auto, NR off + knob defaults.

Quality presets (Low/Medium/High/Ultra) never own DLSS: no preset turns it on,
and picking or cycling a preset does not turn it off or touch the tier. `Reset
to High` does turn it off (it is a full reset) but never un-detects support.

While DLSS is active:

- MSAA and TAA are forced off (the AA respawn owns this).
- The manual Render Scale row parks: it reads `DLSS` and refuses to cycle,
  and the off-screen composite path stands down, because DLSS owns internal
  resolution and upscaling. Both come back the moment DLSS is off.
- Changing the quality tier respawns the operator camera (a blink). That is
  deliberate: a fresh view entity is guaranteed to re-create the DLSS context
  at the new internal resolution. In-place tier mutation is a possible later
  optimization once a dlss build can be A/B tested on real hardware.

State: on/off rides `anti_aliasing` and the tier rides `dlss_quality` in
graphics.json. Capability (`dlss_supported`) is runtime-detected every launch
and never persisted, so a config written on an RTX box is a harmless no-op on
anything else — the AA row just reads `DLSS (N/A)` until you cycle away.

## Building with DLSS

Local one-shot: `.\build_cowland.bat` (repo root, gitignored) sets the three
SDK vars from `streamline/` when unset, builds kuluu + the forwarder crate with
the full local feature set (`dlss,debug-menu,enhanced-mob-hp-under,enhanced-job-display`),
syncs the exe to the repo root via `logs\sync_exe.ps1`, and stages every runtime
DLL next to both exes. Manual steps below for SDK-less or custom environments:

Build-time requirements (all from dlss_wgpu 4.0.0, which bevy's `dlss`
feature pulls in; its build.rs panics without the first two):

1. Clone the NVIDIA DLSS SDK, tag `v310.5.3`, and comply with its license:
   `git clone --branch v310.5.3 https://github.com/NVIDIA/DLSS` — this repo's
   dev checkout lives in `streamline/sdk`.
2. Set `DLSS_SDK` to the SDK root (dev machine: `<repo>\streamline\sdk`).
3. Install the Vulkan SDK and set `VULKAN_SDK` (the LunarG installer sets it
   up; this repo's dev checkout: `streamline/vulkan-sdk`).
4. Install clang (bindgen needs libclang; this repo's dev checkout:
   `LIBCLANG_PATH=<repo>\streamline\llvm\bin`).

Set the three variables once in your user environment and every local build
passes `--features dlss` (e.g. `cargo run -p kuluu --features dlss`).
`scripts/checks.sh` auto-sets them from `streamline/` when they are unset, so
gate runs work without the user env vars.

SDK-less environments (CI runners, release legs, Steam Deck docker) build with
`--no-default-features --features native-window`; that keeps bevy/dlss out of
the link graph entirely.

## Running / distributing

You do not ship the SDK. Next to the built binary, place:

- Windows: `$DLSS_SDK/lib/Windows_x86_64/rel/nvngx_dlss.dll`
- Linux: `$DLSS_SDK/lib/Linux_x86_64/rel/libnvidia-ngx-dlss.so.310.5.3`

plus the copyright/license text from section 9.5 of the SDK's programming
guide if distributing. If the DLL is missing, or the GPU/backend can't do
DLSS, nothing breaks: the renderer just never reports support and the menu
rows stay `N/A`.

For Neural Uplift additionally stage `nvngx_dlssnr.dll` (NR runtime v310.8)
and the forwarder staged as `nvngx.dll_kuluu.dll` next to the exe — see Part 2
§3.4/§3.5. `build_cowland.bat` does all of this automatically for local runs;
the list above is what a distribution package must contain by hand.

Expect some Vulkan validation errors with DLSS active; per dlss_wgpu these
come from a bug in DLSS itself and are safe to ignore.

## Known limitations

- **Nameplate wall-occlusion under DLSS.** The nameplate pass draws into the
  full-res post-upscale image, but the scene depth buffer only holds valid
  geometry in its render-res sub-rectangle (bevy sizes the texture from
  physical_target_size; the main pass writes a top-left viewport). A hardware
  attachment test against that buffer occludes plates against stale texels,
  so under any upscaler the pass runs a `Subrect` depth mode instead: it binds
  the single-sample scene depth as a texture and does one nearest load per
  fragment at `fragment_coord * (render_res / target_size)` — every fragment
  lands inside the sub-rect where valid geometry lives, so walls still occlude
  plates post-upscale. Plates are drawn AFTER the upscaler in all modes, so
  they are never scaled or temporally filtered by SR/NR.
- **Quality-tier changes blink** (camera respawn, see above).
- **HDR pipeline note**: the operator camera is already Hdr, which DLSS
  requires; nothing to do here, just don't remove it.
- **Neural Uplift: zero-MVec stand-in.** Bevy produces no motion-vector texture
  for our camera, so NR evaluates against a zero-filled Rg16Float stand-in —
  an explicit "no camera motion". NVIDIA confirmed (Part 2 §7.3) that the model
  receives exactly two runtime inputs: the frame and the motion vectors. Expect
  shimmer/ghosting specifically during camera movement; fix path = bevy's real
  `MotionVectorPrepass` when SR is active, then drop the stand-in (Part 2
  §2.12 known limitation).

## Placeholders

The DLSS Config surface intentionally shows more rows than are wired, so the
menu structure matches where this is going (the RenoDX Control add-on is the
reference UX). They are inert on every build and read `N/A`:

- `RR Preset` / `RR Responsivity`: Ray Reconstruction. bevy_anti_alias 0.19
  exposes SR only; RR types exist in dlss_wgpu but there is no bevy plumbing.
- `SR Preset` (the J/K/L/M model presets): not surfaced by dlss_wgpu 4.0.
- `Sharpness`: wireable today via bevy's ContrastAdaptiveSharpening; left
  inert with the rest for now, and the obvious first placeholder to bring to
  life.

## Feature-gating map (what compiles when)

Unconditional (every build): `AaMode::Dlss`, `DlssQuality`, the
`dlss_quality`/`dlss_supported` fields, every menu row and label, the
`Subrect` nameplate depth mode (keyed on MainPassResolutionOverride presence,
not on DLSS types). `dlss_supported` can only ever become true when the
feature is compiled in, so all of it is dead-quiet on SDK-less builds.

`#[cfg(feature = "dlss")]` only (opt-in: local dev/test builds pass
`--features dlss`; SDK-less environments simply don't): `kuluu-render/src/graphics/dlss.rs`
(capability probe, tier mapping, project id), `kuluu-render/src/graphics/dlss_nr.rs`
(the NR pipeline — gated at runtime by `GraphicsSettings::nr_active`, which
requires the AA mode to be Dlss as well as support) and its `kuluu-dlss-nr`
FFI crate, the `Dlss` component insert in `camera.rs`, the availability system
registration in `kuluu-render/src/lib.rs`, and the `DlssProjectId` resource
insert in `kuluu/src/view_native/mod.rs` (`DlssInitPlugin` itself is added by
Bevy's DefaultPlugins under the feature).

The DLSS project id (`KULUU_DLSS_PROJECT_ID` in graphics/dlss.rs) is fixed
for the lifetime of the project; NVIDIA's driver keys per-app behavior on it,
so never regenerate it.

---

# Part 2 — Neural Uplift (DLSSNR): research & implementation log

Goal: drive **NVIDIA DLSS 5 Neural Rendering ("Neural Uplift", a.k.a. DLSSNR)** in Kuluu by calling
`nvngx_dlssnr.dll`'s **Vulkan NGX API directly from Rust** — no ReShade, no D3D12 detours. We have client
source control (Bevy/wgpu Vulkan), so the RenoDX-DLSS5 addon's injection tricks are unnecessary for us.

Reference behavior: `streamline/renodx-dlss5.addon64` + its menu (Neural Uplift toggle, NR Intensity default
1.01, Local Tone / Structure Strength). Reference repo clone: `streamline/dlss5-feeder`
(jlrouzies-fr/DLSS5-Feeder — delegates NR settings to the addon via ReShade; not our path).

**Last verified state: build 13** (clean release build of all §2.12 fixes; evaluate succeeds every frame,
device-lost crash fixed) — rolling per-build status in the **Build history** appendix at the end of this doc.

---

## 1. Current setup (what exists today)

### Runtime stack next to the exe (repo root + `target/debug/` / `target/release/`)
| File | Role | Status |
|---|---|---|
| `nvngx_dlss.dll` | NGX backend runtime, SR/RR features (built from `rel_310_8` source tree) | staged, works |
| `sl.common.dll`, `sl.dlss.dll` | Streamline common + DLSS layer | staged |
| `nvngx_dlssnr.dll` (~166 MB) | **NR backend runtime v310.8.0** (NVIDIA-signed) | **staged at root + target/{debug,release}**; `build_cowland.bat` auto-stages it when missing |

### Build/run tooling (all personal, git-excluded via `.git/info/exclude`)
- `build_cowland.bat` (repo root): release default, `debug` arg; builds kuluu + the forwarder crate with the
  full local feature set (`dlss,debug-menu,enhanced-mob-hp-under,enhanced-job-display`); auto-sets
  `DLSS_SDK`/`VULKAN_SDK`/`LIBCLANG_PATH` from `streamline/` when unset; syncs exe via `logs\sync_exe.ps1`;
  stages `nvngx_dlss.dll` + `nvngx_dlssnr.dll` next to both exes when missing and ALWAYS re-copies the
  forwarder as `nvngx.dll_kuluu.dll`. **User builds; we only hand over the cmd.**
- `play_cowland.bat`: double-click launcher — pins `FFXI_MAP_LOCAL_PORT=47500`, sets
  `WGPU_ADAPTER_NAME=NVIDIA` + `SL_LOG_PATH=%~dp0streamline\logs`, runs `kuluu.exe --server 127.0.0.1 play`.
- (Historical: `build-dlss.bat` / `dlssplay.bat` did the same job; deleted when DLSS became a default
  feature, superseded by these two.)
- SL logs go to `streamline/logs/` (never `.verify/`).
- Excluded patterns: `/streamline/`, `/nvngx_*.dll`, `/sl.*.dll`, `/build_cowland.bat`, `/play_cowland.bat`
  (all verified via git check-ignore).

### In-repo DLSS plumbing
- Bevy 0.19 `bevy_anti_alias` → `dlss_wgpu` crate: SR via wgpu HAL raw Vulkan handles;
  `DlssInitPlugin` auto-added by `DefaultPlugins` under the dlss feature — **kuluu must NOT add it again**
  (duplicate-plugin panic). Committed fix in `kuluu/src/view_native/mod.rs`: no manual
  `app.add_plugins(DlssInitPlugin)`; only `insert_resource(project_id())`.
- `kuluu-render/src/camera.rs` (~384): inserts `bevy::anti_alias::dlss::Dlss<DlssSuperResolutionFeature>` on the
  operator camera when `dlss_active()`.
- **The NR pipeline** (all behind `feature = "dlss"`, native-only):
  - `kuluu-dlss-nr/` crate: single home for every unsafe touch of the NGX ABI (kuluu-render keeps
    `#![forbid(unsafe_code)]`; verified empirically that `#[allow]` cannot override a crate-level forbid).
  - `kuluu-render/src/graphics/dlss_nr.rs`: safe Bevy wiring around it (§3.1 below).
  - Settings: Neural Uplift toggle + NR Intensity / Local Tone Strength / Structure Strength rows are LIVE;
    DLSS is reachable solely via its explicit On/Off row (removed from the AA cycler per user request).

### SDK material in `streamline/` (git-excluded)
- `sdk/` = NVIDIA/DLSS public repo @ **v310.5.3** (`include/nvsdk_ngx*.h`, static libs under
  `lib/Windows_x86_64/{khr,vs2010}/x64/`). Public feature enum ends at `RayReconstruction = 13` / `Count`;
  **no NR in any public tag through v310.7.0** (checked; headers saved to `sdk-headers-check/`).
- `vulkan-sdk/Include/` = Vulkan-Headers 1.4.359 flattened; `llvm/bin/libclang.dll` for bindgen.

---

## 2. What the binaries expose (verified ground truth)

### 2.1 `nvngx_dlssnr.dll` v310.8.0 — export table (55 exports total)
Vulkan entry points we can call directly:
```
NVSDK_NGX_VULKAN_Init            @ RVA 0x13F50   (stub, shared with other backends)
NVSDK_NGX_VULKAN_Init_Ext        @ RVA 0x25050
NVSDK_NGX_VULKAN_Init_Ext2       @ RVA 0x251A0
NVSDK_NGX_VULKAN_CreateFeature   @ RVA 0x24A20
NVSDK_NGX_VULKAN_CreateFeature1  @ RVA 0x24B70   (adds VkDevice as first arg)
NVSDK_NGX_VULKAN_EvaluateFeature @ RVA 0x24C80
NVSDK_NGX_VULKAN_GetFeatureRequirements / GetScratchBufferSize
NVSDK_NGX_VULKAN_GetFeatureInstanceExtensionRequirements / ...DeviceExtensionRequirements
NVSDK_NGX_VULKAN_PopulateParameters_Impl @ RVA 0x252F0   (purpose unconfirmed; not needed)
NVSDK_NGX_VULKAN_ReleaseFeature  @ RVA 0x25410
NVSDK_NGX_VULKAN_Shutdown / Shutdown1
```
Plus CUDA/D3D11/D3D12 variants and generic getters (`GetAPIVersion`, `GetSnippetVersion`, …).

**NOT exported:** `AllocateParameters`, `DestroyParameters`, `GetCapabilityParameters`,
`Init_with_ProjectID`. (The RenoDX addon's *reference* NR build does export the D3D12 flavor of these — see 2.5.)

Imports: only `KERNEL32/USER32/ADVAPI32/VERSION` → fully self-contained runtime; loads whatever else it needs
dynamically. Built from the same `rel_310_8` tree as our `nvngx_dlss.dll`.

**Confirmed signatures (from public SDK headers, stable across versions):**
```c
NVSDK_NGX_Result NVSDK_NGX_VULKAN_Init_Ext(
    unsigned long long InApplicationId,      // u64 project id (we pass lower 64 bits of KULUU_DLSS_PROJECT_ID)
    const wchar_t *InApplicationDataPath,    // NGX logs/models land here — we use the OS temp dir (dlss_wgpu convention)
    VkInstance InInstance, VkPhysicalDevice InPD, VkDevice InDevice,
    NVSDK_NGX_Version InSDKVersion,          // pass 0x15 (NGX_VERSION_DOT 1.5.0)
    const NVSDK_NGX_Parameter *InParameters);// we pass NULL

NVSDK_NGX_Result NVSDK_NGX_VULKAN_CreateFeature(
    VkCommandBuffer InCmdList, NVSDK_NGX_Feature InFeatureID /* 0x12 for NR */,
    const NVSDK_NGX_Parameter *InParameters, NVSDK_NGX_Handle **OutHandle);

NVSDK_NGX_Result NVSDK_NGX_VULKAN_EvaluateFeature(
    VkCommandBuffer InCmdList, const NVSDK_NGX_Handle *InFeatureHandle,
    const NVSDK_NGX_Parameter *InParameters, PFN_NVSDK_NGX_ProgressCallback /* NULL */);

NVSDK_NGX_Result NVSDK_NGX_VULKAN_ReleaseFeature(NVSDK_NGX_Handle *InHandle);  // handle-only, NOT cmd-encoded
NVSDK_NGX_Result NVSDK_NGX_VULKAN_Shutdown1(VkDevice InDevice);
```
**Handle ABI — OPAQUE 64-bit object pointer, NOT the public header's `{ unsigned int Id; }`** (build-11 finding,
supersedes the old note here):
- Backend create (@ 0x180017E20) mallocs a 0xb8-byte handle object on success: vtable ptr @ +0, refcounts
  @ +8/+0xC, then stores it into *OutHandle as a **full qword** (`mov QWORD PTR [rcx],rax` @ 0x1800183AE).
- Internal create registers the object in the FNV table rooted at global 0xE0650 under key = `*(u32*)obj`
  (low 32 bits of the vtable address — a build constant ≈ 0x800AFC00).
- EvaluateFeature @ 0x180024C80 and internal release @ 0x1800240A0 both read `*(u32*)InHandle` and FNV-lookup
  that table; miss ⇒ log + **InvalidParameter 0xBAD0_0005**. Hit ⇒ context = [node+0x18], validate, dispatch to
  backend eval @ 0x180018620 → parser @ 0x19F30.
- Consequence: the caller must pass back the **exact 64-bit value CreateFeature wrote**, cast to a pointer —
  never a pointer to its own u32 storage. Build 10's `NvngxHandle{id:u32}` + `&mut handle` as OutHandle meant
  evaluate sent a truncated heap address ≠ registered key ⇒ per-frame InvalidParameter (the user's log).
- RenoDX addon proves the usage: create into a qword slot (`cmp QWORD PTR [rdi],0` after the call), evaluate
  with `mov rdx,[rsi]` — the stored value itself as InFeatureHandle (~0x1800255C8 in addon_disasm.txt).

### 2.2 The NR parameter contract — internal parser at RVA `0x19F30`
The parser walks **our caller-supplied opaque parameter object** and looks up each field by name through three
function-pointer slots on the object (see §2.3 for which slots). Complete accepted parameter list:

**Textures** (value = pointer to an `NVSDK_NGX_Resource_VK`; each also accepts `…SubrectBaseX/BaseY/Width/Height` u32 params, default 0):
```
DLSSNR.Color   DLSSNR.MVec   DLSSNR.Depth   DLSSNR.Output
DLSSNR.ControlMask   DLSSNR.UI   DLSSNR.UIAlpha   DLSSNR.Backbuffer
DLSSNR.BidirectionalDistortionField
```

**Floats:** `DLSSNR.MVecScaleX` (def 1.0), `MVecScaleY` (1.0), `Intensity` (1.0 — the addon menu's "NR Intensity",
default 1.01 there), `LocalToneStrength` (1.0), `SkinStructureStrength` (def −1.0 = "unset"),
`DLSSNR.LocalStructureStrength` (stored as f64, def 1.0).

**u32/bool:** `UseAutoMask` (0), `Reset` (0), `DepthInverted` (default **1**), `Enabled` (default **1**),
`UICorrection` (0), `Style`, and the oddballs `DLSS.Indicator.Invert.X.Axis` / `.Y.Axis` (0).

**Getter-slot map (build-12 disasm of parser @ 0x19F30 + backend eval @ 0x180018620 — CRITICAL for the Set* choice):**
the parser does NOT read every param through a type-matched getter. Verified slot usage:
- **GetI (+0x58)**: `DLSSNR.Reset`, `Enabled`, `DepthInverted`, and ALL subrect params
  (`…SubrectWidth/Height` for Color/MVec/Depth/Output). A value stored via SetUI is INVISIBLE to these
  lookups (they read the int table, defaulting to 0) — build 11 sent DepthSubrectWidth/Height via SetUI,
  so with SR active the runtime never saw them and read 0. Confirmed bug; likely contributor to the
  "ugly" visuals.
- **GetUI (+0x60)**: `Style` is the ONLY param read through this slot in the parser.
- Everything else as §2.3 says (resources via GetVoidPointer +0x40, floats via GetF +0x70).
Consequence encoded in kuluu-dlss-nr: subrects/Reset/Enabled/DepthInverted all go through `set_i`;
Style would need `set_ui` if we ever send it (we don't — parser default).

**Quirk:** `DLSSNR.ScalingRatio` is looked up but then unconditionally overwritten to 1.0 locally — accepted,
but apparently not honored by this build.

Derived logic in the parser: if `ControlMask` resource present → force `UseAutoMask=0`; when
`SkinStructureStrength >= 0` it replaces the local-structure value used downstream (two derived f32 slots at
local +0xf8/+0xfc).

### 2.3 THE parameter-object ABI — fully decoded ✅
The opaque `NVSDK_NGX_Parameter*` is **not** a flat name/value array. It is:
```
param_ptr → [ +0x00 : inner_obj* ]          (first qword of the handle)
inner_obj = struct of function pointers, called as fn(param_ptr, name_str, out_or_value):
  +0x00 SetVoidPointer      +0x40 GetVoidPointer   ← resource lookups (out=qword, def NULL)
  +0x08 SetD3d12Resource    +0x48 GetD3d12Resource
  +0x10 SetD3d11Resource    +0x50 GetD3d11Resource
  +0x18 SetI                +0x58 GetI             ← int lookups (out=dword, def 0)
  +0x20 SetUI               +0x60 GetUI
  +0x28 SetD                +0x68 GetD
  +0x30 SetF                +0x70 GetF             ← float lookups (out=dword f32, def 1.0f)
  +0x38 SetULL              +0x78 GetULL
```
Evidence: disassembled `nvsdk_ngx_parameters_lib.obj` from the static host lib — every exported
`NVSDK_NGX_Parameter_Set*/Get*` does exactly `obj = *param_ptr; fnptr = obj->slot_off; jmp-rax-trampoline(fnptr)`
with rcx=param_ptr, rdx=name, r8/r9=value-or-out. The v310.8 runtime parser uses slots **+0x40 / +0x58 / +0x70** —
**identical offsets in the v310.5.3 host layer**. Cross-version ABI confirmed compatible for everything NR needs.

**Consequence:** we do NOT hand-build any struct layout. We call the static lib's own
`NVSDK_NGX_VULKAN_AllocateParameters(&map)` + `SetVoidPointer/SetI/SetF(map, "DLSSNR.…", …)`, then pass that map
pointer straight into `nvngx_dlssnr.dll!VULKAN_EvaluateFeature`. This is exactly what the RenoDX addon does with
its bundled host layer (allocate in one, evaluate in another).

**Bonus:** `NVSDK_NGX_VULKAN_AllocateParameters` / `_DestroyParameters` are exported by **the very static lib
dlss_wgpu already links** (`nvsdk_ngx_s.lib`, `lib/Windows_x86_64/x64/`) — verified via nm. kuluu-dlss-nr just
declares the externs; no new link dependency, no bindgen needed (signatures are trivial C).

Note: `DlssSdk.parameters` in dlss_wgpu is `pub(crate)` → we allocate our **own** map for NR (clean isolation;
SR path untouched).

### 2.4 Raw Vulkan handles — all available from Bevy resources ✅ (re-verified against registry sources this session)
Exact type shapes in the pinned versions (**wgpu/wgpu-hal 29.0.4, ash 0.38.0+1.3.281**):
- **ash handle layout**: dispatchable handles (`Instance`, `PhysicalDevice`, `Device`) are generated by
  `define_handle!` → `#[repr(transparent)] pub struct X(*mut u8)`; non-dispatchable (`Image`, `ImageView`, …) by
  `handle_nondispatchable!` → `pub struct X(u64)`. So: instance/pd/device = `.0 as u64`; image/imageview = `.0`.
- **`vk::Format`** is `pub struct Format(pub(crate) i32)` — inner field PRIVATE; use the public
  `as_raw() -> i32` (then `as u32`). No ash dependency needed in kuluu-dlss-nr: every extraction works through
  type inference on wgpu-hal's return types.
- **Device**: `device.as_hal::<Vulkan>()` → hal device with `shared_instance() -> &InstanceShared`,
  `.raw_physical_device() -> vk::PhysicalDevice`, `.raw_device() -> &ash::Device`;
  `InstanceShared::raw_instance() -> &ash::Instance`; `ash::{Instance,Device}::handle() -> vk::{Instance,Device}`.
- **Textures**: `TextureView::as_hal::<Vulkan>()?.raw_handle() -> vk::ImageView` (u64);
  `Texture::as_hal::<Vulkan>()?.raw_handle() -> vk::Image` (u64); raw format via
  `adapter.as_hal::<Vulkan>()?.texture_format_as_raw(fmt).as_raw()` (wgpu-hal vulkan/adapter.rs:2826).
- **Command buffers**: `encoder.as_hal_mut::<Vulkan,_,_>(|enc| enc.raw_handle())` → vk::CommandBuffer
  (dispatchable → `.0 as u64`). Same pattern dlss_wgpu uses.
- Per-frame raw cmd buffer: own wgpu CommandEncoder in the node; finish + `ctx.add_command_buffer()`.

### 2.5 RenoDX-DLSS5 addon — how a third party drives this today
- Imports only `KERNEL32` (+USER32/BCrypt for signature checks); everything else via runtime
  `LoadLibraryW` + `GetProcAddress`. It detours the game's D3D12 calls into `nvngx_dlss.dll`, then loads its own
  copy of `nvngx_dlssnr.dll` and drives it **directly**:
  `NVSDK_NGX_D3D12_Init_Ext → AllocateParameters → CreateFeature(0x12) → EvaluateFeature → ReleaseFeature → Shutdown1`.
- It expects the NR DLL to export `AllocateParameters`/`DestroyParameters` (its reference build does; **ours
  doesn't** — we use the static lib's instead, which is strictly better). Tolerates missing `EvaluateFeature_C`.
- **NR FeatureId = 0x12 (decimal 18)** — from its CreateFeature call site (`mov edx,0x12`). Public enum ends at 13;
  18 is the new v310.8 feature (presumably `NVSDK_NGX_Feature_DLSSNR` / NeuralRendering).

### 2.6 The static host layer: `nvsdk_ngx_s.lib` (SDK v310.5.3) — what we link
Already linked into the binary via dlss_wgpu (`cargo:rustc-link-lib=static=nvsdk_ngx_s`). Exports everything in
§2.3 plus `NVSDK_NGX_VULKAN_Init_with_ProjectID`, `GetCapabilityParameters`, typed helpers, etc. Loads runtimes
via trusted-location LoadLibrary; knows feature→DLL suffixes `dlss`(implicit), `dlisr`, `dlslowmo`,
`dlinpainting`. **Knows nothing about NR** (no NeuralRendering/DLSSNR strings) — so CreateFeature(0x12) must go
to the NR runtime DLL directly, not through this layer's context.

### 2.7 Public API shapes we already use (from dlss_wgpu, working in-game today)
```c
NVSDK_NGX_VULKAN_Init_with_ProjectID(project_id, ENGINE_TYPE_CUSTOM, engine_version, app_data_path,
    raw_instance, physical_device, device, get_instance_proc_addr, get_device_proc_addr, feature_info, version);
NGX_VULKAN_EVALUATE_DLSS_EXT(cmd_buf, feature_handle, params_map, &typed_eval_struct);  // SR only (helpers header)
```
Resources are wrapped as `NVSDK_NGX_Resource_VK` (plain C struct: VkImageView + VkImage + subresource range +
format + w/h + readWrite flag — `nvsdk_ngx_helpers_vk.h`) from **wgpu HAL raw handles** via
`NVSDK_NGX_Create_ImageView_Resource_VK(...)`; textures are set with
`NVSDK_NGX_Parameter_SetVoidPointer(params, "SuperSampling.Color", &resource_struct)`.

### 2.8 Bevy-side ground truth verified this session (0.19.1 / wgpu 29.0.4)
- **Depth prepass under SR**: `prepare_prepass_textures` sizes the depth texture at full physical target size,
  but the prepass *node* (`bevy_core_pipeline/src/prepass/node.rs`) applies
  `Viewport::from_viewport_and_override(camera.viewport, MainPassResolutionOverride)` — which only changes
  `physical_size`, offset stays top-left. **So with SR active the full-size depth texture has valid data only in
  its top-left subrect = render resolution** → we pass `DLSSNR.Depth.SubrectWidth/Height` from
  `MainPassResolutionOverride`; without SR no subrect params (whole texture valid). Color never needs a subrect:
  after SR the main texture is fully written at full res; without SR it's rendered at full res.
- **MSAA caveat**: prepass depth uses `msaa.samples()` — with MSAA on, depth is multisampled and unsampleable by
  NGX → v1 passes Depth only when `sample_count() == 1`, else NR runs color-only (parser tolerates missing depth).
- **Main texture usages**: bevy's own `prepare_dlss` ORs `STORAGE_BINDING` into `CameraMainTextureUsages` because
  NGX writes through storage ops. Our `prepare_nr` does the same; the usage is part of the view-target cache key,
  so textures are recreated with it on the next frame.
- **Ping-pong**: `ViewTarget::post_process_write()` (bevy_render view/mod.rs:952) flips main texture A↔B and
  returns `PostProcessWrite { source, source_texture, destination, destination_texture }` — fields, not methods;
  caller MUST write destination. `ViewTarget::main_texture_view()` gives the current main view WITHOUT flipping,
  so we build the color resource first and only flip once committed to encoding.
- **Node placement**: Core3d set order is `(Prepass, MainPass, EarlyPostProcess, PostProcess).chain()`; SR lives in
  EarlyPostProcess → NR node goes in **PostProcess** so it enhances whatever SR produced when both are on.
- **Per-frame command flow** (dlss_wgpu pattern, replicated): barriers via `ctx.command_encoder().transition_resources`
  (source→RESOURCE, depth→RESOURCE, output→STORAGE_READ_WRITE), then encode EvaluateFeature into a SEPARATE
  encoder's command buffer and `ctx.add_command_buffer(cb)` — submitted right after the main one. RenderContext in
  0.19 has no device accessor → our per-camera context carries a `wgpu::Device` clone for its own encoders.
- **Extraction is opt-in** (bevy_render::extract_component): component needs `SyncComponent { type Target = Self }`
  + `ExtractComponent` and an `ExtractComponentPlugin::<T>` in the main app; removal propagates automatically.
- **NGX data path**: dlss_wgpu passes `env::temp_dir()` to its init → NR matches (no new folders next to the exe).

### 2.9 Compile-risk audit against pinned sources (this session) — results
Every API the two new files touch was checked against the exact locked versions in the cargo registry
(bevy/bevy_render/bevy_core_pipeline/bevy_camera **0.19.1**, wgpu/wgpu-hal/wgpu-types **29.0.4**,
ash **0.38.0+1.3.281**, glam **0.32.1**). Ground truth that differs from older-bevy assumptions:

- **`ExtractedView.viewport` is a `UVec4`** (`uvec4(origin.x, origin.y, width, height)`), NOT the old
  `Viewport` struct — so `view.viewport.zw()` returns a `UVec2` of u32s (bevy itself calls `.zw()` on it at
  bevy_render view/mod.rs:1045). No casts needed anywhere in prepare_nr.
- **`MainPassResolutionOverride(pub UVec2)`** — inner is already u32; the depth-subrect params need no cast.
- **Prepass depth chain**: `ViewPrepassTextures.depth: Option<ColorAttachment>` →
  `ColorAttachment.texture: CachedTexture { texture: wgpu::Texture, default_view: wgpu::TextureView }`.
  So sample-count check is `.texture.texture.sample_count()` and the view to hand NGX is
  `.texture.default_view` (a plain wgpu TextureView — exactly what `from_texture_view` takes).
- **wgpu-hal 29 `raw_handle()` is an UNSAFE fn** on the Vulkan Image/ImageView/CommandEncoder types
  (vulkan/mod.rs:743/775/1032). The first draft called it outside its `unsafe {}` block → E0133.
- **`wgpu::hal` is public in wgpu 29** (`pub extern crate wgpu_hal as hal;`, lib.rs:104) and
  `wgc::api` (dlss_wgpu's import path) is a plain re-export of the same module — so
  `use wgpu::hal::api::Vulkan;` names the identical marker type dlss_wgpu uses.
- **Barriers**: wgpu 29 `CommandEncoder::transition_resources(buffer_iter, texture_iter)` takes two
  iterators; items are `TextureTransition<&'a Texture>` (reference field). bevy's
  `PostProcessWrite { source_texture: &'a Texture, destination_texture: &'a Texture }` already hands out the
  right references — matches dlss_wgpu's `barrier_list()` pattern verbatim.
- **Extraction**: `QueryItem<'w,'s,Q> = <Q as QueryData>::Item<'w,'s>`; for `&'static T` that is `&'w T`, so
  `Some(*item)` with `Out = Self` (Copy) compiles — my impl is a verbatim copy of bevy's own
  `CameraMainTextureUsages` impl (bevy_render camera.rs:137). `QueryItem` re-exports through
  `bevy_ecs::query` (`pub use fetch::*`).
- **ash handle shapes re-verified from the macro definitions** (vk/macros.rs): `define_handle!`
  (Instance/PhysicalDevice/Device) → `pub struct X(*mut u8)` ⇒ `.0 as u64`; `handle_nondispatchable!`
  (Image/ImageView) → `pub struct X(u64)` ⇒ `.0`. `vk::Format(pub(crate) i32)` has public
  `as_raw() -> i32`.
- **Device-side extraction**: hal Vulkan device's `shared_instance()` / `raw_physical_device()` /
  `raw_device()` are safe fns; `InstanceShared::raw_instance() -> &ash::Instance` exists (instance.rs:181);
  `texture_format_as_raw` is safe.
- **Schedules**: `Core3dSystems` is an enum with a `PostProcess` variant; `prepare_view_targets` and
  `RenderContext::{command_encoder, add_command_buffer}` exist as used.

**Bugs found & fixed by this audit + first rustc pass:**
1. `kuluu-dlss-nr/src/lib.rs::from_texture_view` — `.raw_handle()` calls moved inside one `unsafe {}`
   block (they were outside; only `as_hal` was wrapped).
2. `kuluu-render/src/graphics/dlss_nr.rs::nr_node` — depth filter corrected to
   `depth.texture.texture.sample_count()` (was calling `sample_count()` on the CachedTexture wrapper).
3. **First build (user's `build-dlss.bat debug`) surfaced 11 errors, all fixed:**
   - ash handle tuple fields are **private** (`pub struct X(*mut u8)` — only the struct is pub). Raw
     extraction goes through the public `ash::vk::Handle` trait: `.as_raw() -> u64`. kuluu-dlss-nr now
     depends on `ash = "0.38"` (resolves to the exact locked copy wgpu-hal already builds — zero new
     compile cost).
   - `NGX_SUCCESS as u32` is not a valid match pattern → literal `0x1`.
   - `transmute_copy` in the export-resolution macro needed its own `unsafe {}` block.
   - **Pre-empted before it could surface**: the command-buffer extraction (`as_hal_mut` +
     `raw_handle().0`) lived in dlss_nr.rs — that would have been both a private-field error AND an
     `unsafe {}` block rejected by kuluu-render's `#![forbid(unsafe_code)]`. Moved into the FFI crate as
     `kuluu_dlss_nr::raw_command_buffer(&mut encoder) -> Option<u64>` (needs &mut — as_hal_mut takes
     &mut self); dlss_nr.rs is now 100% safe code.
4. **Second build: 25 errors, all name-resolution in dlss_nr.rs** (kuluu-render has no direct `wgpu` or
   `tracing` deps — the audit assumed bevy's prelude covered them; it doesn't):
   - `tracing::info!/warn!/error!` ×12 → bare macros: bevy 0.19's prelude re-exports tracing's macros via
     `bevy_log::prelude` (that's how kuluu-render's audio.rs logs).
   - `CommandEncoderDescriptor`, `TextureTransition`, `TextureUses`, `TextureUsages`, `wgpu::Device` →
     added native-only `wgpu = { version = "29", default-features = false, features = ["std"] }` to
     kuluu-render (same locked copy as bevy's — zero new compile) + explicit import.
   - `SyncComponent` is NOT re-exported from `bevy::render::extract_component` (private use there) →
     public home is `bevy::render::sync_component`.
5. **Third build: 6 errors, all type mismatches — bevy 0.19 wraps wgpu types** (the audit's blind spot:
   I read the *field names* in PostProcessWrite but not which `Texture` import view/mod.rs uses):
   - `bevy_render::render_resource::{Texture, TextureView}` are **wrapper structs** (`WgpuWrapper<wgpu::…>`
     + id) that `Deref` to the wgpu types. So `PostProcessWrite.source_texture/destination_texture` are
     `&'a bevy Texture`, and `CachedTexture.default_view` / `main_texture_view()` are bevy TextureViews.
   - Consequence A: building `wgt::TextureTransition { texture }` by value infers the generic from what you
     pass — no deref coercion into a generic. Fix: annotate
     `Vec<TextureTransition<&wgpu::Texture>>`; then `&bevy Texture` coerces via Deref and `dv.texture()`
     (which already yields `&wgpu::Texture`) fits.
   - Consequence B: `RenderDevice` is NOT an alias for wgpu::Device — it's a struct with a private
     `WgpuWrapper<wgpu::Device>` field, derives Clone (so `.clone()` returns RenderDevice), and exposes the
     inner device via **`pub fn wgpu_device(&self) -> &wgpu::Device`** (render_device.rs:259).
   - Also: `runtime.init(…, handles, …)` takes `&VulkanHandles` — pass `&handles`; and the settings-mirror
     match in apply_neural_uplift_system was missing its `(false, None)` no-op arm.
   - **Fixes APPLIED this round** (5 edits in dlss_nr.rs; each re-verified against pinned registry sources
     before applying — `RenderDevice::wgpu_device()` @ render_device.rs:259, `PostProcessWrite` fields @
     view/mod.rs:715–720, bevy Texture/TextureView Deref impls @ texture.rs:54/107):
     - `ensure_initialized`: `VulkanHandles::from_wgpu(device.wgpu_device())`; `runtime.init(…, &handles, …)`.
     - `prepare_nr`: `NrInner.device = render_device.wgpu_device().clone()` (RenderDevice derives Clone —
       `.clone()` alone returned the wrong type).
     - `nr_node`: barriers annotated `Vec<TextureTransition<&wgpu::Texture>>` — one annotation fixes both
       the `dv.texture()` mismatch and the `transition_resources` iterator bound; `&bevy Texture` then
       deref-coerces where the target type is known.
     - `apply_neural_uplift_system`: added the `(false, None) => {}` no-op arm.
6. **Fifth build: all six type errors cleared → borrow-checker round (2 errors + 1 warning), all fixed:**
   - E0596 @ prepare_nr: `usages.0 |= STORAGE_BINDING` needs the binding mutable —
     `if let Some(mut usages) = main_texture_usages`. (Didn't surface in build 4: type errors
     short-circuit before borrow-checking.)
   - E0382 @ prepare_nr: `match context { Some(ctx) => … }` MOVED the `Option<Mut<'_, NrContext>>`
     out of the loop binding, then the release path (`if let Some(ctx) = context`) used it again.
     Fix: `Some(ref ctx)` in the size-check match — field access auto-derefs through bevy's
     `Mut`, and `.lock()` takes `&self`, so nothing else changes; the later move is now legal.
   - unused_mut @ nr_node: `let mut inner = …lock().unwrap()` — evaluate_nr borrows everything,
     nothing is mutated through that lock → dropped the `mut`.
7. **First run on build 6 (release, clean compile+link): NR DLL load fails with a misleading message — diagnostics added:**
   - Log said `dlss-nr: nvngx_dlssnr.dll not found next to the executable` while the file sits right
     next to kuluu.exe. `LoadLibraryW` returns NULL for ANY failure (missing dependency 126, DllMain
     refusal 1114, policy block 5…) and the old code lumped them all into "not found".
   - Verified: root + target/debug + streamline copies are byte-identical; sha256 e16bcf…fc8e matches
     the runtime hash RenoDX reported when it loaded this exact DLL successfully on this machine —
     the file is known-good and loadable here.
   - Fix (this round): `NrRuntime::load()` now loads by ABSOLUTE path next to current_exe()
     (deterministic search), returns `Result<Self, LoadError>` carrying either `Win32(GetLastError())`
     or `MissingSymbol(name)`, and the log line renders code + hint. Build 7's first-run log names
     the real cause. While it fails, the warning repeats at most once per second (retry throttle).
   - Build 7's first pass: E0515 in the new loader itself — `to_str()` borrowed from a temporary
     PathBuf created inside an and_then; fixed by materializing the joined path in a local first.
8. **Build 7 first run: still "export missing" → ROOT CAUSE = unterminated string in `resolve!` (§4 item 4):**
   - Build 7's log: `DLL loaded but export 'NVSDK_NGX_VULKAN_Init_Ext' is missing` — LoadLibraryW now
     succeeds (absolute path), so the failure moved to GetProcAddress.
   - Ruled out in order: DLL file (byte-identical, sha256 e16bcf…fc8e = RenoDX's successful runtime hash);
     export table (objdump -p: all five symbols exported by name, Init_Ext @ index 49 / ordinal 50);
     kernel32 binding (our exe imports plain GetProcAddress by name; a .NET P/Invoke to the same DLL +
     symbol SUCCEEDS — so bytes + OS + file are fine).
   - The bug: `GetProcAddress(hmod, $sym.as_ptr())` where `$sym` is a Rust `"literal"` — `str::as_ptr()`
     points at 27 bytes with **no NUL terminator** (Rust string literals are not C strings). GetProcAddress
     reads past the literal into adjacent .rodata until it hits some zero byte and searches for that longer
     garbage name → no match → NULL. The macro's own SAFETY comment ("ASCII NUL-terminated symbol name") was
     simply wrong. Every other string crossing in the crate (all `NrParams` setters, LoadLibraryW path, init
     data-path) already went through `cstr()` / explicit `\0` — only this one call site was bare.
   - Fix: macro now does `let c_name = cstr($sym); GetProcAddress(hmod, c_name.as_ptr())`, and a NULL result
     carries `GetLastError()` into `LoadError::MissingSymbol(sym, code)` (127 = name truly absent; 126 would
     mean the module handle was bad). No ordinal-based lookup needed — that was only a fallback for an
     unknown cause.
9. **Build 9 first compile round: 3 errors, all in the new forwarder code (kuluu-dlss-nr), all fixed:**
   - E0599 @ load_forwarder: `OsStr::encode_wide` is a TRAIT method — needs
     `use std::os::windows::ffi::OsStrExt;` at crate top. The crate's existing `encode_wide(&str)` helper
     only covers &str, and the forwarder path comes from a Path (the NR-DLL loader keeps its to_str()
     fallback pattern; the forwarder uses OsStr so non-UTF8 paths can't silently degrade).
   - E0107/E0308 ×2 (two rounds): `std::mem::transmute_copy` takes TWO generics — `<T, U>` where src is
     `&T` and the RETURN type is U. Round 1 supplied only T; round 2 supplied both but in the WRONG order
     (`::<PfnFwdAbiVersion, *mut c_void>` → "expected &fn() -> u32, found &*mut c_void"). Final form —
     source pointee first:
     `transmute_copy::<*mut c_void, PfnFwdAbiVersion>(&p_abi)` / `…::<*mut c_void, PfnFwdVulkanInitExt>(…)`.
   - Round 3, overflowing_literals ×1: the sentinel `0xF0F0_0001` does not fit an i32 literal
     (NvngxResult is c_int) → rustc's own suggestion: `pub const FWD_NULL_TARGET: NvngxResult =
     0xF0F0_0001u32 as i32;`. Bit pattern preserved — the forwarder returns it in eax, and both
     comparisons (`r == FWD_NULL_TARGET` in dlss_nr.rs, `match r as u32 { 0xF0F0_0001 => … }` in
     result_name) are unaffected by the sign.
10. **Build-12 compile round (this session's pre-test check): wgpu-29 API drift + one shadowing trap, all fixed:**
   - `Device::as_hal::<Vulkan>()` returns an **Option**, not a Result → let-else guard in
     `wait_device_idle`; first attempt also hit the let-else syntax rule (no bare block before `else` —
     bind to a local first). Non-Vulkan case gets the named sentinel `WAIT_IDLE_NOT_VULKAN: i32 = -1`
     instead of dlss_wgpu's `.unwrap()` panic.
   - Missing imports in dlss_nr.rs: `TextureDescriptor`, `TextureViewDescriptor` (wgpu 29's
     `TextureDescriptor<'a>` is a TYPE ALIAS for `wgt::TextureDescriptor<Label<'a>, &'a [TextureFormat]>`
     — fields unchanged, but `view_formats` is now `&[TextureFormat]`, not Option).
   - **`Extent3d::new()` no longer exists** in wgpu-types 29 → struct literal
     `{ width, height, depth_or_array_layers }` (two call sites).
   - **`TexelCopyTextureInfo` gained an `aspect: TextureAspect` field**; the enum has NO `COLOR` variant —
     variants are All/StencilOnly/DepthOnly/Plane0..2 → use `All` (the default) for color copies.
   - **Shadowing trap**: `RenderDevice` has its OWN inherent `create_texture(&self, &wgpu::TextureDescriptor)
     -> bevy Texture` (render_device.rs:235) that shadows the deref'd wgpu method — so
     `render_device.create_texture(…)` silently returned bevy's wrapper and `.create_view()` gave a bevy
     TextureView (E0308 against our `wgpu::TextureView` field). Fix: call through
     `render_device.wgpu_device().create_texture(…)`.
   - Assignment through the MutexGuard needs plain field syntax (`inner.last_depth_sig = …`, not
     `*inner.last_depth_sig = …`) with a `mut` guard.
11. **Build-12 first run: wgpu-core 29 encoder-API mixing panic (user-found, fixed in build 13):**
   `prepare_nr` encoded the one-time MVec clear (`begin_render_pass`, high-level) on the SAME
   CommandEncoder that was then handed to raw NGX CreateFeature via `as_hal_mut`
   (`raw_command_buffer`). wgpu-core tracks an `EncodingApi` per encoder — Undecided, locked on first
   use; a second use through the OTHER API panics ("Mixing the wgpu encoding API with the raw encoding
   API is not permitted", command/mod.rs:568). Fix: two encoders — the clear runs on its own
   high-level-only encoder (`kuluu_dlss_nr_mvec_clear`) and is submitted via
   `render_queue.submit([mvec_encoder.finish()])` BEFORE the raw-only create encoder, so queue order
   guarantees the clear completes before any frame samples MVec. `nr_node` needed no change: its shared-ctx
   barriers stay on the high-level encoder and its evaluate buffer is added separately via
   `ctx.add_command_buffer`. General rule: **one CommandEncoder, one API style** — never mix
   begin_render_pass/transition_resources/copy_* with an `as_hal_mut`'d raw path on the same encoder.

### 2.10 THE "nvngx.dll" calling-module gate — why Init_Ext returns PlatformError (build 8 finding)

**The exact error.** Build 8 loaded the DLL and resolved all five symbols (the string bug is dead), but
`NVSDK_NGX_VULKAN_Init_Ext` returns `PlatformError` = **0xBAD0_0002**. Disassembly of the entry point
(RVA 0x25050, image base 0x180000000) shows it is NOT a Vulkan init at all — its first two actions are:

```text
Init_Ext(app_id, data_path, instance, pd, device, version, params):
  1. IAT[0x1800ac118](6, <return address>, &out)      ; determine the CALLING module from our return addr
     fail → log "Error: Unable to determine calling module" (str @ RVA 0xae1d0)
            + "NVSDK_NGX_VULKAN_Init_Ext" (str @ 0xb1218) → return 0xBAD0_0002
  2. IAT[0x1800ac080](out, &name_buf, 0x104)          ; get that module's file name (wide, ≤260 chars)
  3. wcsicmp(name_buf, L"nvngx.dll")                  ; case-insensitive compare vs str @ RVA 0xae280
     mismatch → log "Error: Not called from N…" (str @ 0xae298) → return 0xBAD0_0002   ← WE ARE HERE
  4. only then: real init @ RVA 0x23dd0 with the original args
```

**What it means.** The v310.8 NR backend DLL refuses to initialize unless the module that called Init_Ext is
named exactly **`nvngx.dll`**. That is NVIDIA's trust boundary: feature backends (`nvngx_dlssnr.dll`, and by
the same pattern `dlisr/dlslowmo/dlinpainting`) are meant to be initialized *by the main NGX runtime*
(`nvngx.dll` — what a native DLSS 5 game ships), never directly by a game or third-party tool. Our caller is
`kuluu.exe`, so check 3 fails, every time, deterministically.

**Scope of the gate (verified in this DLL):**
- `NVSDK_NGX_VULKAN_Init_Ext` @ RVA 0x25050 — gated ✅ (our path)
- `NVSDK_NGX_D3D12_Init_Ext` @ RVA 0x15df0 — **same gate, same L"nvngx.dll" string** (RenoDX's path)
- `NVSDK_NGX_D3D11_Init_Ext` @ RVA 0x13f60 — same gate
- `NVSDK_NGX_VULKAN_Init` / all plain `_Init`s @ RVA 0x13f50 — one-instruction stubs returning
  `FeatureNotSupported` (0xBAD0_0001); no bypass there.
- **EVERY other exported entry point is gated too.** Each carries its own copy of the same prologue;
  verified by mapping all `GetModuleHandleExW` IAT call sites (slot @ 0x1800ac118, flags=6) in
  nr_full_disasm.txt: CreateFeature @ 0x24a20, CreateFeature1 @ 0x24b70, GetFeatureRequirements-ish
  @ ~0x24e30, GetScratchBufferSize-ish @ ~0x24f50 (out=0), Init_Ext2 @ 0x251A0,
  PopulateParameters_Impl @ 0x252F0, ReleaseFeature @ 0x25410, Shutdown @ ~0x25500, Shutdown1 @ ~0x25610.
- **Only `EvaluateFeature` @ 0x24c80 is ungated** — its prologue goes straight to the lock + FNV-hash
  dispatch and its body contains no IAT call. The first write-up's "the gate is init-only" was wrong:
  it had only read a few prologues, and build 9 proved it when CreateFeature(0x12) returned
  PlatformError from kuluu.exe.

**Answered (§2.11):** RenoDX/OptiScaler defeat it with a **forwarder DLL** — a tiny module whose file name
contains `nvngx.dll`, which makes the one gated call on our behalf so the return address lands in the right
module. No byte-patching anywhere; the gate is defeated by caller identity, not modification.

### 2.11 The gate, answered: it is a calling-module NAME test, and RenoDX/OptiScaler pass it with a forwarder

**What the check actually is.** Steps 1-3 of the `Init_Ext` prologue (§2.10) resolve the return address
to a module, fetch that module's file name, and test it against `nvngx.dll`. Two corrections to the
first write-up (both verified in disasm this session):

- Step 1 is **`GetModuleHandleExW(6, retaddr, &out)`** — flags 6 =
  `GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT`. That is the
  mystery constant `mov $0x6,%ecx`.
- Step 3 (fn @ RVA 0x7ee40) is a **case-insensitive substring search** of `nvngx.dll` inside the caller's
  FULL file name, not an equality test. Two code paths: SSE4.2 (`pcmpistri $0xd`, case-insensitive,
  gated on CPU-feature global @ RVA 0x1140560 ≥ 2 — always true on modern x64) and a scalar fallback.
  Consequence: any module whose path contains `nvngx.dll` passes — including the driver's own
  `C:\...\nvngx.dll`, and forwarders named e.g. `nvngx.dll_dlssnr.dll` (OptiScaler's, public release
  notes) or our `nvngx.dll_kuluu.dll`. Equality would have made both of those impossible.

**Public confirmation.** OptiScaler_DLSSNR ships a ~108 KB forwarder named `nvngx.dll_dlssnr.dll`
documented as existing only because the NR runtime refuses calls from any module whose path does not
contain `nvngx.dll`. RenoDX's addon (strings in its .rdata: "signed feature has no GetModuleFileNameW
import…", "failed to make signed-feature IAT writable…") uses the same identity mechanism internally.
No one byte-patches the DLL; the gate is defeated by identity, not by modification.

**Consequence for kuluu.** Every entry point we call except `EvaluateFeature` is gated (§2.10 scope map),
so the forwarder carries one trampoline per gated call: Init_Ext (build 9) + CreateFeature +
ReleaseFeature (build 10, ABI v2). EvaluateFeature stays a direct call — verified ungated in v310.8;
if a future driver drop adds a gate there, its PlatformError log line names the fix (one more
trampoline). This also explains OptiScaler's ~108 KB forwarder: a trampoline table for all entry
points, not just init.

**Why a forwarder and not a patch.** Patching the two `jne`s means writing into a signed, 166 MB,
version-specific binary at load time, re-locating the offsets every driver drop, and fighting any
integrity check NVIDIA adds later. The forwarder is ~90 lines, has zero dependencies, and is exactly the
mechanism the driver's own `nvngx.dll` uses. Root cause is "wrong caller identity", so the fix is
"correct caller identity".

**Known non-issue documented**: when Neural Uplift is toggled off, the render-world `NrContext` (NGX feature +
params map) stays on the camera until it despawns — bounded to one stale feature per camera entity,
self-healing: re-enabling reuses it at matching size or recreates after a resize. No leak across AA/DLSS
camera respawns (despawn → `NrInner::drop` → ReleaseFeature).

### 2.12 Build-11 first-run findings: visual artifacts + device-lost crash, and the four fixes (build 12)

**Symptom A — "ugly and flashing" visuals.** NR was on (evaluate succeeded every frame) but looked wrong.
Root causes found by disasm + audit:
1. **Subrect params invisible to the runtime** (§2.2 slot map): build 11 wrote DepthSubrectWidth/Height via
   SetUI; the parser reads them through GetI → with SR active the runtime read 0 and sampled depth outside
   the valid subrect.
2. **No Reset on geometry change**: toggling SR or changing tier changes which depth region is valid; without
   `DLSSNR.Reset=1` for that frame, temporal history carries stale geometry across the change — flicker/
   ghosting exactly at mode switches (the user's "flashing").
3. **MVec omitted**: NULL MVec is tolerated by this build (backend eval @ 0x180018620 null-checks ONLY Color
   [rbp+0x20] and Output [rbp+0x68]; MVec/Depth pointers are read for logging only), but its motion semantics
   are undefined → the runtime may assume arbitrary motion. Fix: an explicit zero-filled stand-in = "no camera
   motion" (the RenoDX addon's contract is to always pass MVec).
4. **NR unordered vs tonemapping**: no constraint in the PostProcess set → the scheduler could run NR last,
   enhancing already-tonemapped LDR data instead of HDR scene data.

**Symptom B — `ERROR_DEVICE_LOST` panic on mode change.** Changing modes (AA/DLSS toggle or quality) triggers
the camera respawn (`apply_anti_aliasing_system`, settings.rs ~1530), which drops both dlss_wgpu's
`DlssSuperResolution` and our `NrInner`. Our release path called ReleaseFeature without waiting for in-flight
evaluate command buffers → the runtime freed internal resources that GPU commands still referenced (UAF) →
driver error → device lost; then dlss_wgpu's own Drop (`device_wait_idle().expect(…)`, super_resolution.rs:250
/ sdk.rs:102) panicked on the already-lost device. Fix: `wait_device_idle` before EVERY ReleaseFeature
(NrInner::drop + prepare_nr recreate path), panic-free, raw vk code logged — mirroring dlss_wgpu's ordering
without its `.expect`.

**The four fixes + three extras (all in source; build 12 = their first clean compile):**
- **Bug 1 — stale params**: `evaluate_nr` now writes every param unconditionally each frame. Subrects always
  set (full texture size when no SR override, render resolution when present). When depth is unavailable
  (MSAA), new `clear_void_pointer("DLSSNR.Depth")` stores NULL instead of leaving a stale pointer to a
  possibly-freed texture.
- **Bug 2 — no wait-idle before release**: `wait_device_idle(&wgpu::Device) -> Result<(), i32>` in
  kuluu-dlss-nr (as_hal → raw_device().device_wait_idle(); returns the raw vk code, never panics;
  non-Vulkan sentinel `WAIT_IDLE_NOT_VULKAN`). Called in NrInner::drop() before release_feature and on the
  prepare_nr recreate path.
- **Bug 3 — no Reset**: `evaluate_nr` takes `reset: bool`; nr_node tracks `last_depth_sig =
  (has_depth, sub_w, sub_h)` and sends reset=1 when it changes or on the first frame after create;
  steady-state frames send 0. Always explicit via set_i so a stale 1 cannot linger.
- **Bug 4 — MVec stand-in**: zero-filled Rg16Float texture (= bevy's `MOTION_VECTOR_PREPASS_FORMAT`,
  bevy_core_pipeline prepass/mod.rs:59) created in prepare_nr alongside the feature, cleared exactly once;
  stored as `mvec_view` on NrInner (keeps its texture alive); passed every frame + barriered to shader-read.
  Build-12 first-run note: that one-time clear initially shared the create encoder and hit wgpu-core's
  no-mixing panic (§2.9 item 11) — build 13 gives it a dedicated high-level-only encoder, submitted first.
- **Extra A — SetI vs SetUI**: subrects/Reset now via set_i (§2.2 slot map).
- **Extra B — ping-pong contract**: `preserve_main_after_flip()` copies source→destination on the shared
  encoder when a frame flipped via post_process_write() but failed to encode evaluate (all three failure
  paths) — prevents a permanent garbage screen if evaluate starts failing persistently.
- **Extra C — ordering**: nr_node is now `.before(bevy::core_pipeline::tonemapping::tonemapping)` so NR
  enhances HDR data before the LDR conversion.

**Known limitation (open):** the zero-MVec stand-in makes "no motion" explicit but does NOT fix real
camera-motion misalignment. If flicker persists while moving the camera: fallback = Reset-every-frame (kills
temporal accumulation → pure spatial) or wiring bevy's actual MotionVectorPrepass when SR is active.

---

## 3. What is needed to make NR work — IMPLEMENTED this session

### 3.1 Architecture as built
```
main world                          render world
─────────────                       ────────────
GraphicsSettings                    NrState (resource, lazy)
  neural_uplift / nr_* knobs          ├─ runtime: Option<NrRuntime>   (LoadLibraryW once)
        │ apply_neural_uplift_system    ├─ handles: VulkanHandles      (as_hal extraction once)
        ▼ (every frame; self-heals     └─ initialized: bool           (Init_Ext once, retry ≤1/s)
          across camera respawns)
OperatorCamera + NrEnabled{intensity,tone,structure}   ← ExtractComponentPlugin extracts it
        │
        ▼  prepare_nr (Render/PrepareViews, before prepare_view_targets)
            ├─ usages |= STORAGE_BINDING on main texture
            ├─ create zero-filled Rg16Float MVec stand-in alongside the feature (cleared once)
            └─ create/recreate feature 0x12 at full window res (own encoder + queue.submit;
               wait_device_idle THEN release old first — ReleaseFeature is handle-only in this ABI)
        ▼  nr_node (Core3d/PostProcess, after SR's EarlyPostProcess; .before(tonemapping))
            ├─ Color = main_texture_view() (pre-flip), MVec = stand-in view,
            │   Depth = prepass depth if sample_count==1 (+ SubrectWidth/Height always: render res when
            │   MainPassResolutionOverride present, full size otherwise)
            ├─ post_process_write() → Output = destination
            ├─ Reset=1 when (has_depth, sub_w, sub_h) changed or first frame after create; else 0
            ├─ barriers on shared encoder (incl. MVec→shader-read); EvaluateFeature in own command buffer;
            │   preserve_main_after_flip() copy if the flip happened but evaluate failed to encode
            └─ ctx.add_command_buffer(cb)
```

### 3.2 The FFI crate (`kuluu-dlss-nr/`) — finished, all 5 known issues from last handoff fixed
- Hand-declared C ABI types: `NvngxHandle{ptr:u64}` (OPAQUE object pointer — §2.1; build 10's `{id:u32}`
  was the per-frame InvalidParameter bug), `NvngxResourceVk` (repr(C), layout documented in comments:
  union-as-ImageViewInfo, Type@+0x30, ReadWrite@+0x34, size 0x38) + `image_view()` ctor +
  **`from_texture_view(view, adapter)`** mirroring dlss_wgpu's `texture_to_ngx` (aspect via
  `format.has_color_aspect()`, read_write via STORAGE_BINDING usage).
- externs for static-lib symbols (`AllocateParameters/DestroyParameters`, `Parameter_Set{VoidPointer,I,UI,F}`) —
  resolved at final link from nvsdk_ngx_s.lib via dlss_wgpu (crate only builds under kuluu-render's `dlss` feature).
- `NrParams` guard (allocate/Drop→destroy; Send+Sync documented).
- Runtime loader: `LoadLibraryW("nvngx_dlssnr.dll")` + GetProcAddress ×5; missing DLL → None.
- `VulkanHandles::from_wgpu(device)` — **fixed** to the verified ash 0.38 shapes (§2.4).
- `create_nr_feature()` fills the exact addon create-param set: SetUI Width/Height/OutWidth/OutHeight + all
  DLSSNR.* dimension aliases, SetI DLSSNR.Upscaling=1, SetF ScalingRatio/Scale=1.0 (native-res pass), and
  **CreationNodeMask=1 / VisibilityNodeMask=1** (critical: without these the feature creates but does nothing).
- `evaluate_nr(cmd, handle, params, color, mvec, depth: Option<…>, output, intensity, local_tone,
  structure, depth_inverted, valid_subrect_w/h, reset)` — build-12 contract (§2.12): writes EVERY param
  unconditionally (the map persists across frames; conditional writes leave stale values). Color + MVec stand-in
  always set; Depth set or withdrawn via `clear_void_pointer` (NULL store) when unavailable (MSAA); subrects
  always explicit (full size without SR, render res with it) and through **set_i** (§2.2 slot map — SetUI is
  invisible to the parser for these keys); Enabled/Reset/DepthInverted via set_i; knobs via set_f.
- `wait_device_idle(&wgpu::Device) -> Result<(), i32>` (build 12): as_hal → raw device_wait_idle, raw vk code on
  failure, never panics (`WAIT_IDLE_NOT_VULKAN` sentinel for the impossible non-Vulkan case). Callers run it
  before every ReleaseFeature.
- `init()` no longer leaks; `release_feature(handle)` is handle-only (C signature); `NrRuntime` is Clone+Copy
  and stored in the per-camera context so nothing reloads the DLL per frame.

### 3.3 Settings/menu (`kuluu-render/src/graphics/settings.rs`) — finished
- New persisted fields: `neural_uplift: bool` (def false), `nr_intensity: f32` (def **1.01** = addon default;
  parser's 1.0 reads as "no effect"), `nr_local_tone_strength` / `nr_structure_strength` (def 1.0) — all
  `#[serde(default)]`, in all preset arms, carried over untouched by preset cycling ("presets never own DLSS").
- New rows: `DlssNrIntensity` ("NR Intensity"), `DlssNrLocalTone` ("Local Tone Strength"),
  `DlssNrStructure` ("Structure Strength") after Neural Uplift in `DLSS_CONFIG_FIELDS`.
- **Neural Uplift left `is_dlss_placeholder()`** — live value_label (On/Off when supported, N/A otherwise) and
  cycle arms; knobs show values + cycle while supported. Slots: intensity `[0.5, 0.75, 1.0, 1.01, 1.25, 1.5, 2.0]`,
  tone/structure `[0.0, 0.5, 1.0, 1.5, 2.0]`. `reset_dlss_config()` resets all four NR fields (SR on/off stays put).
- **AA cycler cleanup DONE** (user request): `AA_SLOTS_DLSS` deleted entirely; the AntiAliasing row cycles plain
  AA slots only — DLSS is reachable solely via its explicit On/Off row. Test updated to assert Taa→Off wrap.
- New gate helper `nr_active() = neural_uplift && dlss_supported`; `apply_camera_prepass_system`'s keep_depth now
  includes NR so the DepthPrepass survives when only NR is on.
- Tests: `dlss_placeholders_stay_inert` auto-adapts (Neural Uplift no longer in the set); new
  `neural_uplift_rows_are_live_when_supported` covers toggle/knobs/preset-carry/reset/unsupported-json.

### 3.4 Registration & staging — finished
- `graphics/mod.rs`: `pub mod dlss_nr;` under the same cfg as `dlss`.
- kuluu-render Cargo.toml: optional dep `kuluu-dlss-nr` (native-only target section), wired into
  `dlss = ["bevy/dlss", "dep:kuluu-dlss-nr"]`; root workspace members + default-members updated.
- ViewerCorePlugin::build calls `graphics::dlss_nr::register(app)` under the feature gate.
- `nvngx_dlssnr.dll` staged at repo root + target/{debug,release}; build_cowland.bat auto-stages when missing.

### 3.5 The forwarder crate (`kuluu-ngx-fwd/`) — build 9, code written this session

**Shape.** `cdylib`, no dependencies, std only (~140 lines). Exports:

```
kuluu_ngx_fwd_vulkan_init_ext (target, app_id, data_path, instance, pd, device, version, params) -> NGX_Result
kuluu_ngx_fwd_vulkan_init_ext2(target, app_id, data_path, instance, pd, device, gipa, gdpa, version, params)
kuluu_ngx_fwd_abi_version() -> 1
```

`target` is the real `NVSDK_NGX_VULKAN_Init_Ext` pointer that kuluu-dlss-nr already resolved. The forwarder
does not load anything itself: kuluu-dlss-nr stays the single owner of the `nvngx_dlssnr.dll` module handle
and error path. The forwarder's only job is to be the module in which the `call` instruction lives.

**The one thing that can silently break it.** The call MUST compile to a real `call`, never a tail `jmp`. A
`jmp` leaves kuluu.exe's return address on the stack and the gate fails exactly as before. The result goes
through `std::hint::black_box` so the call is never in tail position at any opt level. If you ever touch that
function, keep that.

**Naming.** Cargo builds it as `kuluu_ngx_fwd.dll` (lib names can't contain `.`). `build_cowland.bat` copies it
next to kuluu.exe as **`nvngx.dll_kuluu.dll`**. Do NOT name it plain `nvngx.dll`: `nvsdk_ngx_s.lib` loads the
driver's real `nvngx.dll` for SR and a same-named file next to the exe is a shadowing risk we don't need.

**Return codes seen by kuluu-dlss-nr:**
| code | meaning |
|---|---|
| `0x1` | success, NR init done |
| `0xBAD0_0002` | still module-gated on THAT call: forwarder missing/stale next to the exe (build_cowland.bat re-stages it every build) — applies to Init_Ext, CreateFeature AND ReleaseFeature (§2.10 scope map) |
| `0xF0F0_0001` | forwarder got a null target (our load-order bug) |
| any other `0xBAD*` | real NGX error, gate passed; see §4 items 1–3 |

**Workspace wiring.**
- Root `Cargo.toml`: `"kuluu-ngx-fwd"` in `members` and `default-members` (next to `kuluu-dlss-nr`).
- No dependency edge from kuluu-render or kuluu-dlss-nr; it is loaded at runtime like the NR DLL — so
  **build_cowland.bat builds it explicitly** (`cargo build -p kuluu-ngx-fwd`, same profile as the main build);
  `cargo build -p kuluu` alone would never compile it.
- `kuluu-dlss-nr`: new `FORWARDER_DLL`/`FORWARDER_ABI` consts, forwarder fn-ptr types (declared with OUR
  existing `FnInitExt`/`NvngxResult`/`NvngxParameter` types — ABI-identical to the forwarder's u32/*const
declaration on x64), `LoadError::ForwarderAbi { found, expected }`, `NrRuntime.fwd_init_ext` (stays
  Clone+Copy), and `load_forwarder(exe_dir)` called from `load()` after the five NR symbols resolve.
- `init()` keeps its existing signature (`app_id: u64, data_path: &str, handles: &VulkanHandles,
  params: Option<&NrParams>`) — only the call site changed: it now calls through `fwd_init_ext` with
  `Some(self.init_ext)` as target. dlss_nr.rs's log lines are unchanged except success now says "(via
  forwarder)".

**Staging snippet (historical — was added to build-dlss.bat; current equivalent lives in build_cowland.bat):**
```bat
rem Forwarder: built as kuluu_ngx_fwd.dll, staged under a name containing "nvngx.dll"
if /i "%~1"=="debug" ( set "PROFILE=debug" ) else ( set "PROFILE=release" )
if not exist "target\%PROFILE%\kuluu_ngx_fwd.dll" ( echo [build-dlss] forwarder missing: target\%PROFILE%\kuluu_ngx_fwd.dll & exit /b 1 )
copy /Y "target\%PROFILE%\kuluu_ngx_fwd.dll" "%~dp0nvngx.dll_kuluu.dll" >nul
copy /Y "target\%PROFILE%\kuluu_ngx_fwd.dll" "target\%PROFILE%\nvngx.dll_kuluu.dll" >nul
```
Unlike the NR DLL, ALWAYS re-copy (ours; changes with every build). The cargo lines also gained a second
`-p kuluu-ngx-fwd` build in each branch.

**`.git/info/exclude` addition:** `/nvngx.dll_*.dll` (the crate source IS committed; only the renamed staged
copy is excluded).

**Verification (build 9), in order:**
1. `.\build_cowland.bat` (no arg) → `kuluu_ngx_fwd.dll` appears in `target\release\`, staged copy
   `nvngx.dll_kuluu.dll` at repo root + target/release. `objdump -p nvngx.dll_kuluu.dll | grep kuluu_ngx_fwd`
   shows the three exports.
2. `.\play_cowland.bat` → log shows `dlss-nr: forwarder v1 loaded (nvngx.dll_kuluu.dll)` then
   `Init_Ext succeeded (via forwarder)`, then toggle Neural Uplift on → `feature created at WxH`.
3. If `0xBAD0_0002` persists: objdump the forwarder, confirm `kuluu_ngx_fwd_vulkan_init_ext` contains a
   `call rax`/`call r..` and not a tail `jmp`. If it is a call and the gate still fails, the gate has a
   second condition — re-read §2.10 step 3.
4. Anything else `0xBAD*` → gate is behind us; continue with §4 (HDR input format first).

### 3.6 How it works, end to end (build 9)

**Startup — once per process, lazy on the first frame where Neural Uplift is supported:**
1. `NrState::ensure_initialized` → `NrRuntime::load()`:
   - `LoadLibraryW(<exe dir>\nvngx_dlssnr.dll)` by absolute path; NULL → log names the DLL + win32 code.
   - GetProcAddress ×5 (CString names — build 8's fix): Init_Ext, CreateFeature, EvaluateFeature,
     ReleaseFeature, Shutdown1.
   - `load_forwarder`: `LoadLibraryW(<exe dir>\nvngx.dll_kuluu.dll)`; then
     `kuluu_ngx_fwd_abi_version()` must return 1 (stale-copy guard) and
     `kuluu_ngx_fwd_vulkan_init_ext` is resolved.
   - Log: `dlss-nr: loaded nvngx_dlssnr.dll + forwarder v1 (nvngx.dll_kuluu.dll)`.
2. `VulkanHandles::from_wgpu` — raw instance/pd/device u64s from the wgpu HAL, extracted once.
3. `runtime.init(app_id, temp_dir, handles, None)` calls the **forwarder**, not the NR DLL:
   - kuluu.exe does `call fwd_init_ext(Some(init_ext), …)` — that return address sits inside
     nvngx.dll_kuluu.dll.
   - The forwarder does `call target(…)` — a real call (the black_box'd result keeps it out of tail
     position, so it can never become a jmp); its return address is ALSO inside nvngx.dll_kuluu.dll.
   - NR runtime prologue: `GetModuleHandleExW(FROM_ADDRESS|UNCHANGED_REFCOUNT, retaddr)` → module =
     nvngx.dll_kuluu.dll → file name contains "nvngx.dll" (case-insensitive substring) → gate passes →
     real init runs.
   - Log: `dlss-nr: Init_Ext succeeded via forwarder (app id …, data path …)`.

**Per frame — Neural Uplift on:**
4. `prepare_nr` (PrepareViews): main texture usages |= STORAGE_BINDING; create/recreate feature 0x12 at
   full window res — **via the forwarder's CreateFeature trampoline** (gated like Init_Ext, §2.10).
5. `nr_node` (Core3d/PostProcess, after SR's EarlyPostProcess): Color = current main view, Depth = prepass
   depth when sample_count==1 (+ SubrectWidth/Height while SR renders at sub-res), Output =
   post_process_write destination; barriers on the shared encoder; EvaluateFeature encoded into its own
   command buffer → `ctx.add_command_buffer`. **Direct call** (ungated).
6. Toggle off / camera despawn: ReleaseFeature (handle-only, **via the forwarder's trampoline** — it is
   gated too); Shutdown1 not called today (gated; would need a trampoline before use).

**Why steps 3/4/6 are indirect:** the gate lives in every exported prologue EXCEPT EvaluateFeature (§2.10
scope map) — each gated export carries its own copy of the same caller-module check, so Init_Ext (step 3),
CreateFeature (step 4) and ReleaseFeature (step 6) all need a trampoline; only the per-frame hot path,
EvaluateFeature (step 5), is verified ungated in v310.8 and stays a direct call.

---

## 4. Open questions / risks (short list)
1. **HDR input format**: kuluu's operator camera is Hdr → main texture likely BgraFloat32. Unknown whether the
   v310.8 NR parser accepts 32F color input; the addon has an "HDR transfer" control, suggesting SDR preference is
   possible. Plan: try as-is first — SL logs will show `UnsupportedInputFormat` (0xBAD0_0008) if not → fallback =
   add an HDR→LDR copy pass before NR. **§7.4 adds a second darkening suspect: our Local Tone default of 1.0 is
   the MAXIMUM of the official 0–1 Tone Intensity range — test Local Tone → 0 first (cheap, no code).**
1b. **MVec — RESOLVED in build 12 (§2.12 Bug 4)**: Bevy produces no motion-vector texture for our camera, so
   evaluate sends a zero-filled Rg16Float stand-in (= bevy's `MOTION_VECTOR_PREPASS_FORMAT`) sized to the input,
   created alongside the feature and cleared once — an explicit "no camera motion" instead of NULL (NULL is
   tolerated by this build: eval null-checks only Color/Output, but MVec semantics with NULL are undefined).
   OPEN LIMITATION: a zero stand-in does not fix real camera-motion misalignment; if flicker persists while the
   camera moves → Reset-every-frame (pure spatial) or wire bevy's actual MotionVectorPrepass when SR is active.
   **§7.3 (post-launch research): NVIDIA confirmed motion vectors are one of only TWO runtime inputs — this
   stand-in is now the prime suspect for any motion-related artifact.**
   (Build-11 history: MVec was omitted then — verified safe at both layers, parser @ 0x19F30 stores NULL for a
   missing MVec and continues; backend eval @ 0x180018620 null-checks ONLY Color (+0x20) and Output (+0x68).)

2. **Vulkan extension requirements for feature 0x12**: if NR needs instance extensions wgpu didn't enable, init or
   create will fail with a 0xBAD* result in the logs; expectation is none/already-covered (SR works today through
   the same device).
3. **First-frame ordering**: NrEnabled extraction lands one frame after the toggle; feature creation happens in
   that frame's prepare — expect ~1-2 frames of no effect on enable, then steady state.
4. **Runtime diagnosis CLOSED (§2.9 item 8)**: GetProcAddress NULL was our own fault — `resolve!` passed an
   unterminated Rust literal (`str::as_ptr()` has no NUL; C strings do). Fixed in build 8's source:
   CString-based lookup + GetLastError on miss. Remaining unknowns are now purely NGX-side (items 1–3):
   HDR input acceptance, extension requirements, first-frame ordering. Build 13 is already compiled + synced — just run `.\play_cowland.bat` (§5).


## 5. Verification plan (user runs) — build 13
> **Launchers renamed.** The personal launchers referenced below have moved: `build-dlss.bat` / `dlssplay.bat`
> are deleted; the current equivalents are `build_cowland.bat` (build + stage everything) and
> `play_cowland.bat` (run), both at repo root, gitignored. Build + DLL-staging steps live in Part 1
> ("Building with DLSS" / "Running / distributing") and inside build_cowland.bat. The numbered checks below
> still describe what to look for in the SL logs / in-game; just run the built exe via play_cowland.bat.
(PowerShell: prefix with `.\` — it won't run batch files from the current dir otherwise.)
**The exe is already built + synced** (build 13's kuluu.exe was hash-verified against target/release, forwarder
re-staged). No rebuild needed unless source changes.
1. `.\play_cowland.bat`; check SL logs (`streamline/logs/`) for:
   - `dlss-nr: loaded nvngx_dlssnr.dll + forwarder`, `Init_Ext succeeded via forwarder`,
     `feature created at WxH (handle 0x…)`;
   - toggle Neural Uplift on (DLSS Config submenu) → **zero** per-frame `EvaluateFeature failed` warnings.
2. **Crash regression test (the build-11 crash)**: with NR on, repeatedly change modes — AA off/TAA/DLSS
   toggles, DLSS quality-tier changes, window resizes. Build 11 panicked here (`ERROR_DEVICE_LOST` in
   dlss_wgpu's drop paths). Expected: no panic; at most a `dlss-nr: device not idle before feature
   release/recreate (vk result …)` warning if the wait ever races.
3. **Visual test**: NR on vs off with NR Intensity ≥1.5 → visible difference expected. Test with DLSS SR on AND
   off and at a non-default quality tier (exercises the depth-subrect path — now sent via set_i under the no-dot
   names, so it is actually visible to the runtime this time).
4. **Specifics from build 11** (answer these if anything still looks wrong): which exact mode change crashed it;
   did the flashing happen with SR off too; screenshot of any remaining "ugly" state.
5. If evaluate still fails: new result code + SL log lines are the next evidence (InvalidParameter = a
   parameter problem now, not the handle; MissingInput 0xBAD0_000A ⇒ Color/Output resource issue;
   UnsupportedInputFormat 0xBAD0_0008 ⇒ §4 item 1 HDR fallback).
6. If flicker persists specifically while moving the camera: that is the zero-MVec stand-in limitation (§2.12
   known limitation) → try Reset-every-frame to confirm temporal state is the culprit; next step = bevy's real
   MotionVectorPrepass when SR is active.

## 6. File map (quick reference)
```
dlss_docs/DLSS.md                   ← this doc (merged from docs/DLSS.md + ffxi_dlss5.md)
build_cowland.bat / play_cowland.bat personal launchers (repo root, gitignored); build_cowland stages all runtime DLLs
.git/info/exclude                    personal exclusions (/streamline/, /nvngx_*.dll, /nvngx.dll_*.dll, /sl.*.dll, both .bat)
nvngx_dlssnr.dll                     staged at repo root + target/{debug,release}/ (excluded via /nvngx_*.dll)
nvngx.dll_kuluu.dll                  forwarder staged copy (repo root + target/{debug,release}; excluded via /nvngx.dll_*.dll)
streamline/                          all personal DLSS material (excluded as a whole folder)
  nvngx_dlssnr.dll                   NR runtime v310.8.0 (~166 MB, source of the staged copies)
  sl.dlss_nr.dll                     Streamline NR plugin (names only, no layout) — not needed by us directly
  renodx-dlss5.addon64               reference D3D12 addon
  dlss5-feeder/                      cloned jlrouzies-fr/DLSS5-Feeder
  sdk/                               NVIDIA DLSS SDK v310.5.3 (headers + static libs)
    lib/Windows_x86_64/x64/nvsdk_ngx_s.lib   ← what dlss_wgpu links; exports VULKAN_AllocateParameters ✅
  logs/addon_disasm.txt              addon disasm (NR create path @ ~0x18001a79a–0x18001aa44)
kuluu-ngx-fwd/                       forwarder cdylib (zero deps; staged as nvngx.dll_kuluu.dll) — committed
kuluu-dlss-nr/                       FFI crate (~700 lines incl. forwarder routing in load()/init()) — committed
kuluu-render/src/graphics/dlss_nr.rs pipeline hook (~490 lines) — committed
kuluu-render/src/graphics/settings.rs  NR rows live; AA cycler cleaned up; nr_active() — committed
kuluu-render/src/graphics/mod.rs       dlss_nr module decl — committed
kuluu-render/Cargo.toml              optional kuluu-dlss-nr dep + feature wiring — committed
kuluu-render/src/lib.rs              ViewerCorePlugin register() call — committed
Cargo.toml                           workspace members/default-members — committed
kuluu/src/view_native/mod.rs         duplicate-plugin fix (no manual DlssInitPlugin) — committed
kuluu-render/src/graphics/dlss.rs    project id + capability probe docs — committed
~/.cargo/.../dlss_wgpu-4.0.0/src/{nvsdk_ngx,sd,super_resolution}.rs      FFI template (texture_to_ngx @ nvsdk_ngx.rs:202)
~/.cargo/.../bevy_anti_alias-0.19.1/src/dlss/{mod,node,prepare,extract}.rs  pipeline hook templates
```

---

## 7. Official DLSS 5 knowledge (researched post-launch, 2026-09-04)

NVIDIA released DLSS 5 on **September 3, 2026** (debut title: NBA 2K27; available via the
Game Ready driver + GeForce NOW). What they published, and what it means for our integration:

### 7.1 What NVIDIA actually published
- **Research page + paper**: [DLSS 5: Generative Neural Rendering](https://research.nvidia.com/labs/adlr/DLSS5)
  (NVIDIA ADLR, published Sept 1, 2026). The PDF (`files/DLSS5_Report.pdf`, ~293 MB — mostly
  embedded media; the abstract is the useful part) states: DLSS 5 is "a real-time generative
  rendering stage" using **3D-guided neural rendering** — a **one-step pixel-space diffusion model**
  conditioned at inference on *the current rendered frame, engine motion vectors, carried temporal
  state, and artistic-direction values*; trained with consistency supervision from renderer-derived
  scene attributes; causal + deterministic inference; strict per-frame compute budget; real-time up to
  4K. "First DLSS technology to generate the final displayed appearance rather than reconstruct a
  higher-cost reference output." Runs locally as a rendering stage on **GeForce RTX 50 Series**.
- **Gamescom technical briefing** (Edward Liu, DLSS research lead + Gabriele Leone, content tech
director), covered in detail by [TechPowerUp's review](https://www.techpowerup.com/review/nvidia-dlss-5-technical-preview/)
  (Sept 1, 2026). This is the best public source on runtime behavior — quotes below are from it.
- **Launch coverage**: release date + RTX 50-only confirmation ([pcmasterinsider](https://pcmasterinsider.com/nvidia-dlss-5-release-date)).

### 7.2 Developer docs status (answers "does NVIDIA release how these things work?")
**No developer-facing NR integration doc exists as of launch day.** The public Streamline SDK is still
at **v2.12.0 (June 23, 2026)** — released *before* DLSS 5; its notes are "Bug Fixes & Stability
Improvements" and the repo docs/feature list cover SR / RR / MFG only ([releases](https://github.com/NVIDIA-RTX/Streamline/releases),
[dev page](https://developer.nvidia.com/rtx/dlss)). The public NGX feature enum (our local SDK v310.5.3
headers) ends at `RayReconstruction = 13`; NR is the undocumented **feature 0x12 (= 18)** we already
target — community logs call it "feature 18" too. Consequences:
- Our direct-Vulkan-NGX path (`nvngx_dlssnr.dll` + forwarder, §2.10–§3.5) remains the only way in;
  nothing to migrate to yet.
- **Watch `NVIDIA-RTX/Streamline` releases** for an NR plugin + programming guide — that would replace
  our reverse-engineered parameter names (§2.2) with a supported ABI, and is the first thing to check
  after any driver/SDK update.
- The RenoDX/OptiScaler community addons (D3D12 detours) are the only other public integrations; their
  logs remain useful ground truth for parameter behavior (§2.5).

### 7.3 Runtime inputs — the big one for us
From the briefing: **at runtime the model receives exactly two things — the rendered output frame and
the motion vectors. Nothing else.** Depth, normals, albedo, lighting semantics are used *during training
only* (consistency supervision). TPU: "a sharp break from the rest of the DLSS family" — RR wants 20+
engine buffers, SR needs color+depth+MV; DLSS 5 needs color+MV. Also confirmed:
- **Runs at native output resolution** — asked whether it runs at reduced internal res: "just native,
  the output resolution of DLSS." (Matches our full-window feature creation.)
- **Placement: end of pipeline, after all SR upscaling outputs; Frame Generation runs immediately
  after.** Our `nr_node` sits in PostProcess after EarlyPostProcess where SR lives — correct placement.
- Works on any input (pure raster, native res, upscale/downscale); no other DLSS tech required. RT input
  gives a better baseline (contact shadows/scattering) but is not needed.
- **Garbage in, garbage out**: TAA/SR pixelation and ghosting are reproduced *pixel for pixel*; low-res
  geometry/textures stay low-res, just less photoreal.
- Geometry preservation: the model adds lighting/material response on top of a frame but "cannot touch
  the geometry underneath" — silhouettes/normals/shapes come back as drawn.

**Implication (validates §4 item 1b):** our zero-filled MVec stand-in is now confirmed as the prime
suspect for any motion-related artifact. Temporal stability is *trained on* real motion vectors; feeding
"no motion" every frame while the camera moves means the temporal state cannot track the scene.
Fix path unchanged: bevy's `MotionVectorPrepass` when SR is active (or a depth+camera TAAU-style pass),
then drop the stand-in. Until then, expect shimmer/ghosting specifically during camera movement — that
is the documented limitation, not a new bug.

### 7.4 Official controls vs our knobs
| Official (launch) | Semantics | Ours (v310.8 runtime) |
|---|---|---|
| **Structure Intensity** 0–1 | high-frequency uplift: AO, contact shadows, reflections, SSS | `DLSSNR.LocalStructureStrength` (`nr_structure_strength`, def 1.0) |
| **Tone Intensity** 0–1 | low-frequency uplift: broad lighting + color response; **at 0 the output is identical to the rendered frame for that component** | `DLSSNR.LocalToneStrength` (`nr_local_tone_strength`, def 1.0) |
| (global strength) | overall effect amount | `DLSSNR.Intensity` (`nr_intensity`, def 1.01 = addon default; parser default 1.0 ≈ no-op) |
| Models A/B/C | three weight sets, visibly different, **no perf difference**, switchable per scene/cutscene | not exposed in v310.8 params we know of — check for a model-select key on next disasm pass |
| Masking (auto + developer groups) | both feed ONE screen-space control buffer; **zero cost to the model** regardless of content | no mask input seen in our param contract (§2.2) — likely driver-side only at this stage |

Exact semantic equivalence between v310.8's `Local*` names and launch naming is unverified (the runtime
predates the public release), but the structure/tone split matches 1:1.

**Implication for "textures darker" (user symptom #3):** Tone Intensity governs low-frequency lighting/
color response, and our default `nr_local_tone_strength = 1.0` is the *maximum* of the official 0–1
range — i.e. full tone uplift by default. Two concrete tests:
1. Set **Local Tone Strength → 0** (keep Structure > 0): if the darkening disappears, it is the tone
   component re-lighting low frequencies; ship a lower default or document the knob.
2. The §4 item-1 HDR question still applies: the model was trained on renderer-derived attributes; if we
   feed post-tonemap LDR where it expects linear HDR (or vice versa), "low-frequency lighting" means
   something different and can read as a global darkening/brightening shift.

### 7.5 Performance — official numbers, set expectations accordingly
- **Cost: ~50–60% of frame rate on average across RTX 50 GPUs at all resolutions** (NVIDIA's own answer,
  measured in NBA 2K27). "No per-game tuning — the model is the model." This is a design property, not a
  bug: NR at full window resolution halving FPS is *expected* on any card below a top-end 5090.
- NVIDIA offsets it with MFG + SR in their own charts (4K/5090: 370 fps ≈ 62 *rendered* frames with
  MFG 6X). Community corollary: **DLAA + NR fight over the same Tensor cores** — DLSS Quality mode + NR
  measured better than DLAA + NR (Stellar Blade, 3090: 73 → 80 fps).
- The model has been made ~5x faster since development began (started needing dual RTX 5090s) — expect
  driver-side cost to keep dropping.
- Community levers we do NOT have officially: OptiScaler's "Model Resolution" control runs the model
  below output resolution (the only real cost lever found so far); and early-driver builds had a **VRAM
  leak when toggling the NR checkbox on/off** (workaround there: slide intensity 0–2 instead of
  toggling). Our pipeline releases/recreates the feature on mode changes (§3.1) — worth watching SL logs
  for VRAM growth across repeated Neural Uplift toggles in `play_cowland.bat` sessions.
- **RTX 50 only, confirmed by NVIDIA ("Yes, it")** — Blackwell FP4 Tensor cores; the leaked DLL's
  explicit runtime gate for unsupported architectures (§2.10–§2.11 module-gate findings) turned out to be
  the shipping behavior. Modders have forced it on Ada/Ampere with patched DLLs (FP8→FP16 pipeline);
  not a path we take.

### 7.6 Sources
- https://research.nvidia.com/labs/adlr/DLSS5 (official abstract; paper PDF behind S3, geo-blocked from this host)
- https://www.techpowerup.com/review/nvidia-dlss-5-technical-preview/ (Gamescom briefing coverage, all 7 pages)
- https://pcmasterinsider.com/nvidia-dlss-5-release-date (launch date / hardware scope)
- https://github.com/NVIDIA-RTX/Streamline/releases (SDK version status as of 2026-09-04)
- Community: NexusMods Stellar Blade DLSS5 thread (nvngx_dlssnr.dll behavior, OptiScaler, toggle VRAM leak — Sept 1–2, 2026)

---

## Build history (builds 1–13, newest first)

**STATUS (build 13, compiled clean):** build 12's four fixes (§2.12) were in source and compiled; the user tested it and hit one new panic: wgpu-core 29 forbids mixing high-level + raw encoding on a single CommandEncoder (EncodingApi locks on first use — §2.9 item 11), which my MVec clear pass did against the create encoder. Fixed with two encoders (clear = high-level-only, submitted first; create = raw-only) and recompiled: build 13 = clean release build of all fixes. The exe is already built + synced; no rebuild needed unless source changes.

**STATUS (build 11, historical): ROOT CAUSE OF PER-FRAME EvaluateFeature InvalidParameter FOUND + FIXED IN SOURCE — handle ABI mismatch (§2.1). The v310.8 NR runtime uses an OPAQUE 64-bit object-pointer handle, not the public header's `{ u32 Id }`; build 10 passed a pointer to our own 4-byte struct, so the runtime's FNV table lookup missed every frame. Fix: `NvngxHandle{ptr:u64}` opaque value, evaluate/release pass the stored value cast to a pointer; plus two parameter fixes from .rdata ground truth (no-dot subrect names `DLSSNR.DepthSubrectWidth/Height`; DepthInverted sign — we were sending 1 for bevy's non-inverted Vulkan depth where the parser default/addon both use 1 = D3D inverted). User tested build 11: evaluate succeeds every frame; two new problems found + fixed (§2.12).**

**STATUS (2026-10 session, build 9–10 history): forwarder defeats the module gate (§2.10–§2.11, §3.5; round errors in §2.9 item 9).**
Builds 1–5 = compile-error rounds (§2.9 items 3–6), all fixed. Build 6 = CLEAN COMPILE + LINK (first full
binary); first run: game works, DLSS SR active at DLAA, but the NR DLL load failed — `GetProcAddress`
returned NULL for `NVSDK_NGX_VULKAN_Init_Ext` even though objdump proves the export exists by name and a
.NET P/Invoke probe resolves it fine from another process. Build 7 added GetLastError + absolute-path
loading; its first run still said "export missing" — which pointed at the lookup itself: **the `resolve!`
macro passed `$sym.as_ptr()` on a Rust string literal, which has NO NUL terminator** (Rust literals are not
C strings). GetProcAddress was searching for the symbol name plus whatever bytes follow it in .rodata until
some zero byte — i.e. a garbage name that matches no export (§2.9 item 8 = root cause + fix). Every other
string crossing in this crate already used `cstr()` (CString) correctly; only the loader macro was bare.
Fix applied: the macro now builds a `CString` first, and symbol-miss errors carry GetLastError (127 expected
on a real miss). Build 8's first run confirmed the fix (DLL loads, all five symbols resolve clean) and exposed
the real blocker: `Init_Ext` returns **PlatformError 0xBAD0_0002** — the v310.8 NR runtime gates every
`*_Init_Ext` entry on caller identity (`GetModuleHandleExW(6, retaddr, …)` + case-insensitive substring test
for "nvngx.dll" in the calling module's file name; kuluu.exe fails it deterministically — §2.10–§2.11).
**Build 9 = the forwarder (§3.5):** new crate `kuluu-ngx-fwd/` (cdylib, zero deps) staged next to the exe as
`nvngx.dll_kuluu.dll`; kuluu-dlss-nr routes ONLY the gated Init_Ext call through it so the return address
lands in a module whose name contains "nvngx.dll". Build 9's first run passed Init_Ext via the forwarder,
then `CreateFeature(0x12)` returned PlatformError — disasm showed the gate is NOT init-only: every export
except EvaluateFeature carries it (§2.10 scope map, corrected). **Build 10 = extended forwarder (ABI v2):**
trampolines for CreateFeature + ReleaseFeature; recreate attempts throttled to 1/s after a failed create;
`shutdown()` documented as gated (no trampoline yet — nothing calls it).
