//! Event-script staging: the screen fade and cutscene mode.
//!
//! The session produces [`ViewerEvent::CutsceneStarted`] / [`ViewerEvent::Cutscene`] /
//! [`ViewerEvent::CutsceneEnded`]; this module renders the two effects that are the
//! renderer's own — the persistent full-screen colour a `fdo0`/`fdi0` scheduler drives,
//! and the HUD/camera surrender a `CameraLock` cue asks for.
//!
//! Retail latches the fade: `ScreenColorDriveTask`'s destructor snaps the screen colour to
//! the destination and nothing drives it back on its own, so every session exit here must
//! end at [`SCREEN_TINT_IDENTITY`] or the player is left behind a black screen.

use std::collections::HashMap;

use bevy::picking::Pickable;
use bevy::prelude::*;

use ffxi_dat::scheduler::Scheduler;
use ffxi_event::{FourCc, SCHEDULER_FADE_DAT_ID, SCHEDULER_TAG_FADE_IN, SCHEDULER_TAG_FADE_OUT};
use kuluu_snapshot::{CutsceneCue, ViewerEvent};

use crate::hud_hide::{HudHidden, HudHideExempt};
use crate::scheduler_runtime::ROUTINE_FPS;
use crate::snapshot::EventLog;

/// The screen colour that leaves the scene alone, in
/// [`ffxi_dat::scheduler::ScreenColor::tint`] units.
pub const SCREEN_TINT_IDENTITY: [f32; 4] = [1.0; 4];

/// Under the zone-load overlay (`kuluu` `zone_transition`, `i32::MAX`), which is a
/// loading screen rather than an in-scene effect and has to cover the cutscene fade too.
const SCREEN_FADE_Z: i32 = i32::MAX - 1;

/// research/XIClient `CMoSchedulerTask::CMoSchedulerTask` — the 0x45 duration operand is a
/// total-frame override: `speed_ratio = operand / total_frame`, except 0 (play as authored)
/// and 1 (loop, also as authored).
const SCHEDULER_DURATION_LOOP: u16 = 1;

/// Backstop only: no scene should ever reach it. The longest single `0x1C` WAIT any
/// shipped event authors is 3370 units of 1/60s (56.2s), so a legitimate hold cannot
/// pass this, and anything that does is a release that never arrived.
const SCREEN_FADE_MAX_HOLD_SECS: f32 = 90.0;

pub fn scheduler_speed_ratio(duration_override: u16, total_frames: u32) -> f32 {
    if duration_override <= SCHEDULER_DURATION_LOOP || total_frames == 0 {
        return 1.0;
    }
    f32::from(duration_override) / total_frames as f32
}

/// One `ScreenColorDrive` stage, on the renderer's clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FadeStep {
    pub start_secs: f32,
    pub duration_secs: f32,
    pub dest: [f32; 4],
}

/// The `ScreenColorDrive` stages of one scheduler routine, in fire order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FadeProgram {
    pub steps: Vec<FadeStep>,
}

impl FadeProgram {
    pub fn from_scheduler(routine: &Scheduler, speed_ratio: f32) -> Self {
        let mut steps: Vec<FadeStep> = routine
            .stages
            .iter()
            .filter_map(|timed| {
                let dest = timed.stage.screen_color?;
                Some(FadeStep {
                    start_secs: timed.frame as f32 * speed_ratio / ROUTINE_FPS,
                    duration_secs: f32::from(timed.stage.duration_frames) * speed_ratio
                        / ROUTINE_FPS,
                    dest: dest.tint(),
                })
            })
            .collect();
        steps.sort_by(|a, b| a.start_secs.total_cmp(&b.start_secs));
        Self { steps }
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn total_secs(&self) -> f32 {
        self.steps
            .iter()
            .map(|s| s.start_secs + s.duration_secs)
            .fold(0.0, f32::max)
    }

    /// The colour this program latches at once its last step has run.
    pub fn latched(&self) -> Option<[f32; 4]> {
        self.steps.last().map(|s| s.dest)
    }

    /// Judged on the same RGB mean [`ScreenFade::opacity`] composites with, not on
    /// all four channels: the authored `fdi0` lands on alpha 0, so an exact compare
    /// against [`SCREEN_TINT_IDENTITY`] never matches the one program that does end
    /// clear.
    fn ends_clear(&self) -> bool {
        self.latched()
            .is_some_and(|dest| (dest[0] + dest[1] + dest[2]) / 3.0 >= 1.0)
    }
}

#[derive(Debug, Clone)]
struct FadeRun {
    from: [f32; 4],
    program: FadeProgram,
    elapsed: f32,
}

impl FadeRun {
    /// research/XIClient `ScreenColorDriveTask::OnMove` — `src` is captured as
    /// `current - dst` when the stage fires, so each step drives from wherever the
    /// previous one left the screen and holds its destination afterwards.
    fn tint(&self) -> [f32; 4] {
        let mut color = self.from;
        for step in &self.program.steps {
            if self.elapsed <= step.start_secs {
                break;
            }
            let progress = if step.duration_secs > 0.0 {
                ((self.elapsed - step.start_secs) / step.duration_secs).clamp(0.0, 1.0)
            } else {
                1.0
            };
            color = std::array::from_fn(|i| color[i] + (step.dest[i] - color[i]) * progress);
        }
        color
    }

    fn finished(&self) -> bool {
        self.elapsed >= self.program.total_secs()
    }
}

/// The persistent screen colour, and whatever is currently driving it.
#[derive(Resource, Debug, Clone)]
pub struct ScreenFade {
    tint: [f32; 4],
    run: Option<FadeRun>,
    held_secs: f32,
}

impl Default for ScreenFade {
    fn default() -> Self {
        Self {
            tint: SCREEN_TINT_IDENTITY,
            run: None,
            held_secs: 0.0,
        }
    }
}

impl ScreenFade {
    pub fn tint(&self) -> [f32; 4] {
        self.run.as_ref().map_or(self.tint, FadeRun::tint)
    }

    /// How much of the scene the fade is currently swallowing, 0 clear .. 1 black. The retail
    /// composite is a `D3DTOP_MODULATE2X` of the scene by the screen colour, which UI has no
    /// blend mode for; every authored fade destination is neutral grey, for which multiplying
    /// by `t` and laying black over at `1 - t` are the same picture.
    pub fn opacity(&self) -> f32 {
        let tint = self.tint();
        let mean = (tint[0] + tint[1] + tint[2]) / 3.0;
        (1.0 - mean).clamp(0.0, 1.0)
    }

    pub fn is_clear(&self) -> bool {
        self.opacity() <= 0.0
    }

    pub fn start(&mut self, program: &FadeProgram) {
        if program.is_empty() {
            return;
        }
        self.run = Some(FadeRun {
            from: self.tint(),
            program: program.clone(),
            elapsed: 0.0,
        });
    }

    /// Snap back to an untinted screen. The teardown for exits with no picture to preserve.
    pub fn clear(&mut self) {
        self.tint = SCREEN_TINT_IDENTITY;
        self.run = None;
        self.held_secs = 0.0;
    }

    /// End the session's hold on the screen. Plays the authored fade-in when one is
    /// available so a scene that ended dark recovers the way retail's does, and snaps
    /// otherwise — either way the fade cannot outlive the event that started it.
    pub fn release(&mut self, fade_in: Option<&FadeProgram>) {
        if self.run.as_ref().is_some_and(|r| r.program.ends_clear()) {
            return;
        }
        if self.run.is_none() && self.tint == SCREEN_TINT_IDENTITY {
            return;
        }
        match fade_in.filter(|p| p.ends_clear()) {
            Some(program) => self.start(program),
            None => self.clear(),
        }
    }

    pub fn tick(&mut self, dt: f32) {
        if let Some(run) = self.run.as_mut() {
            run.elapsed += dt;
            if run.finished() {
                self.tint = run.tint();
                self.run = None;
            }
            self.held_secs = 0.0;
            return;
        }

        // A latched tint is only ever lifted by an event the broadcast channel is
        // free to drop under lag, so the release is not guaranteed to arrive. Time
        // out rather than leave the player blind behind a screen nothing owns.
        if self.is_clear() {
            self.held_secs = 0.0;
            return;
        }
        self.held_secs += dt;
        if self.held_secs >= SCREEN_FADE_MAX_HOLD_SECS {
            warn!(
                held_secs = self.held_secs,
                "screen fade outlived its event; clearing"
            );
            self.clear();
        }
    }
}

/// What the running event script has taken from the player.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CutsceneMode {
    pub active: bool,
    pub camera_locked: bool,
}

impl CutsceneMode {
    fn end(&mut self) {
        *self = Self::default();
    }
}

/// True while nothing is holding the camera, so a caller can `run_if` its input systems on
/// it. `Option` because the mouse-camera systems are also registered by tests and by the
/// wasm viewer, neither of which builds this plugin.
pub fn player_camera_allowed(mode: Option<Res<CutsceneMode>>) -> bool {
    !mode.is_some_and(|m| m.camera_locked)
}

/// The scheduler DAT the fade tags live in, kept per-consumer like every other `*DatRoot`
/// so a launcher DAT-path change re-reads it (`view_native::insert_dat_roots`).
#[derive(Resource, Default, Clone)]
pub struct CutsceneFadeDatRoot(pub Option<std::sync::Arc<ffxi_dat::DatRoot>>);

/// The fade routines of [`SCHEDULER_FADE_DAT_ID`], keyed by their scheduler tag. Process-wide
/// like the DAT root it is read from: nothing in it is session state, so it survives a logout.
#[derive(Resource, Default)]
pub struct FadePrograms {
    by_tag: HashMap<FourCc, FadeProgram>,
}

impl FadePrograms {
    pub fn get(&self, tag: FourCc) -> Option<&FadeProgram> {
        self.by_tag.get(&tag)
    }

    pub fn fade_in(&self) -> Option<&FadeProgram> {
        self.get(SCHEDULER_TAG_FADE_IN)
    }

    pub fn insert(&mut self, tag: FourCc, program: FadeProgram) {
        self.by_tag.insert(tag, program);
    }
}

#[derive(Component)]
pub struct ScreenFadeOverlay;

/// Spawned for the process, not for the session (so no `InGameEntity`): a cue must never find
/// the overlay missing, and the session-scoped half is [`ScreenFade`], which
/// `view_native::drain_cutscene_state` resets at `OnExit(InGame)`.
pub fn spawn_screen_fade_overlay(mut commands: Commands) {
    commands.spawn((
        ScreenFadeOverlay,
        // The fade is the one thing cutscene mode must not hide along with the HUD.
        HudHideExempt,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(0.0),
            left: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            display: Display::None,
            ..default()
        },
        BackgroundColor(Color::NONE),
        GlobalZIndex(SCREEN_FADE_Z),
        Pickable::IGNORE,
    ));
}

pub fn tick_screen_fade(time: Res<Time>, mut fade: ResMut<ScreenFade>) {
    if fade.run.is_some() {
        let dt = time.delta_secs();
        fade.tick(dt);
    }
}

pub fn apply_screen_fade(
    fade: Res<ScreenFade>,
    mut overlay: Query<(&mut BackgroundColor, &mut Node), With<ScreenFadeOverlay>>,
) {
    let opacity = fade.opacity();
    for (mut bg, mut node) in overlay.iter_mut() {
        let want_display = if opacity > 0.0 {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want_display {
            node.display = want_display;
        }
        if bg.0.alpha() != opacity {
            bg.0 = Color::BLACK.with_alpha(opacity);
        }
    }
}

pub fn apply_cutscene_hud_hide(mode: Res<CutsceneMode>, mut hidden: ResMut<HudHidden>) {
    if hidden.cutscene != mode.camera_locked {
        hidden.cutscene = mode.camera_locked;
    }
}

pub fn drain_cutscene_events(
    events: Res<EventLog>,
    programs: Res<FadePrograms>,
    mut cursor: Local<u64>,
    mut mode: ResMut<CutsceneMode>,
    mut fade: ResMut<ScreenFade>,
) {
    let total = events.pushed_total;
    let first_global = total.saturating_sub(events.recent.len() as u64);
    for g in (*cursor).max(first_global)..total {
        match &events.recent[(g - first_global) as usize] {
            ViewerEvent::CutsceneStarted { .. } => mode.active = true,
            ViewerEvent::Cutscene { cue } => apply_cue(cue, &programs, &mut mode, &mut fade),
            ViewerEvent::CutsceneEnded => {
                mode.end();
                fade.release(programs.fade_in());
            }
            // Belt and braces: the producer guarantees a CutsceneEnded on both of these, so
            // reaching them with the screen still held means that guarantee broke.
            ViewerEvent::ZoneChanged { .. } | ViewerEvent::Disconnected { .. } => {
                mode.end();
                fade.clear();
            }
            _ => {}
        }
    }
    *cursor = total;
}

fn apply_cue(
    cue: &CutsceneCue,
    programs: &FadePrograms,
    mode: &mut CutsceneMode,
    fade: &mut ScreenFade,
) {
    match *cue {
        CutsceneCue::CameraLock { lock } => mode.camera_locked = lock,
        CutsceneCue::Scheduler {
            dat_id,
            tag,
            duration,
            ..
        } if dat_id == SCHEDULER_FADE_DAT_ID => {
            let Some(program) = programs.get(tag) else {
                return;
            };
            let ratio = scheduler_speed_ratio(duration, fade_total_frames(program));
            if ratio == 1.0 {
                fade.start(program);
            } else {
                fade.start(&scaled(program, ratio));
            }
        }
        _ => {}
    }
}

fn fade_total_frames(program: &FadeProgram) -> u32 {
    (program.total_secs() * ROUTINE_FPS).round() as u32
}

fn scaled(program: &FadeProgram, ratio: f32) -> FadeProgram {
    FadeProgram {
        steps: program
            .steps
            .iter()
            .map(|s| FadeStep {
                start_secs: s.start_secs * ratio,
                duration_secs: s.duration_secs * ratio,
                ..*s
            })
            .collect(),
    }
}

/// The tags whose routines the renderer drives the screen with. Both live in
/// [`SCHEDULER_FADE_DAT_ID`] — XIClient's own zone fade starts the same two out of the same
/// file (`GameManager::CliLocalTask`, `StartSchedulerFromFile(0x78B8, '0odf'/'0idf', ...)`).
const FADE_TAGS: [FourCc; 2] = [SCHEDULER_TAG_FADE_OUT, SCHEDULER_TAG_FADE_IN];

#[cfg(not(target_arch = "wasm32"))]
pub fn load_fade_programs(root: Res<CutsceneFadeDatRoot>, mut programs: ResMut<FadePrograms>) {
    *programs = FadePrograms::default();
    let Some(root) = root.0.as_ref() else {
        return;
    };
    let Ok(location) = root.resolve(SCHEDULER_FADE_DAT_ID) else {
        warn!("cutscene: fade scheduler DAT {SCHEDULER_FADE_DAT_ID} does not resolve");
        return;
    };
    let Ok(bytes) = std::fs::read(location.path_under(root)) else {
        warn!("cutscene: fade scheduler DAT {SCHEDULER_FADE_DAT_ID} is unreadable");
        return;
    };
    for (tag, program) in fade_programs_in(&bytes) {
        programs.insert(tag, program);
    }
}

/// The fade routines of an already-read [`SCHEDULER_FADE_DAT_ID`] body.
pub fn fade_programs_in(dat: &[u8]) -> Vec<(FourCc, FadeProgram)> {
    ffxi_dat::walk(dat)
        .flatten()
        .filter(|chunk| {
            chunk.kind == ffxi_dat::kind::ChunkKind::Scheduler as u8
                && FADE_TAGS.contains(&chunk.name)
        })
        .filter_map(|chunk| {
            let routine = Scheduler::parse(chunk.name, chunk.data).ok()?;
            let program = FadeProgram::from_scheduler(&routine, 1.0);
            (!program.is_empty()).then_some((chunk.name, program))
        })
        .collect()
}

pub struct CutscenePlugin;

impl Plugin for CutscenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CutsceneMode>()
            .init_resource::<ScreenFade>()
            .init_resource::<FadePrograms>()
            .init_resource::<CutsceneFadeDatRoot>()
            .init_resource::<HudHidden>()
            .add_systems(Startup, spawn_screen_fade_overlay)
            .add_systems(
                Update,
                (
                    drain_cutscene_events,
                    apply_cutscene_hud_hide,
                    tick_screen_fade,
                    apply_screen_fade,
                )
                    .chain(),
            );

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            load_fade_programs.run_if(resource_exists_and_changed::<CutsceneFadeDatRoot>),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use ffxi_dat::scheduler::{
        SchedulerStage, ScreenColor, StageKind, TimedStage, SCREEN_COLOR_UNIT,
    };

    // The DAT-authored destinations of ROM/62/110.DAT's fdo0/fdi0, asserted against the real
    // file by `real_dat_fade_tags_drive_to_black_and_back`.
    const FADE_OUT_DEST: [u8; 4] = [0, 0, 0, SCREEN_COLOR_UNIT];
    const FADE_IN_DEST: [u8; 4] = [
        SCREEN_COLOR_UNIT,
        SCREEN_COLOR_UNIT,
        SCREEN_COLOR_UNIT,
        0x00,
    ];
    const FADE_FRAMES: u16 = 30;

    fn screen_color_routine(name: FourCc, rgba: [u8; 4]) -> Scheduler {
        Scheduler {
            name,
            stages: vec![TimedStage {
                frame: 0,
                stage: SchedulerStage {
                    kind: StageKind::ScreenColorDrive,
                    raw_type: 0x0F,
                    delay_frames: FADE_FRAMES,
                    duration_frames: FADE_FRAMES,
                    id: [0; 4],
                    max_loops: 0,
                    transition_in: 0,
                    transition_out: 0,
                    model_transform: None,
                    follow_points: None,
                    screen_color: Some(ScreenColor { rgba }),
                    actor_fade: None,
                    idle_transition_time: None,
                    flinch_duration: None,
                    random_group: None,
                    local_dir: ffxi_dat::scheduler::NO_LOCAL_DIR,
                },
            }],
        }
    }

    fn synthetic_programs() -> FadePrograms {
        let mut programs = FadePrograms::default();
        for (tag, rgba) in [
            (SCHEDULER_TAG_FADE_OUT, FADE_OUT_DEST),
            (SCHEDULER_TAG_FADE_IN, FADE_IN_DEST),
        ] {
            programs.insert(
                tag,
                FadeProgram::from_scheduler(&screen_color_routine(tag, rgba), 1.0),
            );
        }
        programs
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<EventLog>()
            .init_resource::<HudHidden>()
            .init_resource::<CutsceneMode>()
            .init_resource::<ScreenFade>()
            .insert_resource(synthetic_programs())
            .add_systems(Startup, spawn_screen_fade_overlay)
            .add_systems(
                Update,
                (
                    drain_cutscene_events,
                    apply_cutscene_hud_hide,
                    tick_screen_fade,
                    apply_screen_fade,
                )
                    .chain(),
            );
        app
    }

    fn push(app: &mut App, event: ViewerEvent) {
        app.world_mut().resource_mut::<EventLog>().push(event);
    }

    fn fade_cue(tag: FourCc) -> ViewerEvent {
        ViewerEvent::Cutscene {
            cue: CutsceneCue::Scheduler {
                dat_id: SCHEDULER_FADE_DAT_ID,
                actor: kuluu_snapshot::CutsceneActor::LocalPlayer,
                partner: kuluu_snapshot::CutsceneActor::LocalPlayer,
                tag,
                duration: ffxi_event::SCHEDULER_DURATION_FROM_DAT,
            },
        }
    }

    fn step(app: &mut App, frames: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(frames / ROUTINE_FPS));
        app.update();
    }

    fn overlay_alpha(app: &mut App) -> f32 {
        let mut overlay = app
            .world_mut()
            .query_filtered::<&BackgroundColor, With<ScreenFadeOverlay>>();
        let bg = overlay.single(app.world()).expect("one fade overlay");
        bg.0.alpha()
    }

    #[test]
    fn a_fade_cue_drives_the_screen_to_black_and_latches_there() {
        let mut app = test_app();
        step(&mut app, 0.0);
        assert_eq!(overlay_alpha(&mut app), 0.0, "starts clear");

        push(&mut app, ViewerEvent::CutsceneStarted { event_id: 599 });
        push(&mut app, fade_cue(SCHEDULER_TAG_FADE_OUT));
        step(&mut app, FADE_FRAMES as f32 / 2.0);
        let mid = overlay_alpha(&mut app);
        assert!(mid > 0.0 && mid < 1.0, "partially opaque mid-stage: {mid}");

        step(&mut app, FADE_FRAMES as f32 / 2.0);
        assert_eq!(
            overlay_alpha(&mut app),
            1.0,
            "fully black at the authored duration"
        );

        // The drive task's destructor snaps the field to the destination and nothing
        // else writes it, so the screen stays black with no driver left.
        step(&mut app, FADE_FRAMES as f32 * 10.0);
        assert_eq!(overlay_alpha(&mut app), 1.0, "latched");

        push(&mut app, fade_cue(SCHEDULER_TAG_FADE_IN));
        step(&mut app, FADE_FRAMES as f32 / 2.0);
        let mid_in = overlay_alpha(&mut app);
        assert!(mid_in > 0.0 && mid_in < 1.0, "fading back in: {mid_in}");

        step(&mut app, FADE_FRAMES as f32 / 2.0);
        assert_eq!(overlay_alpha(&mut app), 0.0, "returns to clear");
    }

    #[test]
    fn the_camera_lock_hides_the_hud_and_gates_player_camera_input() {
        let mut app = test_app();
        push(&mut app, ViewerEvent::CutsceneStarted { event_id: 599 });
        push(
            &mut app,
            ViewerEvent::Cutscene {
                cue: CutsceneCue::CameraLock { lock: true },
            },
        );
        step(&mut app, 1.0);
        assert!(app.world().resource::<HudHidden>().cutscene);
        assert!(app.world().resource::<CutsceneMode>().camera_locked);

        push(
            &mut app,
            ViewerEvent::Cutscene {
                cue: CutsceneCue::CameraLock { lock: false },
            },
        );
        step(&mut app, 1.0);
        assert!(!app.world().resource::<HudHidden>().cutscene);
        assert!(!app.world().resource::<CutsceneMode>().camera_locked);
    }

    #[test]
    fn a_session_that_ends_black_recovers_over_the_authored_fade_in() {
        let mut app = test_app();
        push(&mut app, ViewerEvent::CutsceneStarted { event_id: 599 });
        push(&mut app, fade_cue(SCHEDULER_TAG_FADE_OUT));
        push(
            &mut app,
            ViewerEvent::Cutscene {
                cue: CutsceneCue::CameraLock { lock: true },
            },
        );
        step(&mut app, FADE_FRAMES as f32);
        assert_eq!(overlay_alpha(&mut app), 1.0);

        push(&mut app, ViewerEvent::CutsceneEnded);
        step(&mut app, 0.0);
        assert!(
            !app.world().resource::<HudHidden>().cutscene,
            "the HUD comes back without waiting for a CameraLock release"
        );

        step(&mut app, FADE_FRAMES as f32);
        assert_eq!(overlay_alpha(&mut app), 0.0, "no driver, no black screen");
        assert!(!app.world().resource::<CutsceneMode>().active);
    }

    #[test]
    fn zone_change_and_disconnect_force_clear_a_latched_fade() {
        for exit in [
            ViewerEvent::ZoneChanged {
                from: Some(230),
                to: 231,
            },
            ViewerEvent::Disconnected {
                reason: "test".into(),
            },
        ] {
            let mut app = test_app();
            push(&mut app, ViewerEvent::CutsceneStarted { event_id: 599 });
            push(&mut app, fade_cue(SCHEDULER_TAG_FADE_OUT));
            push(
                &mut app,
                ViewerEvent::Cutscene {
                    cue: CutsceneCue::CameraLock { lock: true },
                },
            );
            step(&mut app, FADE_FRAMES as f32);
            assert_eq!(overlay_alpha(&mut app), 1.0);

            push(&mut app, exit);
            step(&mut app, 0.0);
            assert_eq!(overlay_alpha(&mut app), 0.0, "cleared on the same frame");
            assert!(!app.world().resource::<HudHidden>().cutscene);
        }
    }

    #[test]
    fn a_release_with_no_loaded_program_snaps_rather_than_stranding() {
        let programs = synthetic_programs();
        let mut fade = ScreenFade::default();
        fade.start(programs.get(SCHEDULER_TAG_FADE_OUT).unwrap());
        fade.tick(FADE_FRAMES as f32 / ROUTINE_FPS);
        assert_eq!(fade.opacity(), 1.0);

        fade.release(None);
        assert!(fade.is_clear());
    }

    #[test]
    fn a_fade_cue_from_the_vm_reaches_the_screen() {
        // The matcher half of kuluu
        // `wire_translate::tests::a_fade_cue_from_the_vm_arrives_as_a_viewer_event`: that
        // test pins what the producer emits, this one that the renderer still recognises it.
        let mut app = test_app();
        push(&mut app, fade_cue(SCHEDULER_TAG_FADE_OUT));
        step(&mut app, FADE_FRAMES as f32);
        assert_eq!(overlay_alpha(&mut app), 1.0);
    }

    #[test]
    fn a_scheduler_cue_from_another_dat_is_not_a_fade() {
        let mut app = test_app();
        push(
            &mut app,
            ViewerEvent::Cutscene {
                cue: CutsceneCue::Scheduler {
                    dat_id: SCHEDULER_FADE_DAT_ID + 1,
                    actor: kuluu_snapshot::CutsceneActor::LocalPlayer,
                    partner: kuluu_snapshot::CutsceneActor::LocalPlayer,
                    tag: SCHEDULER_TAG_FADE_OUT,
                    duration: ffxi_event::SCHEDULER_DURATION_FROM_DAT,
                },
            },
        );
        step(&mut app, FADE_FRAMES as f32);
        assert_eq!(overlay_alpha(&mut app), 0.0);
    }

    #[test]
    fn the_duration_operand_rescales_the_authored_timing() {
        assert_eq!(
            scheduler_speed_ratio(ffxi_event::SCHEDULER_DURATION_FROM_DAT, 30),
            1.0
        );
        assert_eq!(scheduler_speed_ratio(SCHEDULER_DURATION_LOOP, 30), 1.0);
        assert_eq!(scheduler_speed_ratio(60, 30), 2.0);

        let mut app = test_app();
        app.world_mut()
            .resource_mut::<EventLog>()
            .push(ViewerEvent::Cutscene {
                cue: CutsceneCue::Scheduler {
                    dat_id: SCHEDULER_FADE_DAT_ID,
                    actor: kuluu_snapshot::CutsceneActor::LocalPlayer,
                    partner: kuluu_snapshot::CutsceneActor::LocalPlayer,
                    tag: SCHEDULER_TAG_FADE_OUT,
                    // XIClient's own zone fade doubles the authored 30 frames this way.
                    duration: FADE_FRAMES * 2,
                },
            });
        step(&mut app, FADE_FRAMES as f32);
        let half = overlay_alpha(&mut app);
        assert!(half > 0.0 && half < 1.0, "still mid-fade at 2x: {half}");
        step(&mut app, FADE_FRAMES as f32);
        assert_eq!(overlay_alpha(&mut app), 1.0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_dat_fade_tags_drive_to_black_and_back() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let location = root
            .resolve(SCHEDULER_FADE_DAT_ID)
            .expect("fade scheduler DAT resolves");
        let bytes = std::fs::read(location.path_under(&root)).expect("fade scheduler DAT reads");

        let programs: HashMap<FourCc, FadeProgram> = fade_programs_in(&bytes).into_iter().collect();
        for (tag, dest) in [
            (SCHEDULER_TAG_FADE_OUT, FADE_OUT_DEST),
            (SCHEDULER_TAG_FADE_IN, FADE_IN_DEST),
        ] {
            let program = programs
                .get(&tag)
                .unwrap_or_else(|| panic!("{} is authored", String::from_utf8_lossy(&tag)));
            assert_eq!(program.steps.len(), 1);
            assert_eq!(program.latched(), Some(ScreenColor { rgba: dest }.tint()));
            assert_eq!(
                program.total_secs(),
                FADE_FRAMES as f32 / ROUTINE_FPS,
                "half a second per half at the 60Hz routine clock"
            );
        }

        let mut fade = ScreenFade::default();
        fade.start(programs.get(&SCHEDULER_TAG_FADE_OUT).unwrap());
        fade.tick(FADE_FRAMES as f32 / ROUTINE_FPS);
        assert_eq!(fade.opacity(), 1.0, "fdo0 ends black");
        fade.start(programs.get(&SCHEDULER_TAG_FADE_IN).unwrap());
        fade.tick(FADE_FRAMES as f32 / ROUTINE_FPS);
        assert!(fade.is_clear(), "fdi0 ends clear");
    }
}
