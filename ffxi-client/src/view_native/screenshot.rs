//! `/screenshot [path]` slash command: capture the primary window to a
//! PNG on disk via Bevy's `Screenshot::primary_window()` API.
//!
//! Intended use: empirical material-LUT workflow. Combine with the env
//! vars read by `ffxi_viewer_core::dat_mzb::mzb_palette_color`:
//!   * `FFXI_MATERIAL_HIGHLIGHT=N`  — isolate material N (red on gray)
//!   * `FFXI_MATERIAL_PALETTE=hisat` — saturated rainbow palette
//!
//! Walk the live client to a recognizable viewpoint, `/screenshot
//! bastok-spot1.png`, then take a matching retail FFXI screenshot at
//! the same in-game position. Cross-reference surface-by-surface.
//!
//! Companion: the headless example
//! `ffxi-viewer-core/examples/mzb-render-headless.rs` produces the same
//! capture deterministically without a live client session.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use ffxi_viewer_core::snapshot::ToastEvent;

/// Fired by the slash-command dispatcher; consumed by
/// [`process_screenshot_requests`].
#[derive(Message, Debug, Clone)]
pub struct ScreenshotRequest {
    /// Output path. Relative paths land in the process's CWD (the repo
    /// root when launched via `cargo run`).
    pub path: PathBuf,
}

/// Consumer: spawns a `Screenshot::primary_window()` entity with a
/// `save_to_disk` observer for each pending request. Bevy completes the
/// capture asynchronously over a few frames; we don't block on it.
pub fn process_screenshot_requests(
    mut events: MessageReader<ScreenshotRequest>,
    mut commands: Commands,
    mut toasts: MessageWriter<ToastEvent>,
) {
    for req in events.read() {
        let path = req.path.clone();
        let display = path.display().to_string();
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        toasts.write(ToastEvent::system(format!(
            "/screenshot: capturing -> {display}"
        )));
    }
}
