use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use kuluu_render::snapshot::ToastEvent;

#[derive(Message, Debug, Clone)]
pub struct ScreenshotRequest {
    pub path: PathBuf,
}

static DEFAULT_PATH_COUNTER: AtomicU32 = AtomicU32::new(0);

pub fn next_default_path() -> PathBuf {
    let n = DEFAULT_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!("screenshot-{n}.png"))
}

/// Focus-less GUI driving (kuluu-wwwv): a socket `screenshot` command bumps the
/// shared handle's seq; fire the request when it changes. Bevy captures by
/// reading back the render target, so the window can stay buried and the human
/// keeps working uninterrupted.
pub(crate) fn trigger_screenshot_from_socket(
    handle: Option<Res<super::DebugControlHandle>>,
    mut last_seq: Local<u64>,
    mut requests: MessageWriter<ScreenshotRequest>,
) {
    let Some(handle) = handle else {
        return;
    };
    let Ok(ctrl) = handle.0.lock() else {
        return;
    };
    let seq = ctrl.screenshot_seq();
    if seq != *last_seq {
        *last_seq = seq;
        let path = ctrl.screenshot_path().unwrap_or_else(next_default_path);
        requests.write(ScreenshotRequest { path });
    }
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

#[cfg(test)]
mod tests {
    use super::next_default_path;

    #[test]
    fn default_paths_never_repeat() {
        let first = next_default_path();
        let second = next_default_path();
        assert_ne!(first, second);
        for path in [&first, &second] {
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with("screenshot-"), "{name}");
            assert!(name.ends_with(".png"), "{name}");
        }
    }
}
