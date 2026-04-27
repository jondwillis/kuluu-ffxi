use ffxi_viewer_wire::{SceneDelta, SceneSnapshot, ViewerEvent};

pub trait SceneSource: Send + Sync + 'static {
    fn poll_snapshot(&mut self) -> Option<Box<SceneSnapshot>>;

    fn drain_deltas(&mut self) -> Vec<SceneDelta>;

    fn drain_events(&mut self) -> Vec<ViewerEvent>;
}
