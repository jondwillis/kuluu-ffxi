pub mod action;
pub mod anim;
pub mod archive;
pub mod autotranslate_names;
pub mod bone;
pub mod camera;
pub mod chunk;
pub mod cib;
pub mod client_profile;
pub mod d3m;
pub mod datid;
pub mod dmsg;
pub mod event_dat;
pub mod event_locate;
pub mod footstep;
pub mod ftable;
pub mod generator;
/// Sourced directly from ffxi-install rather than a crate dependency: an
/// ffxi-install crate dependency would drag its unconditional native deps
/// (unrar in particular, a C++ build via cc-rs) into every ffxi-dat consumer,
/// including the wasm32 target (kuluu-viewer-wasm -> kuluu-render ->
/// ffxi-actor/ffxi-dat), which has no C++ stdlib to link against.
pub mod install;
#[path = "../../ffxi-install/src/install_detect.rs"]
pub mod install_detect;
pub mod item_dat;
pub mod key_item;
pub mod kind;
pub mod main_dll;
pub mod map_image;
pub mod mmb;
pub mod mzb;
pub mod npc_names;
pub mod particle_gen;
pub mod pol1;
pub mod resource_dir;
pub mod scheduler;
pub mod sep;
pub mod skel;
pub mod skel_anim;
pub mod skel_mesh;
pub mod spell_info;
pub mod sprite_sheet;
pub mod sub_area;
pub mod sysmes;
pub mod texture;
pub mod ui_element;
pub mod vos2;
pub mod vtable;
pub mod weather;
pub mod zone_dat;
pub mod zone_interaction;

pub use archive::{discover_overlays, excode_client_at, DatLocation, DatRoot};
pub use camera::CameraResource;
pub use chunk::{walk, walk_tree, Chunk, ChunkNode, ChunkWalker};
pub use client_profile::{ClientProfile, ItemBlockLayout};
pub use item_dat::ItemStatic;
pub use key_item::KeyItemTable;
pub use kind::ChunkKind;
pub use npc_names::{compose_id, split_id, NpcNameTable, NPC_LIST_FILE_ID_BASE};

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum DatError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("FFXI_DAT_PATH environment variable not set")]
    EnvMissing,

    #[error("no FFXI install: {reason}")]
    NoInstall { reason: String },

    #[error("invalid table file {path}: expected size multiple of {stride}, got {len}")]
    InvalidTableSize {
        path: PathBuf,
        len: u64,
        stride: u64,
    },

    #[error("file_id {file_id} out of range (table has {table_len} entries)")]
    FileIdOutOfRange { file_id: u32, table_len: u32 },

    #[error("file_id {file_id} marked missing in VTABLE (app byte = 0)")]
    FileNotPresent { file_id: u32 },

    #[error("truncated chunk at offset {offset}: needed {needed} bytes, only {available} remain")]
    TruncatedChunk {
        offset: usize,
        needed: usize,
        available: usize,
    },

    #[error("MMB error: {0}")]
    Mmb(String),

    #[error("MZB error: {0}")]
    Mzb(String),

    #[error("Weather error: {0}")]
    Weather(String),

    #[error("RID error: {0}")]
    Rid(String),

    #[error("FFXiMain.dll marker {hint:#010x} not found")]
    DllMarkerNotFound { hint: u32 },
}

pub type Result<T> = std::result::Result<T, DatError>;

pub mod vehicle;
