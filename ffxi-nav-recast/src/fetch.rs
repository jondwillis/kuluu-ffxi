//! Fetch + cache for `LandSandBoat/xiNavmeshes` `.nav` files.
//!
//! ## Naming convention
//!
//! xiNavmeshes mostly names files by canonical zone name with
//! underscores (`West_Sarutabaruta.nav`). Three legacy zones are
//! numeric (`49.nav`, `133.nav`, `229.nav`). We probe numeric first
//! (cheap for the three exceptions and gives an early "not present"
//! signal for unknown ids), then fall back to the name from
//! `ffxi_nav::zone_name`.
//!
//! ## Cache
//!
//! Files land at `<dirs::cache_dir>/ffxi-agent/navmeshes/{filename}`
//! (typically `~/Library/Caches/ffxi-agent/navmeshes/` on macOS).
//! Once a file is on disk we never re-fetch — clear the directory to
//! refresh from upstream.

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;
use tracing::{debug, info, warn};

const REPO_RAW_BASE: &str = "https://raw.githubusercontent.com/LandSandBoat/xiNavmeshes/master";

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("could not locate user cache directory")]
    NoCacheDir,
    #[error("io error in cache `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("http error fetching `{url}`: {source}")]
    Http {
        url: String,
        #[source]
        source: Box<ureq::Error>,
    },
}

/// Path of the on-disk navmesh cache directory.
pub fn cache_dir() -> Result<PathBuf, FetchError> {
    let base = dirs::cache_dir().ok_or(FetchError::NoCacheDir)?;
    Ok(base.join("ffxi-agent").join("navmeshes"))
}

/// Resolve `zone_id` to a cached `.nav` file path. Downloads on first
/// access; returns `Ok(None)` if the zone has no navmesh upstream.
pub fn fetch(zone_id: u16) -> Result<Option<PathBuf>, FetchError> {
    let dir = cache_dir()?;
    fs::create_dir_all(&dir).map_err(|source| FetchError::Io {
        path: dir.clone(),
        source,
    })?;

    for filename in candidate_filenames(zone_id) {
        let local = dir.join(&filename);
        if local.exists() {
            debug!(zone_id, file = %filename, "navmesh cache hit");
            return Ok(Some(local));
        }
        match download_to(&filename, &local)? {
            DownloadOutcome::Saved => {
                info!(zone_id, file = %filename, "downloaded navmesh");
                return Ok(Some(local));
            }
            DownloadOutcome::NotFound => continue,
        }
    }
    debug!(zone_id, "no navmesh available upstream");
    Ok(None)
}

fn candidate_filenames(zone_id: u16) -> Vec<String> {
    let mut out = vec![format!("{zone_id}.nav")];
    if let Some(name) = ffxi_nav::zone_name(zone_id) {
        out.push(format!("{name}.nav"));
    }
    out
}

enum DownloadOutcome {
    Saved,
    NotFound,
}

fn download_to(filename: &str, dest: &PathBuf) -> Result<DownloadOutcome, FetchError> {
    let url = format!("{REPO_RAW_BASE}/{filename}");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .build();
    match agent.get(&url).call() {
        Ok(resp) => {
            let mut buf = Vec::new();
            resp.into_reader()
                .read_to_end(&mut buf)
                .map_err(|source| FetchError::Io {
                    path: dest.clone(),
                    source,
                })?;
            fs::write(dest, &buf).map_err(|source| FetchError::Io {
                path: dest.clone(),
                source,
            })?;
            Ok(DownloadOutcome::Saved)
        }
        Err(ureq::Error::Status(404, _)) => Ok(DownloadOutcome::NotFound),
        Err(other) => {
            warn!(url = %url, error = %other, "navmesh fetch failed");
            Err(FetchError::Http {
                url,
                source: Box::new(other),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `49`, `133`, `229` are the three legacy numeric `.nav` filenames
    /// in xiNavmeshes. Anything else should also generate a name candidate.
    #[test]
    fn candidate_filenames_includes_numeric_and_name() {
        let cands = candidate_filenames(133); // Tahrongi_Canyon
        assert!(cands.contains(&"133.nav".to_string()));
        assert!(cands.iter().any(|c| c.ends_with(".nav") && c != "133.nav"));
    }

    #[test]
    fn candidate_filenames_unknown_zone_only_numeric() {
        // Zone 9999 is outside the table.
        let cands = candidate_filenames(9999);
        assert_eq!(cands, vec!["9999.nav".to_string()]);
    }
}
