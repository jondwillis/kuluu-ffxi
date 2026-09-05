//! kuluu-ngx-fwd: the "nvngx.dll" calling-module forwarder.
//!
//! WHY THIS EXISTS
//! ---------------
//! `nvngx_dlssnr.dll` v310.8 gates most exported entry points on the identity of the
//! CALLING module: Init_Ext/Init_Ext2, CreateFeature/CreateFeature1, ReleaseFeature,
//! Shutdown/Shutdown1 (each carries its own copy of the same prologue — verified in
//! disasm; only EvaluateFeature omits it). The gate takes the function's return address,
//! resolves that address to a module (GetModuleHandleExW with
//! GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | UNCHANGED_REFCOUNT), reads the module's file
//! name, and does a case-insensitive substring test for "nvngx.dll". Anything else returns
//! NVSDK_NGX_Result_FAIL_PlatformError (0xBAD00002) before a single Vulkan argument is
//! looked at.
//!
//! So the only thing that matters is: the instruction AFTER each gated `call` must live
//! inside a module whose file name contains "nvngx.dll". This DLL is staged as
//! `nvngx.dll_kuluu.dll` and provides exactly that instruction for every gated entry
//! point kuluu calls (Init_Ext, CreateFeature, ReleaseFeature).
//!
//! DESIGN
//! ------
//! * The forwarder does NOT LoadLibrary anything. kuluu-dlss-nr keeps sole ownership of
//!   the nvngx_dlssnr.dll module handle and passes the resolved Init_Ext pointer in.
//!   That keeps one loader, one error path, and no second copy of the 166 MB DLL logic.
//! * The call MUST be a real `call`, never a tail `jmp`. A jmp would leave kuluu.exe's
//!   return address on the stack and the gate would see kuluu.exe again. `black_box` on
//!   the result keeps the call out of tail position at every opt level.
//! * No allocation, no panics, no unwinding across the FFI boundary: every function is a
//!   straight pass-through of scalar arguments.
//!
//! All Vulkan handles cross as u64 (dispatchable handles are pointers on x64, so the ABI
//! is identical) to match how kuluu-dlss-nr already represents them.

#![allow(non_camel_case_types)]

use std::hint::black_box;

/// NVSDK_NGX_Result is a C enum, 32 bits.
pub type NgxResult = u32;

/// Mirrors the confirmed nvngx_dlssnr.dll export:
/// NVSDK_NGX_Result NVSDK_NGX_VULKAN_Init_Ext(
///     unsigned long long InApplicationId,
///     const wchar_t*     InApplicationDataPath,
///     VkInstance, VkPhysicalDevice, VkDevice,
///     NVSDK_NGX_Version  InSDKVersion,
///     const NVSDK_NGX_Parameter* InParameters);
pub type PfnVulkanInitExt = unsafe extern "C" fn(
    app_id: u64,
    app_data_path: *const u16,
    instance: u64,
    physical_device: u64,
    device: u64,
    sdk_version: u32,
    params: *const std::ffi::c_void,
) -> NgxResult;

/// NVSDK_NGX_Result NVSDK_NGX_VULKAN_Init_Ext2(...) has the same shape plus the two
/// proc-addr getters. Not used by kuluu today; exported so the option exists without
/// touching this crate again.
pub type PfnVulkanInitExt2 = unsafe extern "C" fn(
    app_id: u64,
    app_data_path: *const u16,
    instance: u64,
    physical_device: u64,
    device: u64,
    get_instance_proc_addr: *const std::ffi::c_void,
    get_device_proc_addr: *const std::ffi::c_void,
    sdk_version: u32,
    params: *const std::ffi::c_void,
) -> NgxResult;

/// NVSDK_NGX_Result NVSDK_NGX_VULKAN_CreateFeature(
///     VkCommandBuffer InCmdList, NVSDK_NGX_Feature InFeatureID,
///     const NVSDK_NGX_Parameter* InParameters, NVSDK_NGX_Handle** OutHandle);
pub type PfnVulkanCreateFeature = unsafe extern "C" fn(
    cmd: u64,
    feature_id: u32,
    params: *const std::ffi::c_void,
    out_handle: *mut std::ffi::c_void,
) -> NgxResult;

/// NVSDK_NGX_Result NVSDK_NGX_VULKAN_ReleaseFeature(NVSDK_NGX_Handle* InHandle);
pub type PfnVulkanReleaseFeature = unsafe extern "C" fn(handle: *mut std::ffi::c_void) -> NgxResult;

/// Returned when `target` is null so the caller can tell "forwarder got a bad pointer"
/// apart from anything NGX itself would say. Chosen outside NGX's 0xBAD0_xxxx range.
pub const KULUU_FWD_NULL_TARGET: NgxResult = 0xF0F0_0001;

/// Forward one call to `NVSDK_NGX_VULKAN_Init_Ext`.
///
/// # Safety
/// `target` must be the real Init_Ext export of a loaded nvngx_dlssnr.dll and all other
/// arguments must be valid for that function exactly as if the caller invoked it directly.
#[no_mangle]
pub unsafe extern "C" fn kuluu_ngx_fwd_vulkan_init_ext(
    target: Option<PfnVulkanInitExt>,
    app_id: u64,
    app_data_path: *const u16,
    instance: u64,
    physical_device: u64,
    device: u64,
    sdk_version: u32,
    params: *const std::ffi::c_void,
) -> NgxResult {
    let Some(f) = target else {
        return KULUU_FWD_NULL_TARGET;
    };
    // The `call` instruction generated here is the whole point of this crate.
    let r = f(
        app_id,
        app_data_path,
        instance,
        physical_device,
        device,
        sdk_version,
        params,
    );
    // Keep the call out of tail position so it can never become a `jmp`.
    black_box(r)
}

/// Same as above for `NVSDK_NGX_VULKAN_Init_Ext2`.
///
/// # Safety
/// See `kuluu_ngx_fwd_vulkan_init_ext`.
#[no_mangle]
pub unsafe extern "C" fn kuluu_ngx_fwd_vulkan_init_ext2(
    target: Option<PfnVulkanInitExt2>,
    app_id: u64,
    app_data_path: *const u16,
    instance: u64,
    physical_device: u64,
    device: u64,
    get_instance_proc_addr: *const std::ffi::c_void,
    get_device_proc_addr: *const std::ffi::c_void,
    sdk_version: u32,
    params: *const std::ffi::c_void,
) -> NgxResult {
    let Some(f) = target else {
        return KULUU_FWD_NULL_TARGET;
    };
    let r = f(
        app_id,
        app_data_path,
        instance,
        physical_device,
        device,
        get_instance_proc_addr,
        get_device_proc_addr,
        sdk_version,
        params,
    );
    black_box(r)
}

/// Forward one call to `NVSDK_NGX_VULKAN_CreateFeature` (gated like Init_Ext —
/// ffxi_dlss5.md §2.10).
///
/// # Safety
/// See `kuluu_ngx_fwd_vulkan_init_ext`.
#[no_mangle]
pub unsafe extern "C" fn kuluu_ngx_fwd_vulkan_create_feature(
    target: Option<PfnVulkanCreateFeature>,
    cmd: u64,
    feature_id: u32,
    params: *const std::ffi::c_void,
    out_handle: *mut std::ffi::c_void,
) -> NgxResult {
    let Some(f) = target else {
        return KULUU_FWD_NULL_TARGET;
    };
    let r = f(cmd, feature_id, params, out_handle);
    black_box(r)
}

/// Forward one call to `NVSDK_NGX_VULKAN_ReleaseFeature` (gated like Init_Ext).
///
/// # Safety
/// See `kuluu_ngx_fwd_vulkan_init_ext`.
#[no_mangle]
pub unsafe extern "C" fn kuluu_ngx_fwd_vulkan_release_feature(
    target: Option<PfnVulkanReleaseFeature>,
    handle: *mut std::ffi::c_void,
) -> NgxResult {
    let Some(f) = target else {
        return KULUU_FWD_NULL_TARGET;
    };
    let r = f(handle);
    black_box(r)
}

/// Cheap presence/version probe so kuluu can log "forwarder vX loaded" and refuse to run
/// against a stale staged copy after the ABI of these exports changes.
#[no_mangle]
pub extern "C" fn kuluu_ngx_fwd_abi_version() -> u32 {
    2 // v1: init_ext only; v2: + create_feature, release_feature (both are gated too)
}
