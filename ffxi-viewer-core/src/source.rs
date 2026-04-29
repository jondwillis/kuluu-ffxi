//! Pluggable state source — the seam between this crate (Bevy ECS, render
//! systems, HUD) and whoever happens to be feeding it.
//!
//! Two concrete implementations live elsewhere:
//! - `ffxi-client/src/view_native/bridge.rs::NativeSource` reads the
//!   in-process tokio `watch<SessionState>` + `broadcast<AgentEvent>`.
//! - `ffxi-viewer-wasm/src/source.rs::WasmSource` decodes postcard
//!   `Frame`s from a `gloo_net` WebSocket.
//!
//! Both impls poll synchronously — Bevy systems are sync, and the trait
//! doesn't import any runtime. The native side bridges tokio→sync via
//! `try_recv` / `has_changed`; the wasm side bridges gloo-net's async
//! WebSocket→sync via a forwarding task plus a `flume` channel.

use ffxi_viewer_wire::{SceneDelta, SceneSnapshot, ViewerEvent};

/// Sync, non-blocking source of viewer state. Implementations are inserted
/// as a Bevy `Resource` (the `Resource` bound is enforced at the system
/// boundary — declaring it here would make the trait less reusable).
pub trait SceneSource: Send + Sync + 'static {
    /// Return the freshest full snapshot if one has arrived since the last
    /// call, else `None`. The producer side decides when a new snapshot is
    /// "fresh" — the native bridge uses `watch::has_changed`, the wasm
    /// bridge returns whatever the most recent `Frame::Snapshot` was.
    ///
    /// Boxed because `SceneSnapshot` is several hundred bytes and crosses
    /// the trait boundary on the hot path.
    fn poll_snapshot(&mut self) -> Option<Box<SceneSnapshot>>;

    /// Drain any deltas accumulated since last call. Stage 2.0 returns
    /// empty; Stage 2.1 is when this becomes load-bearing.
    fn drain_deltas(&mut self) -> Vec<SceneDelta>;

    /// Drain any high-signal events (TellReceived, EngagedBy, …) accumulated
    /// since last call. The viewer uses these for transient HUD effects
    /// (toast notifications, aggro flashes) — they don't mutate `SceneState`.
    fn drain_events(&mut self) -> Vec<ViewerEvent>;
}
