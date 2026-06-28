use std::path::PathBuf;

use anyhow::{anyhow, Result};

pub const APP_DIR: &str = "kuluu";

pub fn config_file(name: &str) -> Result<PathBuf> {
    let base =
        dirs::config_dir().ok_or_else(|| anyhow!("could not resolve a user config directory"))?;
    Ok(base.join(APP_DIR).join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_uses_player_facing_dir() {
        let p = config_file("launcher.json").unwrap();
        assert!(p.ends_with("kuluu/launcher.json"), "got {}", p.display());
    }
}
