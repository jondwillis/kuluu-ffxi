//! Native Rust parser for FFXI client DAT files.
//!
//! Phase 0: VTABLE/FTABLE → DAT path resolution. Built against
//! POLUtils (Apache-2.0) as the canonical reference, never against
//! the unlicensed galkareeve/TeoTwawki repos.
//!
//! Resolution algorithm (see POLUtils `PlayOnline.FFXI/FFXI.cs`):
//!   VTABLE[file_id] = app_byte (u8)
//!     0 = file does not exist
//!     n>0: App = n - 1; rom_dir = "ROM" if App==0 else format!("ROM{}", App+1)
//!   FTABLE[2 * file_id] = file_dir (u16 LE)
//!     dir  = file_dir >> 7   (upper 9 bits)
//!     file = file_dir & 0x7F (lower 7 bits)
//!   path = {install}/FINAL FANTASY XI/{rom_dir}/{dir}/{file}.DAT

pub mod action;
pub mod anim;
pub mod archive;
pub mod bone;
pub mod chunk;
pub mod cib;
pub mod d3m;
pub mod ftable;
pub mod generator;
pub mod item_dat;
pub mod kind;
pub mod map_image;
pub mod mmb;
pub mod mzb;
pub mod npc_names;
pub mod scheduler;
pub mod sep;
pub mod texture;
pub mod vos2;
pub mod vtable;
pub mod weather;
pub mod zone_dat;

pub use archive::{DatLocation, DatRoot};
pub use chunk::{walk, walk_tree, Chunk, ChunkNode, ChunkWalker};
pub use item_dat::ItemStatic;
pub use kind::ChunkKind;
pub use npc_names::{split_id, NpcNameTable, NPC_LIST_FILE_ID_BASE};

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
}

pub type Result<T> = std::result::Result<T, DatError>;
