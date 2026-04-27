use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use ffxi_viewer_core::snapshot::ToastEvent;

#[derive(Message, Debug, Clone)]
pub struct ScreenshotRequest {
    pub path: PathBuf,
}

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
