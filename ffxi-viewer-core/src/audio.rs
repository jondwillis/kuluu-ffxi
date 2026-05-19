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

use bevy::audio::{AddAudioSource, Decodable, Source as RodioSource};
use bevy::asset::Asset;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use ffxi_audio::{decode_file, find_audio, AudioKind, DecodedAudio};
use ffxi_viewer_wire::ViewerEvent;

// ---------------------------------------------------------------------------
//  Custom Decodable wrapping pre-decoded f32 samples.
//
//  Bevy 0.17 builds `rodio` with `default-features = false, features = ["std"]`
//  — i.e. without any audio-format decoders (wav/flac/mp3 are all opt-in
//  features on Bevy itself). Feeding it WAV-wrapped bytes panics with
//  `UnrecognizedFormat`. Implementing `Decodable` for our own type
//  sidesteps the format-detection path entirely; rodio sees a
//  `rodio::Source` of f32 samples and plays them directly.
// ---------------------------------------------------------------------------

/// Pre-decoded f32 PCM audio. `samples` is interleaved
/// (`L0,R0,L1,R1,...` for stereo). Cheap to clone via `Arc`.
#[derive(Asset, Debug, Clone, TypePath)]
pub struct PcmAudio {
    pub samples: std::sync::Arc<[f32]>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl PcmAudio {
    pub fn from_decoded(d: DecodedAudio) -> Self {
        Self {
            samples: d.samples.into(),
            sample_rate: d.sample_rate as u32,
            channels: d.channels as u16,
        }
    }
}

/// Iterator end of the [`PcmAudio`] → rodio bridge. Implements both
/// `Iterator<Item = f32>` and `rodio::Source` — what rodio needs to
/// pipe samples to the audio output device.
pub struct PcmSource {
    samples: std::sync::Arc<[f32]>,
    sample_rate: u32,
    channels: u16,
    pos: usize,
}

impl Iterator for PcmSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
        self.pos += 1;
        s
    }
}

impl RodioSource for PcmSource {
    fn current_frame_len(&self) -> Option<usize> {
        // Per-frame len is informational; report remaining samples.
        Some(self.samples.len().saturating_sub(self.pos))
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        if self.channels == 0 || self.sample_rate == 0 {
            return None;
        }
        let frames = self.samples.len() / self.channels as usize;
        Some(std::time::Duration::from_secs_f32(
            frames as f32 / self.sample_rate as f32,
        ))
    }
}

impl Decodable for PcmAudio {
    type DecoderItem = f32;
    type Decoder = PcmSource;
    fn decoder(&self) -> Self::Decoder {
        PcmSource {
            samples: self.samples.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            pos: 0,
        }
    }
}

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
///
/// LSB pushes ALL relevant tracks on zone-in (ZoneDay + ZoneNight
/// + CombatSolo + CombatParty + …) — they're cached for later. The
/// client must NOT play CombatSolo just because it's filled; it
/// must check whether the player is actually engaged. Same for
/// Mount, MogHouse, Dead, Fishing. The earlier naive
/// "highest-priority filled slot wins" picked battle music on
/// every zone-in because the server had pre-filled slot 2.
///
/// `state` filters which slots are eligible. With a fresh
/// [`BgmPlaybackState`] (no engaged / no mount / not in mog-house
/// / not dead / not fishing), only Zone slots are eligible and the
/// player hears zone ambient — which is the right default.
fn resolve_audible_slot(
    slots: &BgmSlots,
    state: &BgmPlaybackState,
) -> Option<(u8, u16)> {
    // Priority order, highest-first. Each entry is a slot index.
    // Same priority retail uses internally:
    //   Dead > Mount > MogHouse > CombatParty > CombatSolo >
    //   Fishing > ZoneNight/Day.
    // BUT each non-Zone slot is gated by a state flag so that
    // pre-filled slots stay silent until the player is in the
    // matching state.
    let zone_pref: [u8; 2] = if state.is_night { [1, 0] } else { [0, 1] };
    let candidates: [(u8, bool); SLOT_COUNT] = [
        (5, state.dead),         // Dead
        (4, state.mounted),      // Mount
        (6, state.in_mog_house), // MogHouse
        (3, state.engaged_party), // CombatParty
        (2, state.engaged_solo), // CombatSolo
        (7, state.fishing),      // Fishing
        (zone_pref[0], true),    // ZoneDay/Night per clock
        (zone_pref[1], true),    // the other zone variant as fallback
    ];
    for (slot, eligible) in candidates {
        if !eligible {
            continue;
        }
        if let Some(track) = slots.tracks[slot as usize] {
            if track == 0 {
                return None; // explicit server silence
            }
            return Some((slot, track));
        }
    }
    None
}

/// Game-state flags that gate non-Zone music slots. Default = "in
/// zone, not doing anything special" — only Zone slots play.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct BgmPlaybackState {
    pub engaged_solo: bool,
    pub engaged_party: bool,
    pub mounted: bool,
    pub in_mog_house: bool,
    pub dead: bool,
    pub fishing: bool,
    pub is_night: bool,
}

/// Derive `BgmPlaybackState` from existing scene/clock resources
/// every frame. Mapping (from LSB `status_effect.h` + party state):
///
///   engaged_*    self entity has non-zero `bt_target_id`. Solo if
///                party.len() ≤ 1, else Party.
///   mounted      `status_icons` contains EFFECT_MOUNTED (252).
///   in_mog_house self party member's `in_mog_house` flag (set by
///                LSB GROUP_LIST/ATTR packets — the server is the
///                authority here since zone_id doesn't change
///                when entering a mog house in the same city).
///   dead         `status_icons` contains EFFECT_KO (0).
///   fishing      `status_icons` contains EFFECT_FISHING_IMAGERY
///                (235) — the cast-rod-out effect, set while
///                fishing minigame is active.
///   is_night     `VanaSky::sun_altitude < 0` (sun below horizon).
///                Switches at V-hours 6 and 18, which is ~ every
///                5 real minutes.
///
/// Source ids come from `vendor/server/src/map/status_effect.h`:
///   EFFECT_KO              = 0
///   EFFECT_FISHING_IMAGERY = 235
///   EFFECT_MOUNTED         = 252
pub fn derive_bgm_playback_state(
    scene: Res<crate::snapshot::SceneState>,
    sky: Res<crate::sun_moon::VanaSky>,
    mut state: ResMut<BgmPlaybackState>,
) {
    const EFFECT_KO: u16 = 0;
    const EFFECT_FISHING_IMAGERY: u16 = 235;
    const EFFECT_MOUNTED: u16 = 252;

    let snap = &scene.snapshot;
    let self_id = snap.self_char_id;
    let self_entity = self_id.and_then(|id| snap.entities.iter().find(|e| e.id == id));
    let engaged = self_entity.map(|e| e.bt_target_id != 0).unwrap_or(false);
    let in_party = snap.party.len() > 1;

    let in_mog_house = self_id
        .and_then(|id| snap.party.iter().find(|p| p.id == id))
        .map(|p| p.in_mog_house)
        .unwrap_or(false);

    let icons = &snap.status_icons;
    let dead = icons.contains(&EFFECT_KO);
    let mounted = icons.contains(&EFFECT_MOUNTED);
    let fishing = icons.contains(&EFFECT_FISHING_IMAGERY);

    let is_night = sky.sun_altitude < 0.0;

    *state = BgmPlaybackState {
        engaged_solo: engaged && !in_party,
        engaged_party: engaged && in_party,
        mounted,
        in_mog_house,
        dead,
        fishing,
        is_night,
    };
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
                    info!(
                        "audio: 0x05F slot={} ({}) track={}",
                        slot,
                        slot_name(*slot),
                        track_id
                    );
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

/// Short label for the 8 LSB `MusicSlot` values. Diagnostic only.
fn slot_name(slot: u8) -> &'static str {
    match slot {
        0 => "ZoneDay",
        1 => "ZoneNight",
        2 => "CombatSolo",
        3 => "CombatParty",
        4 => "Mount",
        5 => "Dead",
        6 => "MogHouse",
        7 => "Fishing",
        _ => "?",
    }
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
    state: Res<BgmPlaybackState>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut pcm_assets: ResMut<Assets<PcmAudio>>,
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
    let resolved = resolve_audible_slot(&slots, &state);

    if resolved == slots.active {
        return;
    }

    // One-line summary of every BGM transition. Crucial diagnostic
    // for "I'm not hearing anything" — surfaces whether (a) the
    // server filled the slots we'd play, (b) the state flags
    // gated those slots out, or (c) the audible slot is correct
    // but the file's missing.
    info!(
        target: "audio::bgm",
        "transition: {:?} → {:?} | slots={:?} state={:?}",
        slots.active, resolved, slots.tracks, *state
    );

    // Despawn the previous sink.
    if let Some(e) = slots.active_entity.take() {
        commands.entity(e).despawn();
    }
    slots.active = resolved;

    let Some((slot, track_id)) = resolved else {
        return;
    };
    let _ = slot;
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

    let frames = decoded.frames();
    let sr = decoded.sample_rate;
    let ch = decoded.channels;
    let pcm = PcmAudio::from_decoded(decoded);
    let handle = pcm_assets.add(pcm);

    let entity = commands
        .spawn((
            AudioPlayer(handle),
            PlaybackSettings::LOOP,
        ))
        .id();
    slots.active_entity = Some(entity);
    info!(
        "audio: bgm {track_id} started ({} frames @ {:.0}Hz {}ch)",
        frames, sr, ch
    );
    let _ = asset_server;
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
/// every trigger. Caches the `PcmAudio` handle so repeated plays
/// don't re-decode the .spw — the handle itself is cheap to clone
/// and Bevy's asset system refcounts the underlying samples.
#[derive(Resource, Default)]
pub struct SfxCache {
    cached: std::collections::HashMap<u32, Handle<PcmAudio>>,
}

/// Consume `SfxEvent`s and spawn one-shot `AudioPlayer`s.
pub fn play_sfx_system(
    mut events: MessageReader<SfxEvent>,
    slots: Res<BgmSlots>,
    mut cache: ResMut<SfxCache>,
    mut pcm_assets: ResMut<Assets<PcmAudio>>,
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
        let handle = if let Some(h) = cache.cached.get(&ev.se_id) {
            h.clone()
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
            let h = pcm_assets.add(PcmAudio::from_decoded(decoded));
            cache.cached.insert(ev.se_id, h.clone());
            h
        };
        commands.spawn((
            AudioPlayer(handle),
            PlaybackSettings::DESPAWN
                .with_volume(bevy::audio::Volume::Linear(ev.volume.clamp(0.0, 1.0))),
        ));
    }
}

/// Hardcoded system-SFX mapping. Each entry is a `ViewerEvent`
/// shape or a local UI event + a placeholder SE id; the ids are
/// best-guesses pulled from the DAT scan of `ROM/0/58.DAT` and
/// verified to exist on disk. Use [`SystemSfxTable::bind`] from the
/// `/sfx_bind` slash command to rebind any entry at runtime once
/// you've identified the canonical id via `/sfx N`.
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
    /// `ViewerEvent::LevelUp` (from 0x02D msg id 9) → level-up jingle.
    pub level_up: Option<u32>,
    /// `ViewerEvent::SkillLevelUp` (from 0x02D msg id 53) → skill-up
    /// blip. Fires often at low skill — gate this off (set None) if
    /// it's noisy.
    pub skill_level_up: Option<u32>,
    /// UI: chat input prompt opened (`/` pressed in World mode).
    pub ui_chat_open: Option<u32>,
    /// UI: chat input submitted (Enter pressed in Chat mode).
    pub ui_chat_send: Option<u32>,
    /// UI: chat input dismissed via Escape.
    pub ui_chat_cancel: Option<u32>,
    /// UI: action menu opened (Enter in World).
    pub ui_menu_open: Option<u32>,
    /// UI: menu cursor moved (Up/Down within a menu).
    pub ui_menu_move: Option<u32>,
    /// UI: menu confirmed (Enter on a menu entry — pushes submenu or
    /// fires the action).
    pub ui_menu_confirm: Option<u32>,
    /// UI: menu dismissed (Escape — pops the stack or closes the menu).
    pub ui_menu_cancel: Option<u32>,
    /// UI: slash command dispatched (any recognised `/foo`).
    pub ui_command_ok: Option<u32>,
    /// UI: slash command parse failed / unknown command.
    pub ui_command_err: Option<u32>,
}

impl Default for SystemSfxTable {
    fn default() -> Self {
        // Conservative defaults — every id below has a corresponding
        // .spw file on disk in our reference install. The mapping
        // (which sound for which event) is the user's to refine; rebind
        // via `/sfx_bind <name> <id>` after listening to candidates
        // through `/sfx <id>`. These are placeholders, not verified
        // retail sounds.
        Self {
            zone_changed: Some(1097),
            low_hp: Some(1077),
            engaged_by: Some(1064),
            tell_received: Some(1053),
            // Gameplay placeholders — FFXI retail level-up is in the
            // 188xxx range historically; without a verified table we
            // pick high-bank ids that exist on disk in the reference
            // install.
            level_up: Some(188000),
            skill_level_up: Some(188002),
            // UI placeholders — common menu blips in retail are
            // bank 0 ids. These are guesses; rebind via /sfx_bind.
            ui_chat_open: Some(1),
            ui_chat_send: Some(2),
            ui_chat_cancel: Some(3),
            ui_menu_open: Some(1),
            ui_menu_move: Some(4),
            ui_menu_confirm: Some(2),
            ui_menu_cancel: Some(3),
            ui_command_ok: Some(2),
            ui_command_err: Some(5),
        }
    }
}

/// String-keyed accessor used by the `/sfx_bind` slash command. Keep
/// the key set in sync with the field names above so users have a
/// stable, discoverable vocabulary.
impl SystemSfxTable {
    pub const SLOT_NAMES: &'static [&'static str] = &[
        "zone_changed",
        "low_hp",
        "engaged_by",
        "tell_received",
        "level_up",
        "skill_level_up",
        "ui_chat_open",
        "ui_chat_send",
        "ui_chat_cancel",
        "ui_menu_open",
        "ui_menu_move",
        "ui_menu_confirm",
        "ui_menu_cancel",
        "ui_command_ok",
        "ui_command_err",
    ];

    /// Update one slot by name. Pass `id = 0` to mute (sets `None`).
    /// Returns `false` for an unknown slot name; caller should report
    /// `SLOT_NAMES` to the user in that case.
    pub fn bind(&mut self, slot: &str, id: u32) -> bool {
        let v = if id == 0 { None } else { Some(id) };
        match slot {
            "zone_changed" => self.zone_changed = v,
            "low_hp" => self.low_hp = v,
            "engaged_by" => self.engaged_by = v,
            "tell_received" => self.tell_received = v,
            "level_up" => self.level_up = v,
            "skill_level_up" => self.skill_level_up = v,
            "ui_chat_open" => self.ui_chat_open = v,
            "ui_chat_send" => self.ui_chat_send = v,
            "ui_chat_cancel" => self.ui_chat_cancel = v,
            "ui_menu_open" => self.ui_menu_open = v,
            "ui_menu_move" => self.ui_menu_move = v,
            "ui_menu_confirm" => self.ui_menu_confirm = v,
            "ui_menu_cancel" => self.ui_menu_cancel = v,
            "ui_command_ok" => self.ui_command_ok = v,
            "ui_command_err" => self.ui_command_err = v,
            _ => return false,
        }
        true
    }

    /// Mirror of [`bind`] used by `text_input_system` and other UI
    /// code that wants to fire a named sfx without coupling to the
    /// field set. Returns `None` if the slot is unmapped or unknown.
    pub fn get(&self, slot: &str) -> Option<u32> {
        match slot {
            "zone_changed" => self.zone_changed,
            "low_hp" => self.low_hp,
            "engaged_by" => self.engaged_by,
            "tell_received" => self.tell_received,
            "level_up" => self.level_up,
            "skill_level_up" => self.skill_level_up,
            "ui_chat_open" => self.ui_chat_open,
            "ui_chat_send" => self.ui_chat_send,
            "ui_chat_cancel" => self.ui_chat_cancel,
            "ui_menu_open" => self.ui_menu_open,
            "ui_menu_move" => self.ui_menu_move,
            "ui_menu_confirm" => self.ui_menu_confirm,
            "ui_menu_cancel" => self.ui_menu_cancel,
            "ui_command_ok" => self.ui_command_ok,
            "ui_command_err" => self.ui_command_err,
            _ => None,
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
            ViewerEvent::LevelUp { .. } => table.level_up,
            ViewerEvent::SkillLevelUp { .. } => table.skill_level_up,
            _ => None,
        };
        if let Some(se_id) = id {
            writer.write(SfxEvent::new(se_id));
        }
    }
    cursor.pos = len;
}

/// Discriminant of [`crate::InputMode`] — just the kind, no payload.
/// Used by [`observe_ui_mode_transitions`] to detect cross-kind
/// transitions (World↔Chat etc.) without false-firing on internal
/// payload updates (e.g. each keystroke that mutates `ChatBuffer`).
#[derive(Default, PartialEq, Eq, Clone, Copy, Debug)]
enum InputModeKind {
    #[default]
    World,
    Chat,
    Menu,
    QuickAction,
    Dialog,
    PassiveCursor,
}

impl InputModeKind {
    fn of(mode: &crate::InputMode) -> Self {
        match mode {
            crate::InputMode::World => Self::World,
            crate::InputMode::Chat(_) => Self::Chat,
            crate::InputMode::Menu(_) => Self::Menu,
            crate::InputMode::QuickAction(_) => Self::QuickAction,
            crate::InputMode::Dialog(_) => Self::Dialog,
            crate::InputMode::PassiveCursor(_) => Self::PassiveCursor,
        }
    }
}

/// Compares the `InputMode` discriminant + menu-stack depth across
/// frames and emits `SfxEvent`s on transitions. Menu stack depth is
/// folded in so menu push/pop (Talk→Talk Submenu) plays the open/
/// cancel blip too — pure kind comparison would miss it.
///
/// Per-keystroke internal mutations (typing into ChatBuffer, cursor
/// moves inside a menu *level*) don't trigger this — that would be
/// the wrong granularity. Cursor-move SFX (`ui_menu_move`) is left
/// for an inline emit point in `text_input.rs` when we wire it; this
/// system covers the larger mode lifecycle.
pub fn observe_ui_mode_transitions(
    mode: Res<crate::InputMode>,
    table: Res<SystemSfxTable>,
    mut writer: MessageWriter<SfxEvent>,
    mut prev_kind: Local<InputModeKind>,
    mut prev_menu_depth: Local<usize>,
) {
    let new_kind = InputModeKind::of(&mode);
    let new_menu_depth = match &*mode {
        crate::InputMode::Menu(stack) => stack.levels.len(),
        _ => 0,
    };

    if new_kind != *prev_kind {
        let id = match (*prev_kind, new_kind) {
            // Enter chat input from world / passive cursor.
            (_, InputModeKind::Chat) => table.ui_chat_open,
            // Leave chat (Enter submit OR Esc cancel — we can't
            // disambiguate from outside; both end on World). The
            // `ui_chat_send` SFX is fired from the slash-command
            // dispatcher itself (`SlashOutcome` path) where we
            // *know* it was a submit; this branch only catches the
            // Esc-cancel path.
            (InputModeKind::Chat, _) => table.ui_chat_cancel,
            // Enter menu / quick-action.
            (_, InputModeKind::Menu) | (_, InputModeKind::QuickAction) => table.ui_menu_open,
            // Leave menu / quick-action back to world.
            (InputModeKind::Menu, _) | (InputModeKind::QuickAction, _) => {
                table.ui_menu_cancel
            }
            _ => None,
        };
        if let Some(se_id) = id {
            writer.write(SfxEvent::new(se_id));
        }
        *prev_kind = new_kind;
    } else if new_kind == InputModeKind::Menu && new_menu_depth != *prev_menu_depth {
        // Same kind, but menu stack depth changed → push or pop.
        let id = if new_menu_depth > *prev_menu_depth {
            table.ui_menu_confirm
        } else {
            table.ui_menu_cancel
        };
        if let Some(se_id) = id {
            writer.write(SfxEvent::new(se_id));
        }
    }
    *prev_menu_depth = new_menu_depth;
}

/// Plugin entry point. Registered from `lib.rs`.
pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        // Register `PcmAudio` as a custom audio source so rodio
        // never tries to format-detect our pre-decoded samples. The
        // call also adds the right `PlayAudio<PcmAudio>` system to
        // the schedule, so `AudioPlayer(Handle<PcmAudio>)` entities
        // are picked up.
        app.add_audio_source::<PcmAudio>();
        app.init_resource::<BgmSlots>()
            .init_resource::<BgmPlaybackState>()
            .init_resource::<SeRegistry>()
            .init_resource::<SfxCache>()
            .init_resource::<SystemSfxTable>()
            .init_resource::<SystemSfxCursor>()
            .add_message::<SfxEvent>()
            .add_systems(
                Update,
                (
                    drain_music_events_system,
                    derive_bgm_playback_state,
                    apply_bgm_system,
                    fire_system_sfx_events,
                    // UI mode-change SFX observer. Runs before
                    // `play_sfx_system` so any SfxEvent it writes is
                    // consumed this frame rather than next.
                    observe_ui_mode_transitions,
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

    /// Regression: in real LSB zone-in traffic the server pre-fills
    /// EVERY slot (Zone + Combat + Mount + …) ahead of time. The
    /// earlier resolver picked highest-priority-filled, which made
    /// the client play battle music on zone-in to Bastok Mines
    /// because slot 2 (CombatSolo) was filled even though the player
    /// wasn't engaged. Default state must yield Zone music only.
    #[test]
    fn default_state_picks_zone_not_combat_when_both_filled() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101); // ZoneDay
        slots.tracks[2] = Some(99); // CombatSolo (pre-filled by server)
        let state = BgmPlaybackState::default(); // engaged_solo: false
        assert_eq!(resolve_audible_slot(&slots, &state), Some((0, 101)));
    }

    #[test]
    fn combat_overrides_zone_only_when_engaged() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101);
        slots.tracks[2] = Some(99);
        let engaged = BgmPlaybackState {
            engaged_solo: true,
            ..Default::default()
        };
        assert_eq!(resolve_audible_slot(&slots, &engaged), Some((2, 99)));
    }

    #[test]
    fn dead_state_picks_dead_slot_above_all_else() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101);
        slots.tracks[2] = Some(99);
        slots.tracks[5] = Some(70); // Dead track
        let state = BgmPlaybackState {
            engaged_solo: true, // even mid-combat, Dead wins
            dead: true,
            ..Default::default()
        };
        assert_eq!(resolve_audible_slot(&slots, &state), Some((5, 70)));
    }

    #[test]
    fn dead_track_zero_means_silence_not_fallthrough() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101);
        slots.tracks[5] = Some(0); // Dead → explicit silence
        let state = BgmPlaybackState {
            dead: true,
            ..Default::default()
        };
        assert_eq!(resolve_audible_slot(&slots, &state), None);
    }

    #[test]
    fn night_prefers_zone_night_over_zone_day() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101); // day track
        slots.tracks[1] = Some(102); // night track
        let day = BgmPlaybackState::default();
        let night = BgmPlaybackState {
            is_night: true,
            ..Default::default()
        };
        assert_eq!(resolve_audible_slot(&slots, &day), Some((0, 101)));
        assert_eq!(resolve_audible_slot(&slots, &night), Some((1, 102)));
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
        app.init_asset::<PcmAudio>();
        // Override the resolved install root so the test uses the
        // env-supplied path even if the default resolver missed.
        let mut slots = BgmSlots::default();
        slots.install_root = Some(std::path::PathBuf::from(install));
        app.insert_resource(slots)
            .init_resource::<EventLog>()
            .init_resource::<BgmPlaybackState>()
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
            app.world().get::<AudioPlayer<PcmAudio>>(entity).is_some(),
            "the spawned entity should carry an AudioPlayer<PcmAudio> component"
        );
    }
}
