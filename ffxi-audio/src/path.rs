use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioKind {
    Bgm,
    Sfx,
}

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

        assert!(find_audio(root, AudioKind::Bgm, 101).is_none());
    }
}
