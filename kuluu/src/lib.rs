#![allow(clippy::type_complexity, clippy::too_many_arguments)]

pub mod launcher;
pub mod launcher_store;
pub mod secret_store;

#[cfg(feature = "native-window")]
pub mod audio_store;
#[cfg(feature = "native-window")]
pub mod graphics_store;
#[cfg(feature = "native-window")]
pub mod keybinds_store;
#[cfg(feature = "native-window")]
pub mod marker_store;
#[cfg(feature = "native-window")]
pub mod overlay_store;
#[cfg(feature = "native-window")]
pub mod padbinds_store;

// The windowed viewer lives in the library (not the binary) so examples and
// integration tests can drive it headless, e.g. the walker's zz-field-walk.
#[cfg(feature = "native-window")]
pub mod view_native;
