//! NVIDIA DLSS 5 Neural Rendering ("Neural Uplift", a.k.a. DLSSNR) FFI.
//!
//! Drives `nvngx_dlssnr.dll`'s Vulkan NGX API directly — no ReShade, no D3D12
//! detours. This crate is the single home for every unsafe touch of that ABI;
//! everything above it (kuluu-render) stays safe code.
//!
//! # The parameter-map ABI (why this looks the way it does)
//! `NVSDK_NGX_Parameter*` is NOT a flat name/value array. It points at an
//! object whose first qword is a table of function pointers, called as
//! `fn(param_ptr, name_str, value_or_out)` — decoded from the SDK's static host
//! lib (`nvsdk_ngx_parameters_lib.obj`) and cross-checked against the v310.8
//! runtime parser (see ffxi_dlss5.md §2.3). Consequence: we never hand-build
//! its layout. We allocate through the host layer's own `AllocateParameters`
//! and fill it with its own `Set*` accessors, exactly what the RenoDX-DLSS5
//! addon does with its bundled copy of the same host layer.
//!
//! # Link sources
//! * **Static host layer** — `NVSDK_NGX_VULKAN_AllocateParameters/
//!   DestroyParameters` + `NVSDK_NGX_Parameter_Set*`: declared as externs here,
//!   resolved at final link from nvsdk_ngx_s.lib (linked by dlss_wgpu under
//!   bevy's `dlss` feature). Build this crate only in that configuration.
//! * **NR runtime** — `nvngx_dlssnr.dll`: loaded at runtime via LoadLibraryW
//!   from next to the executable. Missing => NR unavailable, SR unaffected.

#![allow(non_snake_case)] // C ABI names stay verbatim at the FFI boundary
#![allow(clippy::too_many_arguments)] // evaluate_nr carries the full per-frame knob set

use std::ffi::{c_char, c_int, c_void, CString};

// ash handle tuple fields are private; the public `Handle` trait is the only
// sanctioned way to read the raw u64 out of a vk::* handle.
use ash::vk::Handle;

// Windows-only path encoding for LoadLibraryW (this crate already assumes
// Win32 throughout — its externs and wgpu-hal Vulkan extraction are native).
use std::os::windows::ffi::OsStrExt;

// ---------------------------------------------------------------------------
// C ABI types (hand-declared; mirror nvsdk_ngx_defs.h / nvsdk_ngx_vk.h)
// ---------------------------------------------------------------------------

/// `NVSDK_NGX_Result` — a C enum. Note: success is **0x1**, not 0.
pub type NvngxResult = c_int;

pub const NGX_SUCCESS: NvngxResult = 0x1;

const FAIL_FEATURE_NOT_SUPPORTED: u32 = 0xBAD0_0001;
const FAIL_PLATFORM_ERROR: u32 = 0xBAD0_0002;
const FAIL_FEATURE_ALREADY_EXISTS: u32 = 0xBAD0_0003;
const FAIL_FEATURE_NOT_FOUND: u32 = 0xBAD0_0004;
const FAIL_INVALID_PARAMETER: u32 = 0xBAD0_0005;
const FAIL_SCRATCH_BUFFER_TOO_SMALL: u32 = 0xBAD0_0006;
const FAIL_NOT_INITIALIZED: u32 = 0xBAD0_0007;
const FAIL_UNSUPPORTED_INPUT_FORMAT: u32 = 0xBAD0_0008;
const FAIL_RW_FLAG_MISSING: u32 = 0xBAD0_0009;
const FAIL_MISSING_INPUT: u32 = 0xBAD0_000A;
const FAIL_UNABLE_TO_INITIALIZE_FEATURE: u32 = 0xBAD0_000B;
const FAIL_OUT_OF_DATE: u32 = 0xBAD0_000C;
const FAIL_OUT_OF_GPU_MEMORY: u32 = 0xBAD0_000D;
const FAIL_UNSUPPORTED_FORMAT: u32 = 0xBAD0_000E;

/// Sentinel from [`wait_device_idle`] when the device is not on the Vulkan
/// backend — no vk::Result covers that case; callers only log it.
pub const WAIT_IDLE_NOT_VULKAN: i32 = -1;

/// Short human-readable name for a result code (log lines).
pub fn result_name(r: NvngxResult) -> &'static str {
    match r as u32 {
        0x1 => "Success", // NGX_SUCCESS (a const cast is not a valid pattern)
        FAIL_FEATURE_NOT_SUPPORTED => "FeatureNotSupported",
        FAIL_PLATFORM_ERROR => "PlatformError",
        FAIL_FEATURE_ALREADY_EXISTS => "FeatureAlreadyExists",
        FAIL_FEATURE_NOT_FOUND => "FeatureNotFound",
        FAIL_INVALID_PARAMETER => "InvalidParameter",
        FAIL_SCRATCH_BUFFER_TOO_SMALL => "ScratchBufferTooSmall",
        FAIL_NOT_INITIALIZED => "NotInitialized",
        FAIL_UNSUPPORTED_INPUT_FORMAT => "UnsupportedInputFormat",
        FAIL_RW_FLAG_MISSING => "RWFlagMissing",
        FAIL_MISSING_INPUT => "MissingInput",
        FAIL_UNABLE_TO_INITIALIZE_FEATURE => "UnableToInitializeFeature",
        FAIL_OUT_OF_DATE => "OutOfDate",
        FAIL_OUT_OF_GPU_MEMORY => "OutOfGPUMemory",
        FAIL_UNSUPPORTED_FORMAT => "UnsupportedFormat",
        0xF0F0_0001 => "ForwarderNullTarget", // FWD_NULL_TARGET: forwarder got a null target
        _ => "Unknown",
    }
}

/// `NVSDK_NGX_Handle` for this NR runtime build: an OPAQUE object pointer, not
/// the public header's `{ unsigned int Id; }`. CreateFeature writes a full
/// 64-bit heap-object pointer into *OutHandle (the backend create stores it as
/// a qword @ 0x1800183AE); Evaluate/Release receive that exact value cast to a
/// pointer and read its first u32 as the FNV feature-table key. Callers never
/// interpret the value — they store it, pass it back verbatim, and zero it on
/// release.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NvngxHandle {
    /// The opaque object pointer CreateFeature wrote; 0 = empty/invalid.
    pub ptr: u64,
}

impl NvngxHandle {
    /// A handle is valid once the runtime has written a non-zero object pointer.
    pub fn is_empty(self) -> bool {
        self.ptr == 0
    }
}

/// Opaque parameter map (`NVSDK_NGX_Parameter*`). The pointee layout is owned
/// by the host layer (function-pointer table, see module docs); never
/// dereferenced here.
pub type NvngxParameter = *mut c_void;

/// `VkImageSubresourceRange` — five u32s, no padding.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VkImageSubresourceRange {
    pub aspect_mask: u32,
    pub base_mip_level: u32,
    pub level_count: u32,
    pub base_array_layer: u32,
    pub layer_count: u32,
}

/// `NVSDK_NGX_Resource_VK`. The C union is represented by its largest member
/// (ImageViewInfo); BufferInfo is a strict prefix of it. Layout:
/// ```text
///  +0x00 VkImageView            (u64)
///  +0x08 VkImage                (u64)
///  +0x10 VkImageSubresourceRange(5 x u32 = 20 bytes, ends at 0x28)
///  +0x24 VkFormat               (u32)
///  +0x28 Width                  (u32)
///  +0x2C Height                 (u32)   <- ImageViewInfo ends at 0x30
///  +0x30 Type                   (u32, enum: 0 = VK_IMAGEVIEW)
///  +0x34 ReadWrite              (bool)  <- struct size 0x38
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NvngxResourceVk {
    pub image_view: u64,
    pub image: u64,
    pub subresource_range: VkImageSubresourceRange,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    /// `NVSDK_NGX_RESOURCE_VK_TYPE_VK_IMAGEVIEW` (0).
    pub r#type: u32,
    /// True when the image carries VK_IMAGE_USAGE_STORAGE_BIT — the runtime
    /// refuses outputs without it (RWFlagMissing), so set it for any texture
    /// NGX writes.
    pub read_write: bool,
}

impl NvngxResourceVk {
    const RESOURCE_TYPE_VK_IMAGEVIEW: u32 = 0;

    /// Vulkan constants used when building subresource ranges / formats.
    pub const ASPECT_COLOR: u32 = 0x1;
    pub const ASPECT_DEPTH: u32 = 0x2;
    pub const REMAINING_MIPS: u32 = 0xFFFFFFFF;
    pub const REMAINING_LAYERS: u32 = 0xFFFFFFFF;

    /// `NVSDK_NGX_Create_ImageView_Resource_VK` (the SDK's static-inline
    /// helper, replicated verbatim — it is a plain struct fill).
    #[must_use]
    pub fn image_view(
        image_view: u64,
        image: u64,
        aspect_color: bool,
        format: u32,
        width: u32,
        height: u32,
        read_write: bool,
    ) -> Self {
        Self {
            image_view,
            image,
            subresource_range: VkImageSubresourceRange {
                aspect_mask: if aspect_color {
                    Self::ASPECT_COLOR
                } else {
                    Self::ASPECT_DEPTH
                },
                base_mip_level: 0,
                level_count: Self::REMAINING_MIPS,
                base_array_layer: 0,
                layer_count: Self::REMAINING_LAYERS,
            },
            format,
            width,
            height,
            r#type: Self::RESOURCE_TYPE_VK_IMAGEVIEW,
            read_write,
        }
    }

    /// Extracts raw Vulkan handles from a wgpu TextureView (Vulkan backend only).
    /// Mirrors dlss_wgpu's `texture_to_ngx`: ImageView + Image handles, aspect
    /// from the format, raw VkFormat via the adapter, and the read-write flag
    /// from STORAGE_BINDING usage. Returns None on any non-Vulkan setup — NR is
    /// then simply unavailable.
    #[must_use]
    pub fn from_texture_view(view: &wgpu::TextureView, adapter: &wgpu::Adapter) -> Option<Self> {
        use wgpu::hal::api::Vulkan;

        let texture = view.texture();
        // SAFETY: as_hal only reads the backend tag of an already-created
        // resource and returns its hal view; raw_handle is a plain field read
        // on the hal resource. No state is mutated.
        let (raw_view, raw_image) = unsafe {
            (
                view.as_hal::<Vulkan>()?.raw_handle().as_raw(),
                texture.as_hal::<Vulkan>()?.raw_handle().as_raw(),
            )
        };
        let format = unsafe { adapter.as_hal::<Vulkan>()? }
            .texture_format_as_raw(texture.format())
            .as_raw() as u32;

        Some(Self::image_view(
            raw_view,
            raw_image,
            texture.format().has_color_aspect(),
            format,
            texture.width(),
            texture.height(),
            texture
                .usage()
                .contains(wgpu::TextureUsages::STORAGE_BINDING),
        ))
    }
}

// ---------------------------------------------------------------------------
// Static host layer (resolved at final link from nvsdk_ngx_s.lib via dlss_wgpu)
// ---------------------------------------------------------------------------

extern "C" {
    fn NVSDK_NGX_VULKAN_AllocateParameters(out_params: *mut NvngxParameter) -> NvngxResult;
    fn NVSDK_NGX_VULKAN_DestroyParameters(params: NvngxParameter) -> NvngxResult;
    fn NVSDK_NGX_Parameter_SetVoidPointer(
        p: NvngxParameter,
        name: *const c_char,
        value: *mut c_void,
    );
    fn NVSDK_NGX_Parameter_SetI(p: NvngxParameter, name: *const c_char, value: c_int);
    fn NVSDK_NGX_Parameter_SetUI(p: NvngxParameter, name: *const c_char, value: u32);
    fn NVSDK_NGX_Parameter_SetF(p: NvngxParameter, name: *const c_char, value: f32);
}

/// An allocated NGX parameter map; destroyed exactly once on drop.
///
/// The map is a plain name/value container read by the runtime at call time —
/// no thread affinity of its own. Callers that share it across threads must do
/// so under a lock (kuluu-render keeps it inside `Mutex<NrContext>`).
pub struct NrParams {
    ptr: NvngxParameter,
}

// SAFETY: the map is an opaque pointer to host-layer-owned memory with no
// thread affinity; cross-thread sharing is guarded by the caller's Mutex.
unsafe impl Send for NrParams {}
unsafe impl Sync for NrParams {}

impl NrParams {
    /// `NVSDK_NGX_VULKAN_AllocateParameters`. Must be called after a
    /// successful runtime Init (the host layer validates its own state).
    pub fn allocate() -> Result<Self, NvngxResult> {
        let mut ptr: NvngxParameter = std::ptr::null_mut();
        // SAFETY: `ptr` is a valid out location; on success the host layer
        // takes ownership of the allocation and we take ownership of it here.
        let r = unsafe { NVSDK_NGX_VULKAN_AllocateParameters(&mut ptr) };
        if r != NGX_SUCCESS || ptr.is_null() {
            return Err(r);
        }
        Ok(Self { ptr })
    }

    /// `NVSDK_NGX_Parameter_SetVoidPointer` — how textures (pointers to
    /// [`NvngxResourceVk`]) and callbacks are passed. The pointed-to value must
    /// outlive the runtime call that reads it, not just this setter.
    pub fn set_void_pointer<T>(&self, name: &str, value: &T) {
        // SAFETY: `name` is NUL-terminated for the duration of the call; the
        // host layer copies nothing — callers keep `value` alive until the
        // corresponding Create/Evaluate returns (see kuluu-render's node).
        unsafe {
            NVSDK_NGX_Parameter_SetVoidPointer(
                self.ptr,
                cstr(name).as_ptr(),
                value as *const T as *mut c_void,
            );
        }
    }

    pub fn set_i(&self, name: &str, value: i32) {
        // SAFETY: same as above; the int is passed by value.
        unsafe { NVSDK_NGX_Parameter_SetI(self.ptr, cstr(name).as_ptr(), value) };
    }

    pub fn set_ui(&self, name: &str, value: u32) {
        // SAFETY: same as above; the uint is passed by value.
        unsafe { NVSDK_NGX_Parameter_SetUI(self.ptr, cstr(name).as_ptr(), value) };
    }

    pub fn set_f(&self, name: &str, value: f32) {
        // SAFETY: same as above; the float is passed by value.
        unsafe { NVSDK_NGX_Parameter_SetF(self.ptr, cstr(name).as_ptr(), value) };
    }

    /// Unsets a previously set void-pointer entry (stores NULL). The runtime's
    /// lookup then returns its default (NULL) as if the key were absent — how a
    /// resource is withdrawn from this map, which has no remove API of its own.
    pub fn clear_void_pointer(&self, name: &str) {
        // SAFETY: `name` is NUL-terminated for the call; NULL is a valid stored
        // value (the host layer's setter just records the pointer it is given).
        unsafe {
            NVSDK_NGX_Parameter_SetVoidPointer(self.ptr, cstr(name).as_ptr(), std::ptr::null_mut());
        };
    }

    /// The raw map pointer to hand to the runtime entry points.
    #[must_use]
    pub const fn as_ptr(&self) -> NvngxParameter {
        self.ptr
    }
}

impl Drop for NrParams {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: `ptr` came from AllocateParameters and is destroyed once.
            let _ = unsafe { NVSDK_NGX_VULKAN_DestroyParameters(self.ptr) };
        }
    }
}

/// Borrow a NUL-terminated C string for the duration of one FFI call.
fn cstr(s: &str) -> CString {
    // Parameter names are fixed ASCII literals; infallible in practice.
    CString::new(s).unwrap_or_else(|_| CString::new("").expect("empty is valid"))
}

// ---------------------------------------------------------------------------
// NR runtime entry points (nvngx_dlssnr.dll, loaded at runtime)
// ---------------------------------------------------------------------------

/// `NVSDK_NGX_VULKAN_Init_Ext` — the v310.8 signature (public header shape,
/// stable across versions).
type FnInitExt = unsafe extern "C" fn(
    app_id: u64,
    data_path: *const u16,
    instance: u64,
    physical_device: u64,
    device: u64,
    sdk_version: c_int,
    params: NvngxParameter,
) -> NvngxResult;

/// `NVSDK_NGX_VULKAN_CreateFeature`. Feature id 0x12 (decimal 18) is DLSSNR —
/// beyond the public enum's RayReconstruction=13; confirmed from the RenoDX
/// addon's call site and the runtime's per-feature dispatch table.
type FnCreateFeature = unsafe extern "C" fn(
    cmd: u64,
    feature_id: u32,
    params: NvngxParameter,
    out_handle: *mut NvngxHandle,
) -> NvngxResult;

/// `NVSDK_NGX_VULKAN_EvaluateFeature` (callback is NULL — no progress hook).
/// The handle arg is the opaque object pointer CreateFeature wrote, cast to a
/// pointer — the runtime dereferences it and reads its first u32 as the table
/// key. It is NOT a pointer to our [`NvngxHandle`] storage.
type FnEvaluateFeature = unsafe extern "C" fn(
    cmd: u64,
    handle: *const c_void,
    params: NvngxParameter,
    callback: *const c_void,
) -> NvngxResult;

/// `NVSDK_NGX_VULKAN_ReleaseFeature` — same opaque-pointer semantics as
/// Evaluate; the runtime does not write back through it.
type FnReleaseFeature = unsafe extern "C" fn(handle: *mut c_void) -> NvngxResult;

/// `NVSDK_NGX_VULKAN_Shutdown1` (device-scoped shutdown).
type FnShutdown1 = unsafe extern "C" fn(device: u64) -> NvngxResult;

extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    // `*const c_char` (i8 on Windows) — matches CString::as_ptr() exactly.
    fn GetProcAddress(h_module: *mut c_void, proc_name: *const c_char) -> *mut c_void;
    /// Thread-local error code set by the last failing Win32 call.
    fn GetLastError() -> u32;
}

/// The NR feature id (0x12). Not in the public v310.5.3 enum — see module docs.
pub const FEATURE_DLSSNR: u32 = 0x12;

/// `NVSDK_NGX_Version_API` from nvsdk_ngx_defs.h (NGX_VERSION_DOT 1.5.0).
const NGX_VERSION_API: c_int = 0x0000_0015;

// ---------------------------------------------------------------------------
// The "nvngx.dll" calling-module forwarder (build 9 — ffxi_dlss5.md §2.10–§3.5)
// ---------------------------------------------------------------------------

/// Staged next to kuluu.exe as nvngx.dll_kuluu.dll (renamed from kuluu_ngx_fwd.dll;
/// see "Running / distributing" in docs/DLSS.md for the copy step).
/// The NR runtime gates its Init_Ext/CreateFeature/ReleaseFeature entry points on
/// the calling module's file name containing "nvngx.dll" (case-insensitive substring);
/// this name passes without shadowing the driver's real nvngx.dll that nvsdk_ngx_s.lib
/// loads. EvaluateFeature is ungated in v310.8 and stays a direct call.
pub const FORWARDER_DLL: &str = "nvngx.dll_kuluu.dll";

/// ABI version the forwarder must report via `kuluu_ngx_fwd_abi_version`.
const FORWARDER_ABI: u32 = 2; // v1: init_ext only; v2: + create_feature, release_feature

/// Sentinel returned by the forwarder when it received a null target pointer —
/// outside NGX's 0xBAD0_xxxx range so it can never be confused with an NGX error.
// u32 -> i32 cast keeps the bit pattern (the forwarder returns it in eax);
// a bare literal would not fit c_int's range.
pub const FWD_NULL_TARGET: NvngxResult = 0xF0F0_0001u32 as i32;

/// The forwarder's trampolines, declared with OUR existing types (ABI-identical to the
/// forwarder's u64/u32/*const declarations on x86_64) so `Some(self.<entry>)` passes
/// without any transmute.
type PfnFwdVulkanInitExt = unsafe extern "C" fn(
    target: Option<FnInitExt>,
    app_id: u64,
    data_path: *const u16,
    instance: u64,
    physical_device: u64,
    device: u64,
    sdk_version: c_int,
    params: NvngxParameter,
) -> NvngxResult;

type PfnFwdCreateFeature = unsafe extern "C" fn(
    target: Option<FnCreateFeature>,
    cmd: u64,
    feature_id: u32,
    params: NvngxParameter,
    out_handle: *mut NvngxHandle,
) -> NvngxResult;

type PfnFwdReleaseFeature =
    unsafe extern "C" fn(target: Option<FnReleaseFeature>, handle: *mut c_void) -> NvngxResult;

/// The forwarder's `kuluu_ngx_fwd_abi_version` export.
type PfnFwdAbiVersion = unsafe extern "C" fn() -> u32;

/// Raw Vulkan handles for the NGX runtime, extracted once from wgpu.
#[derive(Clone, Copy, Debug)]
pub struct VulkanHandles {
    pub instance: u64,
    pub physical_device: u64,
    pub device: u64,
}

impl VulkanHandles {
    /// Extracts the raw handles from a wgpu Device (Vulkan backend only).
    /// Returns None on any non-Vulkan setup — NR is then simply unavailable.
    #[must_use]
    pub fn from_wgpu(device: &wgpu::Device) -> Option<Self> {
        use wgpu::hal::api::Vulkan;
        // SAFETY: as_hal only reads the backend tag of an already-created
        // device and returns its hal view; no state is mutated.
        let hal_device = unsafe { device.as_hal::<Vulkan>() }?;
        // ash 0.38 dispatchable handles are `*mut u8`; the NGX runtime wants
        // the raw pointer value as a u64 (VkInstance/VkPhysicalDevice/VkDevice).
        Some(Self {
            instance: hal_device
                .shared_instance()
                .raw_instance()
                .handle()
                .as_raw(),
            physical_device: hal_device.raw_physical_device().as_raw(),
            device: hal_device.raw_device().handle().as_raw(),
        })
    }
}

/// Why loading the NR runtime failed — rendered into log lines by callers.
#[derive(Clone, Copy, Debug)]
pub enum LoadError {
    /// `LoadLibraryW` returned NULL for this module; carries the Win32 code
    /// from GetLastError (and which DLL was being loaded).
    Win32(&'static str, u32),
    /// The DLL loaded but this export lookup failed; carries GetLastError
    /// (127 = name truly absent from the table, 126 = handle wasn't a module).
    MissingSymbol(&'static str, u32),
    /// The forwarder DLL loaded but reported a different ABI version than
    /// expected (stale staged copy after an export change).
    ForwarderAbi { found: u32, expected: u32 },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Win32(module, code) => {
                write!(
                    f,
                    "LoadLibraryW failed for `{module}` (win32 error {code}: {})",
                    win32_load_hint(*code)
                )
            }
            Self::MissingSymbol(sym, code) => {
                write!(f, "DLL loaded but export `{sym}` is missing (win32 {code})")
            }
            Self::ForwarderAbi { found, expected } => {
                write!(f, "forwarder ABI mismatch: found v{found}, expected v{expected} (stale staged copy?)")
            }
        }
    }
}

/// Short hints for the Win32 codes that actually matter here (winerror.h).
fn win32_load_hint(code: u32) -> &'static str {
    match code {
        2 => "file not found at the expected path",
        5 => "access denied (antivirus/policy?)",
        126 => "a dependency of the DLL is missing",
        127 => "an entry point is missing in a dependent module",
        1114 => "the DLL's DllMain refused to load it",
        _ => "see winerror.h",
    }
}

/// A loaded `nvngx_dlssnr.dll` with its five Vulkan entry points resolved.
#[derive(Clone, Copy)]
pub struct NrRuntime {
    init_ext: FnInitExt,
    create_feature: FnCreateFeature,
    evaluate_feature: FnEvaluateFeature,
    release_feature: FnReleaseFeature,
    shutdown1: FnShutdown1,
    /// The forwarder's trampolines for the module-gated entry points (ffxi_dlss5.md
    /// §2.10): Init_Ext, CreateFeature, ReleaseFeature.
    fwd_init_ext: PfnFwdVulkanInitExt,
    fwd_create_feature: PfnFwdCreateFeature,
    fwd_release_feature: PfnFwdReleaseFeature,
}

/// The forwarder's resolved exports. The ABI version check runs before any of these
/// lookups, so a stale staged copy fails with ForwarderAbi instead of MissingSymbol.
struct ForwarderExports {
    init_ext: PfnFwdVulkanInitExt,
    create_feature: PfnFwdCreateFeature,
    release_feature: PfnFwdReleaseFeature,
}

// SAFETY: function pointers to a stateless DLL are Send+Sync.
unsafe impl Send for NrRuntime {}
unsafe impl Sync for NrRuntime {}

/// Resolves one export from the forwarder module and reinterprets it as `T`.
fn resolve_forwarder_export<T>(hmod: *mut c_void, name: &'static str) -> Result<T, LoadError> {
    // SAFETY: valid NUL-terminated ASCII name against a live module handle.
    let p = unsafe { GetProcAddress(hmod, cstr(name).as_ptr()) };
    if p.is_null() {
        return Err(LoadError::MissingSymbol(name, unsafe { GetLastError() }));
    }
    // SAFETY: `p` is a valid export from the forwarder's table; both sides are
    // 8-byte x64 values (raw ptr -> fn ptr). transmute_copy::<T, U> takes &T and
    // returns U — source pointee first.
    Ok(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
}

impl std::fmt::Debug for NrRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NrRuntime").finish_non_exhaustive()
    }
}

impl NrRuntime {
    /// Loads `nvngx_dlssnr.dll` from next to the executable and resolves every
    /// entry point. The absolute path makes the search deterministic (the app
    /// dir is LoadLibraryW's first stop anyway, but a miss then becomes a real
    /// load error we can diagnose instead of a silent NULL).
    pub fn load() -> Result<Self, LoadError> {
        // Materialize the exe dir once: both the NR DLL path and the forwarder
        // path are built from it. `to_str()` borrows from a joined PathBuf, so
        // the join must not be a temporary (build 7's E0515).
        let mut exe_dir = std::path::PathBuf::from(".");
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                exe_dir = dir.to_path_buf();
            }
        }
        let mut name = String::from("nvngx_dlssnr.dll\0");
        if let Some(abs) = exe_dir.join("nvngx_dlssnr.dll").to_str() {
            name = format!("{abs}\0");
        }
        let wide = encode_wide(&name);
        // SAFETY: `wide` is a valid NUL-terminated UTF-16 string for the call.
        let hmod = unsafe { LoadLibraryW(wide.as_ptr()) };
        if hmod.is_null() {
            // Thread-local, set by the failed call above; read it before
            // anything else can clobber it.
            return Err(LoadError::Win32("nvngx_dlssnr.dll", unsafe {
                GetLastError()
            }));
        }

        macro_rules! resolve {
            ($sym:literal) => {{
                // Rust string literals are NOT C strings — `as_ptr()` would hand
                // GetProcAddress an unterminated name and it would search for the
                // symbol plus whatever bytes follow in .rodata (build 7's NULL).
                let c_name = cstr($sym);
                // SAFETY: `c_name` is a valid NUL-terminated ASCII string.
                let p = unsafe { GetProcAddress(hmod, c_name.as_ptr()) };
                if p.is_null() {
                    // Thread-local; read before anything else can clobber it
                    // (127 = name absent, 126 = bad module handle).
                    return Err(LoadError::MissingSymbol($sym, unsafe { GetLastError() }));
                }
                // SAFETY: `p` is a valid function pointer from the DLL's export
                // table; transmute_copy reinterprets it as the typed fn ptr.
                unsafe { std::mem::transmute_copy(&p) }
            }};
        }

        // The forwarder must be present too: without it the gated entry points
        // (Init_Ext, CreateFeature, ReleaseFeature) all fail with PlatformError
        // (ffxi_dlss5.md §2.10). Failing here — not at init time — gives a clear,
        // one-shot load error instead of a per-second retry loop.
        let fwd = Self::load_forwarder(&exe_dir)?;

        Ok(Self {
            init_ext: resolve!("NVSDK_NGX_VULKAN_Init_Ext"),
            create_feature: resolve!("NVSDK_NGX_VULKAN_CreateFeature"),
            evaluate_feature: resolve!("NVSDK_NGX_VULKAN_EvaluateFeature"),
            release_feature: resolve!("NVSDK_NGX_VULKAN_ReleaseFeature"),
            shutdown1: resolve!("NVSDK_NGX_VULKAN_Shutdown1"),
            fwd_init_ext: fwd.init_ext,
            fwd_create_feature: fwd.create_feature,
            fwd_release_feature: fwd.release_feature,
        })
    }

    /// Loads the "nvngx.dll" calling-module forwarder from next to the exe and
    /// checks its ABI version. The NR runtime gates its Init_Ext/CreateFeature/
    /// ReleaseFeature entries on the caller's module name (ffxi_dlss5.md §2.10–§2.11);
    /// this DLL is staged as `nvngx.dll_kuluu.dll` so those calls can be made from
    /// inside a module whose file name contains "nvngx.dll".
    fn load_forwarder(exe_dir: &std::path::Path) -> Result<ForwarderExports, LoadError> {
        let path = exe_dir.join(FORWARDER_DLL);
        // NUL-terminated UTF-16 for LoadLibraryW (encode_wide expects the NUL
        // included; build it directly from the Path here).
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is a valid NUL-terminated UTF-16 string for the call.
        let hmod = unsafe { LoadLibraryW(wide.as_ptr()) };
        if hmod.is_null() {
            return Err(LoadError::Win32(FORWARDER_DLL, unsafe { GetLastError() }));
        }

        let abi = resolve_forwarder_export::<PfnFwdAbiVersion>(hmod, "kuluu_ngx_fwd_abi_version")?;
        let found = unsafe { abi() };
        if found != FORWARDER_ABI {
            return Err(LoadError::ForwarderAbi {
                found,
                expected: FORWARDER_ABI,
            });
        }

        Ok(ForwarderExports {
            init_ext: resolve_forwarder_export(hmod, "kuluu_ngx_fwd_vulkan_init_ext")?,
            create_feature: resolve_forwarder_export(hmod, "kuluu_ngx_fwd_vulkan_create_feature")?,
            release_feature: resolve_forwarder_export(
                hmod,
                "kuluu_ngx_fwd_vulkan_release_feature",
            )?,
        })
    }

    /// `NVSDK_NGX_VULKAN_Init_Ext` — once per process, before any Create.
    /// `data_path` must be a writable directory (NGX logs/models land there).
    /// Routed through the forwarder: the NR runtime gates this entry on the
    /// calling module's file name containing "nvngx.dll", and kuluu.exe does
    /// not — but the staged `nvngx.dll_kuluu.dll` does (§2.10–§3.5).
    pub fn init(
        &self,
        app_id: u64,
        data_path: &str,
        handles: &VulkanHandles,
        params: Option<&NrParams>,
    ) -> NvngxResult {
        let path = format!("{data_path}\0");
        let wide = encode_wide(&path);
        // SAFETY: `wide` is a valid NUL-terminated UTF-16 string for the call
        // (the runtime copies what it needs during init); all handles come from
        // wgpu-hal for a live device; the
        // forwarder is a pure pass-through whose only side effect is where the
        // return address lands (inside nvngx.dll_kuluu.dll, which passes the
        // gate). `Some(self.init_ext)` hands it the real Init_Ext export.
        unsafe {
            (self.fwd_init_ext)(
                Some(self.init_ext),
                app_id,
                wide.as_ptr(),
                handles.instance,
                handles.physical_device,
                handles.device,
                NGX_VERSION_API,
                params.map_or(std::ptr::null_mut(), NrParams::as_ptr),
            )
        }
    }

    /// Creates the DLSSNR feature (id 0x12) on `cmd` and fills in every
    /// create-time parameter the runtime expects — the exact set the RenoDX
    /// addon sets before its CreateFeature call:
    /// generic Width/Height/OutWidth/OutHeight, the DLSSNR.* dimension aliases,
    /// Upscaling=1, ScalingRatio 1.0 (native-res pass), and
    /// CreationNodeMask/VisibilityNodeMask = 1.
    pub fn create_nr_feature(
        &self,
        cmd: u64,
        params: &NrParams,
        render_w: u32,
        render_h: u32,
        out_w: u32,
        out_h: u32,
    ) -> Result<NvngxHandle, NvngxResult> {
        // The runtime looks the same dimensions up under several name aliases;
        // set them all (cheap) so a v310.8 parser variant finds each one it
        // asks for instead of falling back to 0.
        params.set_ui("Width", render_w);
        params.set_ui("Height", render_h);
        params.set_ui("OutWidth", out_w);
        params.set_ui("OutHeight", out_h);
        params.set_ui("DLSSNR.Width", render_w);
        params.set_ui("DLSSNR.Height", render_h);
        params.set_ui("DLSSNR.InputWidth", render_w);
        params.set_ui("DLSSNR.InputHeight", render_h);
        params.set_ui("DLSSNR.OutputWidth", out_w);
        params.set_ui("DLSSNR.OutputHeight", out_h);
        params.set_ui("DLSSNR.Output.Width", out_w);
        params.set_ui("DLSSNR.Output.Height", out_h);
        // Native-resolution enhancement pass: input and output are the same
        // size, so no upscaling. (The addon's dynamic-ratio callback is for
        // games that resize between frames; we recreate on resize instead.)
        params.set_i("DLSSNR.Upscaling", 1);
        params.set_f("DLSSNR.ScalingRatio", 1.0);
        params.set_f("DLSSNR.Scale", 1.0);
        // Which internal compute nodes the runtime builds/enables. The addon
        // sets both to 1; without them the feature creates but does nothing.
        params.set_ui("CreationNodeMask", 1);
        params.set_ui("VisibilityNodeMask", 1);

        let mut handle = NvngxHandle::default();
        // SAFETY: `cmd` is a valid in-flight VkCommandBuffer; `params` outlives
        // the call; `handle` is a valid out location. CreateFeature is module-gated
        // like Init_Ext (ffxi_dlss5.md §2.10), so the call goes through the forwarder.
        let r = unsafe {
            (self.fwd_create_feature)(
                Some(self.create_feature),
                cmd,
                FEATURE_DLSSNR,
                params.as_ptr(),
                &mut handle,
            )
        };
        if r != NGX_SUCCESS || handle.is_empty() {
            return Err(r);
        }
        Ok(handle)
    }

    /// Runs one frame of Neural Uplift: `color` + `mvec` (+ optional `depth`) in,
    /// `output` out. All resources are full-window-size textures; when the scene
    /// was rendered at a sub-resolution (DLSS SR active), pass that size as
    /// `valid_subrect_w/h` so the runtime only reads the valid top-left region of
    /// the depth prepass — otherwise pass the texture's full size.
    ///
    /// Every parameter is written unconditionally: the map persists across frames,
    /// and a conditional write would leave the previous frame's value in place
    /// (stale subrects after an SR toggle, a stale Reset=1, a stale depth pointer
    /// under MSAA). `reset` flushes the runtime's temporal history for this frame
    /// only; pass false on steady-state frames.
    pub fn evaluate_nr(
        &self,
        cmd: u64,
        handle: &NvngxHandle,
        params: &NrParams,
        color: &NvngxResourceVk,
        mvec: &NvngxResourceVk,
        depth: Option<&NvngxResourceVk>,
        output: &NvngxResourceVk,
        intensity: f32,
        local_tone_strength: f32,
        structure_strength: f32,
        depth_inverted: bool,
        valid_subrect_w: u32,
        valid_subrect_h: u32,
        reset: bool,
    ) -> NvngxResult {
        params.set_void_pointer("DLSSNR.Color", color);
        // Motion vectors are always provided — the RenoDX addon's contract.
        // Kuluu passes a zero-filled stand-in sized to the input: an explicit
        // "no motion" instead of a NULL, whose semantics this build does not
        // define (the parser stores it; eval never null-checks it).
        params.set_void_pointer("DLSSNR.MVec", mvec);
        match depth {
            Some(depth) => {
                params.set_void_pointer("DLSSNR.Depth", depth);
                // The parser defaults DepthInverted to 1 — the D3D near=1/far=0
                // convention, which the RenoDX addon also sends for its D3D12
                // games. Bevy's prepass writes standard Vulkan depth (near=0), so
                // send the flag verbatim: 0 = not inverted.
                params.set_i("DLSSNR.DepthInverted", i32::from(depth_inverted));
                // No-dot names, read through the parser's INT getter slot (+0x58)
                // — verified in this build's disasm; a SetUI value is invisible to
                // it. Always explicit: full texture when the scene rendered at full
                // res (the map would otherwise keep the last SR subrect).
                let sub_w = i32::try_from(valid_subrect_w).unwrap_or(i32::MAX);
                let sub_h = i32::try_from(valid_subrect_h).unwrap_or(i32::MAX);
                params.set_i("DLSSNR.DepthSubrectWidth", sub_w);
                params.set_i("DLSSNR.DepthSubrectHeight", sub_h);
            }
            None => {
                // Depth unavailable this frame (MSAA): withdraw it. Without the
                // NULL store, a stale pointer to a texture wgpu may already have
                // freed would be sampled by the runtime.
                params.clear_void_pointer("DLSSNR.Depth");
            }
        }
        params.set_void_pointer("DLSSNR.Output", output);

        // The menu knobs. Intensity 1.0 is the parser default and can read as
        // "no visible effect" — kuluu's default is 1.01 (the addon's).
        params.set_f("DLSSNR.Intensity", intensity);
        params.set_f("DLSSNR.LocalToneStrength", local_tone_strength);
        params.set_f("DLSSNR.LocalStructureStrength", structure_strength);
        params.set_i("DLSSNR.Enabled", 1);
        // Read through the INT slot like Enabled; always explicit so a stale
        // Reset=1 from a config-change frame cannot keep flushing history.
        params.set_i("DLSSNR.Reset", i32::from(reset));

        // SAFETY: `cmd` is a valid in-flight VkCommandBuffer; `handle.ptr` is
        // the opaque object pointer CreateFeature wrote for a live feature —
        // the runtime dereferences it and reads its first u32 as the table key;
        // the resource structs on the caller's stack outlive this call (the
        // runtime reads them synchronously during encoding).
        unsafe {
            (self.evaluate_feature)(
                cmd,
                handle.ptr as *const c_void,
                params.as_ptr(),
                std::ptr::null(),
            )
        }
    }

    /// `NVSDK_NGX_VULKAN_ReleaseFeature` — drops the feature from the runtime's
    /// table. The handle must not be used afterwards. Not command-buffer
    /// encoded: the C signature takes only the handle.
    pub fn release_feature(&self, handle: &mut NvngxHandle) -> NvngxResult {
        // SAFETY: `handle.ptr` is the opaque object pointer of a live feature;
        // the runtime dereferences it for its table lookup and does not write
        // back through it. ReleaseFeature is module-gated like Init_Ext
        // (ffxi_dlss5.md §2.10), so the call goes through the forwarder.
        let r = unsafe {
            (self.fwd_release_feature)(Some(self.release_feature), handle.ptr as *mut c_void)
        };
        // Zeroing is our bookkeeping only — the runtime never stores through
        // InHandle (verified in disasm).
        handle.ptr = 0;
        r
    }

    /// `NVSDK_NGX_VULKAN_Shutdown1` — process teardown, after the device is idle.
    /// Gated like Init_Ext in v310.8 (ffxi_dlss5.md §2.10): add a forwarder
    /// trampoline before calling this from kuluu.exe.
    pub fn shutdown(&self, handles: &VulkanHandles) -> NvngxResult {
        // SAFETY: `device` is a live VkDevice for the call's duration.
        unsafe { (self.shutdown1)(handles.device) }
    }
}

/// Waits for all previously submitted GPU work to complete (Vulkan only).
/// Must run before releasing an NGX feature whose evaluate command buffers may
/// still be in flight — otherwise the runtime can free internal resources that
/// in-flight compute references, which surfaces as a driver error and eventual
/// device loss. Mirrors dlss_wgpu's Drop ordering (wait idle -> ReleaseFeature).
/// Returns the raw vk::Result code on failure; callers log it and proceed with
/// the best-effort release anyway.
pub fn wait_device_idle(device: &wgpu::Device) -> Result<(), i32> {
    use wgpu::hal::api::Vulkan;

    // Unreachable in practice (every caller sits behind a Vulkan-only init),
    // but keep the guard panic-free like the rest of this function.
    let hal_opt = unsafe { device.as_hal::<Vulkan>() };
    let Some(hal_device) = hal_opt else {
        return Err(WAIT_IDLE_NOT_VULKAN);
    };
    // SAFETY: `device` is a live wgpu Device for the call's duration.
    match unsafe { hal_device.raw_device().device_wait_idle() } {
        Ok(()) => Ok(()),
        Err(e) => Err(e.as_raw()),
    }
}

/// Extracts the raw VkCommandBuffer from a wgpu CommandEncoder (Vulkan only).
/// The hal `raw_handle()` call is unsafe; callers above stay safe code.
#[must_use]
pub fn raw_command_buffer(encoder: &mut wgpu::CommandEncoder) -> Option<u64> {
    use wgpu::hal::api::Vulkan;

    let mut raw = 0u64;
    // SAFETY: as_hal_mut only borrows the backend's encoder for the closure's
    // duration; raw_handle is a plain field read on the hal command buffer.
    unsafe {
        encoder.as_hal_mut::<Vulkan, _, _>(|enc| {
            if let Some(enc) = enc {
                raw = enc.raw_handle().as_raw();
            }
        });
    }
    (raw != 0).then_some(raw)
}

/// UTF-16-encodes `s` (which must already include its terminating NUL).
fn encode_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}
