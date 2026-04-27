use std::sync::atomic::{AtomicU64, Ordering};

static MODEL_LOADS: AtomicU64 = AtomicU64::new(0);
static NAMEPLATE_RASTERS: AtomicU64 = AtomicU64::new(0);

pub fn note_model_load() {
    MODEL_LOADS.fetch_add(1, Ordering::Relaxed);
}

pub fn note_nameplate_raster() {
    NAMEPLATE_RASTERS.fetch_add(1, Ordering::Relaxed);
}

pub fn model_loads() -> u64 {
    MODEL_LOADS.load(Ordering::Relaxed)
}

pub fn nameplate_rasters() -> u64 {
    NAMEPLATE_RASTERS.load(Ordering::Relaxed)
}
