pub mod settings;

/// DLSS runtime plumbing (capability probe, settings -> bevy mapping). Only
/// exists on dlss-feature builds; every reference to it is feature-gated, so
/// an accidental ungated use fails the default build loudly.
#[cfg(all(not(target_arch = "wasm32"), feature = "dlss"))]
pub mod dlss;

#[cfg(all(target_os = "windows", feature = "enhanced-neural-uplift"))]
pub mod dlss_nr;

#[cfg(not(target_arch = "wasm32"))]
pub mod render_scale;

pub use settings::*;
