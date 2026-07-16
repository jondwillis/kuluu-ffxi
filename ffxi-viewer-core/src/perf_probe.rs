use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static MODEL_LOADS: AtomicU64 = AtomicU64::new(0);
static NAMEPLATE_RASTERS: AtomicU64 = AtomicU64::new(0);
static DEBUG_PROBE_NS: AtomicU64 = AtomicU64::new(0);
static RENDER_PREP_NS: AtomicU64 = AtomicU64::new(0);
static RENDER_GRAPH_NS: AtomicU64 = AtomicU64::new(0);
static RENDER_TOTAL_NS: AtomicU64 = AtomicU64::new(0);

/// Sub-spans of the render-prep fence (see `RenderSpanStamp` in the client):
/// xtr = extract→end of ExtractCommands, ast = PrepareAssets+PrepareMeshes,
/// vws = CreateViews+Specialize+PrepareViews, que = Queue+PhaseSort,
/// prp = Prepare (resources, batching, bind groups).
pub const RPREP_SPAN_LABELS: [&str; 5] = ["xtr", "ast", "vws", "que", "prp"];

static RPREP_SPANS_NS: [AtomicU64; 5] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

pub fn note_rprep_span(idx: usize, elapsed: Duration) {
    RPREP_SPANS_NS[idx].fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
}

pub fn rprep_spans_ns() -> [u64; 5] {
    core::array::from_fn(|i| RPREP_SPANS_NS[i].load(Ordering::Relaxed))
}

pub fn note_model_load() {
    MODEL_LOADS.fetch_add(1, Ordering::Relaxed);
}

pub fn note_nameplate_raster() {
    NAMEPLATE_RASTERS.fetch_add(1, Ordering::Relaxed);
}

pub fn note_debug_probe(elapsed: Duration) {
    DEBUG_PROBE_NS.fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
}

pub fn model_loads() -> u64 {
    MODEL_LOADS.load(Ordering::Relaxed)
}

pub fn nameplate_rasters() -> u64 {
    NAMEPLATE_RASTERS.load(Ordering::Relaxed)
}

pub fn debug_probe_ns() -> u64 {
    DEBUG_PROBE_NS.load(Ordering::Relaxed)
}

pub fn note_render_prep(elapsed: Duration) {
    RENDER_PREP_NS.fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
}

pub fn note_render_graph(elapsed: Duration) {
    RENDER_GRAPH_NS.fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
}

pub fn note_render_total(elapsed: Duration) {
    RENDER_TOTAL_NS.fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
}

pub fn render_prep_ns() -> u64 {
    RENDER_PREP_NS.load(Ordering::Relaxed)
}

pub fn render_graph_ns() -> u64 {
    RENDER_GRAPH_NS.load(Ordering::Relaxed)
}

pub fn render_total_ns() -> u64 {
    RENDER_TOTAL_NS.load(Ordering::Relaxed)
}
