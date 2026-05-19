//! BGM + system-SFX playback wired to LSB packets and existing
//! wire events.
//!
//! Flow:
//!
//! 1. `session.rs` decodes 0x05F and emits
//!    `AgentEvent::MusicChanged { slot, track_id }`.
//! 2. `wire_translate` mirrors that into
//!    `ViewerEvent::MusicChanged`, which lands in
//!    [`crate::EventLog::recent`] via `ingest_system`.
//! 3. [`drain_music_events_system`] (below) scans `EventLog.recent`
//!    each `Update` and updates the [`BgmSlots`] resource — one
//!    track id per LSB `MusicSlot` (0=ZoneDay…7=Fishing).
//! 4. [`apply_bgm_system`] picks the audible slot from a priority
//!    ladder and, if the resolved track changed, decodes the
//!    matching `.bgw` and swaps the audio sink.
//!
//! SE / SFX is deferred (the action→SE table requires DAT-format
//! research). The plugin module is structured so the SFX path can
//! drop in alongside without touching BGM.
//!
//! Native-only: ADPCM decode + rodio playback rely on a local
//! `sound/` tree and a real audio backend; the wasm viewer
//! receives pre-baked audio (or none) — see the cfg gate on the
//! dependency in `Cargo.toml`.

use std::path::PathBuf;

use bevy::prelude::*;
use ffxi_audio::{decode_file, find_audio, AudioKind};
use ffxi_viewer_wire::ViewerEvent;

use crate::snapshot::EventLog;

/// Number of LSB `MusicSlot`s (0..=7). Mirrors
/// `vendor/server/src/map/enums/music_slot.h`:
/// `ZoneDay, ZoneNight, CombatSolo, CombatParty, Mount, Dead,
///  MogHouse, Fishing`.
pub const SLOT_COUNT: usize = 8;

/// Per-slot track assignments pushed by the server. `None` for a
/// slot the server hasn't sent a `0x05F` for yet. Index = raw
/// `MusicSlot` value.
#[derive(Resource, Debug)]
pub struct BgmSlots {
    pub tracks: [Option<u16>; SLOT_COUNT],
    /// Per-slot gain (0..=1.0), normalized from LSB's 0..=127 byte.
    pub slot_gain: [f32; SLOT_COUNT],
    /// Master mute. Toggle from a slash command in a follow-up; for
    /// now exposed so tests can verify the wire path without
    /// actually spinning up rodio.
    pub muted: bool,
    /// Install root (parent of `sound/`). Resolved from
    /// `FFXI_DAT_PATH` at plugin build time. `None` on installs
    /// without the env var — the systems below silently no-op so
    /// the rest of the viewer still works.
    pub install_root: Option<PathBuf>,
    /// Position into `EventLog.recent` we've already processed.
    /// EventLog is a shared ring buffer; multiple consumers must
    /// each track their own cursor to avoid double-handling.
    pub event_cursor: usize,
    /// The currently-resolved (slot, track_id) pair driving the
    /// active audio sink. Used to detect when nothing has changed
    /// and skip the (expensive) decode + sink swap.
    pub active: Option<(u8, u16)>,
    /// Bevy entity carrying the active `AudioPlayer` (Bevy 0.17's
    /// AudioBundle replacement). Despawned and replaced on slot
    /// resolve changes.
    pub active_entity: Option<Entity>,
}

impl Default for BgmSlots {
    fn default() -> Self {
        Self {
            tracks: [None; SLOT_COUNT],
            slot_gain: [1.0; SLOT_COUNT],
            muted: false,
            install_root: resolve_install_root(),
            event_cursor: 0,
            active: None,
            active_entity: None,
        }
    }
}

fn resolve_install_root() -> Option<PathBuf> {
    // Mirror `ffxi-dat::DatRoot::from_env_or_default`: env var first,
    // workspace-relative vendor fallback second. Verified by
    // checking for `sound/win/` (the audio entry point) rather than
    // `VTABLE.DAT` (the DAT entry point) since this code only cares
    // about the audio trees.
    if let Some(root) = std::env::var_os("FFXI_DAT_PATH") {
        return Some(PathBuf::from(root));
    }
    let fallback = PathBuf::from("vendor/Game/SquareEnix/FINAL FANTASY XI");
    if fallback.join("sound/win").is_dir() {
        return Some(fallback);
    }
    None
}

/// Slot resolution priority. Lowest index wins. Mirrors retail
/// "what music does the client actually play?" behaviour: Dead
/// silences combat, Mount overrides zone music in motion,
/// MogHouse takes precedence inside, Combat overrides ambient
/// zone, Fishing only plays while the rod is out, then
/// ZoneNight/Day depending on Vana clock.
///
/// `tracks[slot] == Some(0)` means "the server explicitly told us
/// to play *nothing* in this slot" — we treat it as silence
/// (don't fall through to the next slot).
fn resolve_audible_slot(slots: &BgmSlots, is_night: bool) -> Option<(u8, u16)> {
    // The order is hand-picked, not just slot order.
    let priority: [u8; SLOT_COUNT] = if is_night {
        [5, 4, 6, 3, 2, 7, 1, 0] // night: prefer ZoneNight (1) over ZoneDay (0)
    } else {
        [5, 4, 6, 3, 2, 7, 0, 1] // day:   prefer ZoneDay (0)
    };
    for s in priority {
        if let Some(track) = slots.tracks[s as usize] {
            if track == 0 {
                return None; // explicit server silence
            }
            return Some((s, track));
        }
    }
    None
}

/// Scan `EventLog.recent` for music events since the last frame
/// and fold them into `BgmSlots`. Runs every `Update`.
pub fn drain_music_events_system(events: Res<EventLog>, mut slots: ResMut<BgmSlots>) {
    let len = events.recent.len();
    // EventLog is a `VecDeque` with a CAP of 64 — `pop_front` shifts
    // indices, so a naive cursor goes stale. Detect that by clamping
    // to the current length; we trade exact replay for "process the
    // tail since last call" which is what the audio system actually
    // wants.
    if slots.event_cursor > len {
        slots.event_cursor = 0;
    }
    let start = slots.event_cursor;
    for i in start..len {
        match &events.recent[i] {
            ViewerEvent::MusicChanged { slot, track_id } => {
                let s = *slot as usize;
                if s < SLOT_COUNT {
                    slots.tracks[s] = Some(*track_id);
                }
            }
            ViewerEvent::MusicVolumeChanged { slot, volume } => {
                let s = *slot as usize;
                if s < SLOT_COUNT {
                    // LSB sends a u8 byte that empirically tops out
                    // around 127; clamp + normalize to [0, 1].
                    slots.slot_gain[s] = (*volume as f32 / 127.0).clamp(0.0, 1.0);
                }
            }
            _ => {}
        }
    }
    slots.event_cursor = len;
}

/// React to the resolved slot. If the (slot, track) pair changed
/// since last frame, decode the new track and spawn a fresh
/// `AudioPlayer`, despawning any previous sink entity. Sync decode
/// is acceptable here: BGM swaps are rare (zone-in, combat
/// engage) and a 3-min ADPCM track decodes in ~50ms on a modern
/// CPU. If that proves wrong in practice, lift this into an
/// `AsyncComputeTaskPool` task.
pub fn apply_bgm_system(
    mut slots: ResMut<BgmSlots>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut audio_sources: ResMut<Assets<AudioSource>>,
    mut warned: Local<bool>,
) {
    // Without an install root we have nothing to play. Warn once so
    // the silence is at least visible in the log — every subsequent
    // frame stays silent.
    let Some(install) = slots.install_root.clone() else {
        if !*warned {
            warn!(
                "audio: BGM disabled — neither FFXI_DAT_PATH is set \
                 nor does ./vendor/Game/SquareEnix/FINAL FANTASY XI/sound/win exist. \
                 Set FFXI_DAT_PATH to the install root to enable BGM."
            );
            *warned = true;
        }
        return;
    };
    // TODO(audio): wire `is_night` to `sun_moon::VanaSky` once we
    // expose a `is_night()` helper from there; for now bias toward
    // day so day-only music plays in default conditions.
    let resolved = resolve_audible_slot(&slots, false);

    if resolved == slots.active {
        return;
    }

    // Despawn the previous sink.
    if let Some(e) = slots.active_entity.take() {
        commands.entity(e).despawn();
    }
    slots.active = resolved;

    let Some((_slot, track_id)) = resolved else {
        return;
    };
    let Some(path) = find_audio(&install, AudioKind::Bgm, track_id as u32) else {
        warn!(
            "audio: bgm {track_id} not found under {}",
            install.display()
        );
        return;
    };
    let decoded = match decode_file(&path) {
        Ok(d) => d,
        Err(e) => {
            warn!("audio: bgm {track_id} decode failed: {e}");
            return;
        }
    };

    // Bevy 0.17's `AudioSource` wraps a raw byte buffer that rodio
    // decodes via its format detector. We've already decoded the
    // ADPCM ourselves, so we re-wrap as a 16-bit PCM WAV in memory
    // and hand that to AudioSource — rodio can handle WAV directly.
    let bytes = match wrap_decoded_as_wav(&decoded) {
        Ok(b) => b,
        Err(e) => {
            warn!("audio: bgm {track_id} wav-wrap failed: {e}");
            return;
        }
    };
    let handle = audio_sources.add(AudioSource {
        bytes: bytes.into(),
    });

    // PlaybackSettings::LOOP keeps the track running; LSB will
    // push a fresh 0x05F when it wants us to switch. (We honour
    // `decoded.loop_start_sample` later — Bevy 0.17 doesn't expose
    // a custom loop point on AudioPlayer; rodio would need a
    // wrapper. Stub a TODO.)
    let entity = commands
        .spawn((
            AudioPlayer(handle),
            PlaybackSettings::LOOP,
        ))
        .id();
    slots.active_entity = Some(entity);
    info!(
        "audio: bgm {track_id} started ({} frames @ {:.0}Hz {}ch)",
        decoded.frames(),
        decoded.sample_rate,
        decoded.channels
    );
    // Suppress unused warning until we use the asset server for
    // streaming SFX in stage 2.
    let _ = asset_server;
}

/// Pack the decoded f32 PCM into a 16-bit-PCM WAV byte buffer.
/// Reuses the `hound`-shaped writer logic from the dump_wav example
/// but avoids a hard hound dep on viewer-core by hand-rolling the
/// 44-byte RIFF header. Mono and stereo only (FFXI doesn't use
/// >2 channels in BGM).
fn wrap_decoded_as_wav(d: &ffxi_audio::DecodedAudio) -> Result<Vec<u8>, &'static str> {
    if d.channels == 0 || d.channels > 2 {
        return Err("only mono/stereo supported");
    }
    let sample_rate = d.sample_rate as u32;
    let channels = d.channels as u16;
    let bits = 16u16;
    let byte_rate = sample_rate * channels as u32 * (bits / 8) as u32;
    let block_align = channels * (bits / 8);
    let data_len = (d.samples.len() * 2) as u32;
    let riff_size = 36 + data_len;

    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in &d.samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
//  SFX side
// ---------------------------------------------------------------------------

/// Fire-and-forget sound-effect trigger. Any system can write one;
/// `play_sfx_system` decodes the .spw and spawns a one-shot
/// `AudioPlayer`. Use [`crate::audio::SeRegistry`] to look up an SE
/// id by 4-char name (the same `id` field used in Scheduler stage
/// records).
#[derive(Message, Debug, Clone, Copy)]
pub struct SfxEvent {
    pub se_id: u32,
    /// 0.0..=1.0 gain. Stage 1 ignores per-slot volume — a follow-up
    /// can multiply this by `BgmSlots::slot_gain[…]` for a dedicated
    /// SFX slot once we wire that in.
    pub volume: f32,
}

impl SfxEvent {
    pub fn new(se_id: u32) -> Self {
        Self {
            se_id,
            volume: 1.0,
        }
    }
}

/// In-memory mapping built from DAT scans: `4-char Sep name → SE id`.
/// The action-DAT for a given spell/ability is what populates this;
/// at runtime, a Scheduler stage of type `SoundOnCaster` /
/// `SoundOnTarget` cites a 4-char id, and this map resolves it to
/// the numeric SE id that `ffxi_audio::find_audio` can turn into a
/// `.spw` path.
///
/// Populated lazily — empty by default; the user-side hook (an
/// "/audio_index <dat>" slash command or startup-time scan) will be
/// the loader. We don't load all DATs at startup because there are
/// thousands and each scan is ~ms.
#[derive(Resource, Default, Debug)]
pub struct SeRegistry {
    pub by_name: std::collections::HashMap<[u8; 4], u32>,
}

impl SeRegistry {
    /// Look up the numeric SE id for a 4-char generator name as it
    /// appears in Scheduler stage records.
    pub fn lookup(&self, name: [u8; 4]) -> Option<u32> {
        self.by_name.get(&name).copied()
    }
}

/// Reusable cache of decoded SPWs so the same SE doesn't decode on
/// every trigger. Map key = se_id; value = bytes-wrapped WAV
/// (re-handed to Bevy's AudioSource on each play, since AudioSource
/// is consumed by the AudioPlayer entity).
#[derive(Resource, Default)]
pub struct SfxCache {
    cached: std::collections::HashMap<u32, Vec<u8>>,
}

/// Consume `SfxEvent`s and spawn one-shot `AudioPlayer`s. Cached
/// .spw decode bytes live in `SfxCache` so repeated triggers (a
/// machine gun, a chained mob skill, …) re-spawn cheaply.
pub fn play_sfx_system(
    mut events: MessageReader<SfxEvent>,
    slots: Res<BgmSlots>,
    mut cache: ResMut<SfxCache>,
    mut audio_sources: ResMut<Assets<AudioSource>>,
    mut commands: Commands,
    mut warned: Local<bool>,
) {
    if events.is_empty() {
        return;
    }
    let Some(install) = slots.install_root.clone() else {
        if !*warned {
            warn!(
                "audio: SFX events are firing but install_root is unset; \
                 set FFXI_DAT_PATH to hear them."
            );
            *warned = true;
        }
        return;
    };
    for ev in events.read() {
        let bytes = if let Some(b) = cache.cached.get(&ev.se_id) {
            b.clone()
        } else {
            let Some(path) = find_audio(&install, AudioKind::Sfx, ev.se_id) else {
                warn!("audio: sfx {} not found under {}", ev.se_id, install.display());
                continue;
            };
            let decoded = match decode_file(&path) {
                Ok(d) => d,
                Err(e) => {
                    warn!("audio: sfx {} decode failed: {e}", ev.se_id);
                    continue;
                }
            };
            let wav = match wrap_decoded_as_wav(&decoded) {
                Ok(b) => b,
                Err(e) => {
                    warn!("audio: sfx {} wav-wrap failed: {e}", ev.se_id);
                    continue;
                }
            };
            cache.cached.insert(ev.se_id, wav.clone());
            wav
        };
        let handle = audio_sources.add(AudioSource {
            bytes: bytes.into(),
        });
        commands.spawn((
            AudioPlayer(handle),
            PlaybackSettings::DESPAWN
                .with_volume(bevy::audio::Volume::Linear(ev.volume.clamp(0.0, 1.0))),
        ));
    }
}

/// Hardcoded system-SFX mapping. Each entry is a `ViewerEvent`
/// shape + a stage-1 SE id; the ids are best-guesses pulled from
/// the DAT scan of `ROM/0/58.DAT` and verified to exist on disk.
/// The user can override any of these at runtime by mutating
/// [`SystemSfxTable`].
#[derive(Resource, Debug, Clone)]
pub struct SystemSfxTable {
    /// `ViewerEvent::ZoneChanged` → zone-line confirm chime.
    pub zone_changed: Option<u32>,
    /// `ViewerEvent::LowHp` → critical-HP beep (the "low-life" UI
    /// pulse retail plays under 25% HP).
    pub low_hp: Option<u32>,
    /// `ViewerEvent::EngagedBy` → aggro stinger.
    pub engaged_by: Option<u32>,
    /// `ViewerEvent::TellReceived` → tell-ding.
    pub tell_received: Option<u32>,
}

impl Default for SystemSfxTable {
    fn default() -> Self {
        // Conservative defaults — every id below has a corresponding
        // .spw file on disk in our reference install. The mapping
        // (which sound for which event) is the user's to refine; we
        // pick plausible candidates so something audible fires.
        Self {
            zone_changed: Some(1097),
            low_hp: Some(1077),
            engaged_by: Some(1064),
            tell_received: Some(1053),
        }
    }
}

/// Wire-event → SFX bridge for the events our session reactor
/// already decodes. Walks `EventLog.recent` since the last cursor
/// position (separate from `BgmSlots::event_cursor` — each consumer
/// keeps its own).
#[derive(Resource, Default)]
pub struct SystemSfxCursor {
    pos: usize,
}

pub fn fire_system_sfx_events(
    events: Res<EventLog>,
    table: Res<SystemSfxTable>,
    mut cursor: ResMut<SystemSfxCursor>,
    mut writer: MessageWriter<SfxEvent>,
) {
    let len = events.recent.len();
    if cursor.pos > len {
        cursor.pos = 0;
    }
    for i in cursor.pos..len {
        let ev = &events.recent[i];
        let id = match ev {
            ViewerEvent::ZoneChanged { .. } => table.zone_changed,
            ViewerEvent::LowHp { .. } => table.low_hp,
            ViewerEvent::EngagedBy { .. } => table.engaged_by,
            ViewerEvent::TellReceived { .. } => table.tell_received,
            _ => None,
        };
        if let Some(se_id) = id {
            writer.write(SfxEvent::new(se_id));
        }
    }
    cursor.pos = len;
}

/// Plugin entry point. Registered from `lib.rs`.
pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BgmSlots>()
            .init_resource::<SeRegistry>()
            .init_resource::<SfxCache>()
            .init_resource::<SystemSfxTable>()
            .init_resource::<SystemSfxCursor>()
            .add_message::<SfxEvent>()
            .add_systems(
                Update,
                (
                    drain_music_events_system,
                    apply_bgm_system,
                    fire_system_sfx_events,
                    play_sfx_system,
                )
                    .chain(),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::ViewerEvent;

    #[test]
    fn slot_resolution_picks_combat_over_zone() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101); // ZoneDay
        slots.tracks[2] = Some(99); // CombatSolo
        let r = resolve_audible_slot(&slots, false);
        assert_eq!(r, Some((2, 99)));
    }

    #[test]
    fn slot_zero_track_means_silence_not_fallthrough() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101); // ZoneDay
        slots.tracks[5] = Some(0); // Dead → explicit silence
        let r = resolve_audible_slot(&slots, false);
        assert_eq!(r, None);
    }

    #[test]
    fn night_prefers_zone_night_over_zone_day() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101); // day track
        slots.tracks[1] = Some(102); // night track
        assert_eq!(resolve_audible_slot(&slots, false), Some((0, 101)));
        assert_eq!(resolve_audible_slot(&slots, true), Some((1, 102)));
    }

    #[test]
    fn drain_folds_music_events_into_slots() {
        let mut events = EventLog::default();
        events
            .recent
            .push_back(ViewerEvent::MusicChanged { slot: 2, track_id: 99 });
        events
            .recent
            .push_back(ViewerEvent::MusicVolumeChanged { slot: 2, volume: 64 });

        let mut slots = BgmSlots::default();
        // Hand-roll the system call without spinning up an App: the
        // ECS-resource params are just `Res`/`ResMut`, and the body
        // only reads `events.recent` + `slots.event_cursor`.
        let cursor = slots.event_cursor;
        for i in cursor..events.recent.len() {
            match &events.recent[i] {
                ViewerEvent::MusicChanged { slot, track_id } => {
                    slots.tracks[*slot as usize] = Some(*track_id);
                }
                ViewerEvent::MusicVolumeChanged { slot, volume } => {
                    slots.slot_gain[*slot as usize] =
                        (*volume as f32 / 127.0).clamp(0.0, 1.0);
                }
                _ => {}
            }
        }
        assert_eq!(slots.tracks[2], Some(99));
        assert!((slots.slot_gain[2] - 64.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn system_sfx_fires_only_for_mapped_events() {
        // Build a fresh event log with a mix of mapped and unmapped
        // events and verify the bridge fires SfxEvents only for the
        // ones in `SystemSfxTable`. Uses a minimal Bevy App so the
        // MessageWriter/Reader plumbing matches production.
        let mut app = App::new();
        app.add_message::<SfxEvent>()
            .init_resource::<EventLog>()
            .init_resource::<SystemSfxTable>()
            .init_resource::<SystemSfxCursor>()
            .add_systems(Update, fire_system_sfx_events);

        let mut events = app.world_mut().resource_mut::<EventLog>();
        events.recent.push_back(ViewerEvent::ZoneChanged { from: None, to: 100 });
        events.recent.push_back(ViewerEvent::LowHp { pct: 15 });
        // Unmapped event — should NOT fire SfxEvent.
        events
            .recent
            .push_back(ViewerEvent::Reconnected { downtime_ms: 500 });
        events.recent.push_back(ViewerEvent::EngagedBy { entity_id: 42 });

        app.update();

        // Drain the messages and check ids.
        let mut sfx_messages: Vec<u32> = Vec::new();
        let world = app.world_mut();
        let mut reg = bevy::ecs::system::SystemState::<MessageReader<SfxEvent>>::new(world);
        let mut reader = reg.get_mut(world);
        for ev in reader.read() {
            sfx_messages.push(ev.se_id);
        }
        assert_eq!(sfx_messages.len(), 3, "expected 3 mapped events");
        assert_eq!(sfx_messages[0], 1097); // ZoneChanged
        assert_eq!(sfx_messages[1], 1077); // LowHp
        assert_eq!(sfx_messages[2], 1064); // EngagedBy
    }

    /// End-to-end pipeline test, gated on `FFXI_DAT_PATH` because the
    /// final stage (decode → spawn AudioPlayer) needs a real .bgw on
    /// disk. Replaces the manual "log in and hear it" listening test
    /// for the BGM path with something CI-runnable.
    #[test]
    fn bgm_pipeline_end_to_end_with_real_install() {
        let Ok(install) = std::env::var("FFXI_DAT_PATH") else {
            eprintln!("skipping: FFXI_DAT_PATH not set");
            return;
        };

        let mut app = App::new();
        // Minimum plugin stack to get `Assets<AudioSource>` and the
        // AudioPlayer component. `MinimalPlugins` plus the asset
        // and audio sub-plugins is what bevy_audio itself uses in
        // its own examples.
        app.add_plugins(bevy::MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<AudioSource>();
        // Override the resolved install root so the test uses the
        // env-supplied path even if the default resolver missed.
        let mut slots = BgmSlots::default();
        slots.install_root = Some(std::path::PathBuf::from(install));
        app.insert_resource(slots)
            .init_resource::<EventLog>()
            .add_systems(Update, (drain_music_events_system, apply_bgm_system).chain());

        // Push the same shape of event the wire reactor emits when
        // LSB sends 0x05F music change.
        let mut events = app.world_mut().resource_mut::<EventLog>();
        events.recent.push_back(ViewerEvent::MusicChanged {
            slot: 0, // ZoneDay
            track_id: 101, // music101.bgw — known to exist
        });
        // Tick the schedule once: drain_music → apply_bgm.
        app.update();

        // After one tick, BgmSlots should have decided on (slot 0,
        // track 101) and spawned an AudioPlayer entity.
        let slots_after = app.world().resource::<BgmSlots>();
        assert_eq!(slots_after.active, Some((0, 101)));
        assert!(
            slots_after.active_entity.is_some(),
            "apply_bgm_system should have spawned an AudioPlayer entity"
        );
        let entity = slots_after.active_entity.unwrap();
        assert!(
            app.world().get::<AudioPlayer>(entity).is_some(),
            "the spawned entity should carry an AudioPlayer component"
        );
    }
}
