//! Item-icon DAT plumbing for the detail panel — mirrors the status-icon
//! ribbon's [`crate::hud::status_ribbon::StatusIconDatRoot`] /
//! [`crate::hud::status_ribbon::StatusIconCache`] pair exactly, but for
//! the retail *item* DAT family.
//!
//! Item metadata is two-tier (see [`crate::hud::item_meta`]). The STATIC
//! tier — name, slot/job/race restrictions, flags, recast, and the
//! embedded 32bpp icon — lives in the retail client item DATs and is
//! parsed by `ffxi_dat::item_dat` (owned by the item-data feature agent).
//! This file owns the Bevy side of that: a front-end-provided
//! [`ItemDatRoot`] handle and a persistent [`ItemIconCache`] that decodes
//! one item icon on demand via [`ffxi_dat::item_dat::icon_at`] and keeps
//! the resulting [`Handle<Image>`] for the process lifetime.
//!
//! Like the status-icon sheet, the item DAT is install-invariant: the same
//! icon for item N is identical across characters, zones, and logins, so
//! the cache is intentionally **not** drained by
//! `despawn_ingame_entities`. The UI nodes that display the icon are
//! session-scoped (`InGameEntity`) and drain normally.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::DatRoot;

/// Front-end-provided handle for resolving the retail item DAT to a disk
/// path. The front-end inserts this from the same `DatRoot` it uses for
/// the minimap / status icons. Without it (or without a reachable
/// install), the detail panel degrades to the LSB-scraped label-only
/// fallback and shows no icon.
#[derive(Resource, Default, Clone)]
pub struct ItemDatRoot(pub Option<Arc<DatRoot>>);

/// Decoded item-icon textures, keyed by `item_no`. `None` marks an id we
/// tried and failed to decode (out-of-range, truncated block, or no DAT)
/// so we don't re-attempt every frame.
///
/// This is a **persistent asset cache**, not session-scoped game state:
/// the item DAT is identical across characters / zones / logins, so the
/// textures are loaded once and kept for the process lifetime — like a
/// font atlas, and exactly like [`crate::hud::status_ribbon::StatusIconCache`].
/// Intentionally *not* drained by `despawn_ingame_entities`.
#[derive(Resource, Default)]
pub struct ItemIconCache {
    /// Whole item DAT, read once on first need.
    dat: Option<Arc<Vec<u8>>>,
    /// Set once a load attempt failed so we stop retrying the file read.
    dat_unavailable: bool,
    /// Per-item texture handle; `None` = decode failed (no icon shown).
    icons: HashMap<u16, Option<Handle<Image>>>,
}

impl ItemIconCache {
    /// Resolve (and lazily decode) the texture for `item_no`. Returns
    /// `None` when the DAT is unreachable or the item doesn't decode — the
    /// caller then hides the icon node.
    pub fn ensure(
        &mut self,
        item_no: u16,
        dat_root: &ItemDatRoot,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        if let Some(slot) = self.icons.get(&item_no) {
            return slot.clone();
        }
        let handle = self
            .dat_bytes(dat_root)
            .and_then(|bytes| ffxi_dat::item_dat::icon_at(&bytes, item_no))
            .map(|img| upload_icon(img, images));
        self.icons.insert(item_no, handle.clone());
        handle
    }

    /// Lazily read the item DAT once, caching the bytes. Returns `None`
    /// (and latches `dat_unavailable`) if the root is unset or the file
    /// can't be read.
    fn dat_bytes(&mut self, dat_root: &ItemDatRoot) -> Option<Arc<Vec<u8>>> {
        if let Some(bytes) = &self.dat {
            return Some(bytes.clone());
        }
        if self.dat_unavailable {
            return None;
        }
        let root = match &dat_root.0 {
            Some(r) => r,
            // Don't latch unavailable on a missing root — the front-end
            // may insert the resource a frame later.
            None => return None,
        };
        // The retail item DAT family is split across several files by
        // category (general / weapons / armor). `ITEM_DAT_FILE_ID` lists
        // the well-known candidates; the resolver + a stride-multiple
        // length check is the source of truth (per the parser's contract).
        // Take the first candidate that resolves to a readable,
        // stride-aligned file. (v1 reads the single general-items DAT;
        // multi-DAT category routing can layer on later — the inventory
        // packets use one id space, and the common items live in the
        // first file.)
        let loaded = ffxi_dat::item_dat::ITEM_DAT_FILE_ID
            .iter()
            .find_map(|&file_id| {
                let path = root.resolve(file_id).ok()?.path_under(root.root());
                let bytes = std::fs::read(path).ok()?;
                (bytes.len() % ffxi_dat::item_dat::ITEM_BLOCK_STRIDE == 0 && !bytes.is_empty())
                    .then_some(bytes)
            });
        match loaded {
            Some(bytes) => {
                let arc = Arc::new(bytes);
                self.dat = Some(arc.clone());
                Some(arc)
            }
            None => {
                warn!(
                    "item icons: no item DAT in {:?} resolved/readable; label-only fallback",
                    ffxi_dat::item_dat::ITEM_DAT_FILE_ID
                );
                self.dat_unavailable = true;
                None
            }
        }
    }

    /// Whether the cache has latched the DAT as unreachable. The detail
    /// panel reads this to decide between "icon pending" and "no install".
    pub fn dat_unavailable(&self) -> bool {
        self.dat_unavailable
    }

    /// Public handle on the lazily-read DAT bytes, so the detail panel can
    /// reuse the same cached file read for `ffxi_dat::item_dat::lookup`
    /// (static metadata) instead of opening the file a second time. Same
    /// non-latching-on-missing-root semantics as the icon path.
    pub fn dat_bytes_for_static(&mut self, dat_root: &ItemDatRoot) -> Option<Arc<Vec<u8>>> {
        self.dat_bytes(dat_root)
    }
}

/// Upload a decoded item icon as a Bevy texture. Linear sampling so the
/// 32px source reads cleanly when drawn larger in the detail panel.
fn upload_icon(
    img: ffxi_dat::map_image::GraphicImage,
    images: &mut Assets<Image>,
) -> Handle<Image> {
    let mut image = Image::new(
        Extent3d {
            width: img.width,
            height: img.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        img.rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    images.add(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty cache with no DAT root yields `None` without latching
    /// `dat_unavailable` — the front-end may insert the root a frame
    /// later, and we must retry then. Mirrors the status-ribbon
    /// invariant.
    #[test]
    fn cache_without_root_does_not_latch() {
        let mut cache = ItemIconCache::default();
        let root = ItemDatRoot(None);
        assert!(cache.dat_bytes(&root).is_none());
        assert!(!cache.dat_unavailable, "must retry once root is provided");
    }
}
