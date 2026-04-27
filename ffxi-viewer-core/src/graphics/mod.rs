pub mod settings;

#[cfg(not(target_arch = "wasm32"))]
pub mod render_scale;

pub use settings::*;
