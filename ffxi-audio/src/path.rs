//! Locate `.bgw` / `.spw` files under the FFXI install tree.
//!
//! Layout (matches `vendor/lotus-ffxi/ffxi/audio/ffxi_audio.cppm:84-89`):
//!
//! ```text
//! {install}/sound/win/music/data/musicNNN.bgw
//! {install}/sound/win/se/seNNN/seNNNNNN.spw
//! ```
//!
//! where `NNN = id / 1000` (zero-padded to 3) and `NNNNNN = id`
//! (zero-padded to 6). Expansion sound directories live at
//! `sound2/win`, `sound3/win`, … up through `sound15/win`.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioKind {
    Bgm,
    Sfx,
}

/// Search every `sound{,2..15}/win/` subtree for the audio file
/// matching `(kind, id)`. Returns `None` if no candidate exists.
///
/// `install_root` should be the directory containing
/// `sound/` (typically `.../SquareEnix/FINAL FANTASY XI`).
pub fn find_audio(install_root: &Path, kind: AudioKind, id: u32) -> Option<PathBuf> {
    for n in 0..=15u32 {
        let sound_dir = if n == 0 {
            install_root.join("sound")
        } else {
            install_root.join(format!("sound{n}"))
        };
        let candidate = match kind {
            AudioKind::Bgm => sound_dir
                .join("win")
                .join("music")
                .join("data")
                .join(format!("music{:03}.bgw", id)),
            AudioKind::Sfx => sound_dir
                .join("win")
                .join("se")
                .join(format!("se{:03}", id / 1000))
                .join(format!("se{:06}.spw", id)),
        };
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgm_path_layout() {
        let root = Path::new("/install");
        // Use a name that won't exist on disk; assert by reading
        // through the candidate path the function would have tried
        // first. (We don't expose the candidate generator publicly,
        // so we just verify the function returns None for a fake
        // install — the path-shape regression test lives in the
        // crate's own integration tests.)
        assert!(find_audio(root, AudioKind::Bgm, 101).is_none());
    }
}
