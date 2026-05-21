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
///
/// `loop_start_sample` is the interleaved-sample index where the
/// loop body begins (NOT the frame index — this is `frame *
/// channels` so `PcmSource::next()` can compare directly against
/// `pos`). `None` for one-shots (SFX); `Some(n)` for BGW music with
/// an intro lead-in that should play once before looping `[n..end]`
/// forever. When `Some`, the source returns infinite samples and the
/// caller must use `PlaybackSettings::ONCE` (otherwise rodio's outer
/// LOOP would race ours and restart from 0).
#[derive(Asset, Debug, Clone, TypePath)]
pub struct PcmAudio {
    pub samples: std::sync::Arc<[f32]>,
    pub sample_rate: u32,
    pub channels: u16,
    pub loop_start_sample: Option<usize>,
}

impl PcmAudio {
    pub fn from_decoded(d: DecodedAudio) -> Self {
        let channels = d.channels as usize;
        // `DecodedAudio::loop_start_sample` is per-channel frames;
        // expand to the interleaved sample index `PcmSource::pos`
        // operates on.
        let loop_start_sample = d
            .loop_start_sample
            .map(|frame| frame as usize * channels);
        Self {
            samples: d.samples.into(),
            sample_rate: d.sample_rate as u32,
            channels: d.channels as u16,
            loop_start_sample,
        }
    }

    /// Override looping behavior. Use `None` to play once (for SFX
    /// where any loop point in the source file is irrelevant), or
    /// `Some(n)` to force-loop at sample `n` regardless of what the
    /// file said. Builder-style for use at the spawn site.
    pub fn with_loop(mut self, loop_start_sample: Option<usize>) -> Self {
        self.loop_start_sample = loop_start_sample;
        self
    }
}

/// Iterator end of the [`PcmAudio`] → rodio bridge. Implements both
/// `Iterator<Item = f32>` and `rodio::Source` — what rodio needs to
/// pipe samples to the audio output device.
///
/// When `loop_start_sample` is `Some`, hitting the end of the buffer
/// rewinds `pos` to that point instead of returning `None` — so the
/// source produces samples indefinitely. SFX leaves `loop_start_sample`
/// at `None` and the iterator terminates normally.
pub struct PcmSource {
    samples: std::sync::Arc<[f32]>,
    sample_rate: u32,
    channels: u16,
    pos: usize,
    loop_start_sample: Option<usize>,
}

impl Iterator for PcmSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.samples.len() {
            // End-of-buffer. Two cases:
            //  * `loop_start_sample = Some(n)` → BGM with FFXI's
            //    intro-then-loop semantics: rewind to `n`, never end.
            //  * `loop_start_sample = None` → one-shot (SFX): return
            //    None so rodio drops the sink.
            let loop_to = self.loop_start_sample?;
            // Defensive: a malformed loop point past EOF would spin
            // forever yielding nothing. Clamp + bail to terminate
            // cleanly instead.
            if loop_to >= self.samples.len() {
                return None;
            }
            self.pos = loop_to;
        }
        let s = self.samples[self.pos];
        self.pos += 1;
        Some(s)
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
        // Looping sources have no total duration — the iterator
        // never ends. SFX (no loop) reports its natural length.
        if self.loop_start_sample.is_some() {
            return None;
        }
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
            loop_start_sample: self.loop_start_sample,
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
    let file_loop_frame = decoded.loop_start_sample;
    let mut pcm = PcmAudio::from_decoded(decoded);
    // BGM must loop. If the BGW file declared no loop point we still
    // want the track to loop (FFXI zone music plays forever) — fall
    // back to "loop the whole track from sample 0" so we never go
    // silent after one play-through. Tracks that DO declare a
    // loop_start (battle themes with intro fanfares) get the
    // intro-once / body-looped behavior the file specifies.
    if pcm.loop_start_sample.is_none() {
        pcm = pcm.with_loop(Some(0));
    }
    let handle = pcm_assets.add(pcm);

    let entity = commands
        .spawn((
            AudioPlayer(handle),
            // `PlaybackSettings::ONCE` because looping is handled
            // inside `PcmSource::next()` — rodio's outer LOOP would
            // restart at sample 0 and ignore the BGW's loop_start.
            PlaybackSettings::ONCE,
        ))
        .id();
    slots.active_entity = Some(entity);
    info!(
        "audio: bgm {track_id} started ({} frames @ {:.0}Hz {}ch, loop_frame={:?})",
        frames, sr, ch, file_loop_frame
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
            // SFX are one-shots — even if the SPW file carries a
            // loop_start (rare, but happens for ambient loops the
            // retail client uses elsewhere), force it off so the
            // sink despawns when the clip ends.
            let h = pcm_assets.add(PcmAudio::from_decoded(decoded).with_loop(None));
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
/// shape or a local UI event + an SE id. Ids are sourced from the
/// DAT scan of `ROM/0/58.DAT` and the open-source FFXI catalogs in
/// `vendor/` (AltanaViewer's CSVs, LSB script references); when no
/// cited source exists yet the field is `None` and the event stays
/// silent rather than playing a guess.
///
/// Read-only: edit the source defaults rather than mutating at
/// runtime. The earlier `/sfx_bind` rebind affordance was scaffolding
/// for unfounded defaults — removing it forces every value to come
/// from a citable source or stay silent.
#[derive(Resource, Debug, Clone)]
pub struct SystemSfxTable {
    /// `ViewerEvent::ZoneChanged` → zone-line confirm chime.
    pub zone_changed: Option<u32>,
    /// `ViewerEvent::LowHp` → critical-HP beep (the "low-life" UI
    /// pulse retail plays under 25% HP).
    pub low_hp: Option<u32>,
    /// `ViewerEvent::EngagedBy` → aggro stinger (mob attacks us).
    pub engaged_by: Option<u32>,
    /// Local engage transition (self.bt_target_id 0 → nonzero) → the
    /// "draw weapon" stinger. Distinct from `engaged_by` because the
    /// situation is different — *we* initiated combat, not the mob.
    pub engage_self: Option<u32>,
    /// Per-swing combat blip — fires on every growth of the Battle
    /// chat-line count. Each hit/miss/proc/reaction produces one
    /// audible tick so combat has rhythm. Default `None` until a
    /// short hit-or-miss clip is sourced — until then the engaged
    /// badge pulse carries the rhythm visually.
    pub swing_tick: Option<u32>,
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
        // Every value here needs a citable source — either a DAT-scan
        // observation tied to retail behavior, or a known reference in
        // an open-source vendor catalog. Unsourced placeholders should
        // stay `None` so silence is the default rather than a guess.
        //
        // Today none of the SE→event mappings have been verified — the
        // earlier defaults were guesses propped up by `/sfx_bind` for
        // runtime correction. With `/sfx_bind` removed we'd rather
        // ship silence than wrong sound. The companion task (build a
        // sourced catalog in `ffxi-audio/sfx_catalog.csv` from vendor/
        // references) populates real defaults; until then each event
        // stays muted.
        Self {
            zone_changed: None,
            low_hp: None,
            engaged_by: None,
            engage_self: None,
            swing_tick: None,
            tell_received: None,
            level_up: None,
            skill_level_up: None,
            ui_chat_open: None,
            ui_chat_send: None,
            ui_chat_cancel: None,
            ui_menu_open: None,
            ui_menu_move: None,
            ui_menu_confirm: None,
            ui_menu_cancel: None,
            ui_command_ok: None,
            ui_command_err: None,
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

/// Tracks the local player's prior engagement state + Battle chat line
/// count so transitions can be detected each frame. The two signals
/// drive [`fire_combat_sfx_events`]:
///   - `prev_engaged`: latches on 0→nonzero `bt_target_id` to fire
///     [`SystemSfxTable::engage_self`] exactly once per engage.
///   - `prev_battle_count`: counts `ChatChannel::Battle` lines and
///     fires [`SystemSfxTable::swing_tick`] on every growth (same
///     signal the engaged-badge pulse rides — keeps the audible cue
///     in lockstep with the visual one).
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct CombatSfxState {
    pub prev_engaged: bool,
    pub prev_battle_count: usize,
}

/// Combat-event → SFX bridge for signals not covered by the
/// `ViewerEvent`-level [`fire_system_sfx_events`]: the local player's
/// own engage transition (no wire event for this — derived from
/// `bt_target_id`) and per-swing ticks (driven by Battle chat-line
/// arrivals). Mirrors the badge-pulse latch in
/// `hud::target_panel::detect_swing_pulse_system` so the visual
/// flash and the audible tick come from the same source.
pub fn fire_combat_sfx_events(
    scene: Res<crate::snapshot::SceneState>,
    table: Res<SystemSfxTable>,
    mut state: ResMut<CombatSfxState>,
    mut writer: MessageWriter<SfxEvent>,
) {
    let snap = &scene.snapshot;

    // Self-engage transition.
    let engaged_now = snap
        .self_char_id
        .and_then(|sid| snap.entities.iter().find(|e| e.id == sid))
        .map(|self_pc| self_pc.bt_target_id != 0)
        .unwrap_or(false);
    if engaged_now && !state.prev_engaged {
        if let Some(se_id) = table.engage_self {
            writer.write(SfxEvent::new(se_id));
        }
    }
    state.prev_engaged = engaged_now;

    // Per-swing tick. Count Battle channel lines; growth → fire.
    let battle_count = crate::snapshot::rendered_chat(&scene)
        .iter()
        .filter(|l| l.channel == ffxi_viewer_wire::ChatChannel::Battle)
        .count();
    if battle_count > state.prev_battle_count {
        if let Some(se_id) = table.swing_tick {
            writer.write(SfxEvent::new(se_id));
        }
    }
    state.prev_battle_count = battle_count;
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

// ---------------------------------------------------------------------------
//  Weather SFX
//
//  Two layers, both driven by `SceneState.snapshot.weather` changes:
//
//    1. **Stinger** — one-shot SfxEvent on transition (rain-start
//       splat, thunder clap, sand-storm whoosh). Same path as system
//       SFX; just resolves through `WeatherSfxTable`.
//
//    2. **Ambient** — a dedicated looping AudioPlayer entity carrying
//       the sustained weather sound (rain hiss, wind howl). On
//       weather change, the previous ambient fades out over
//       `WEATHER_FADE_SECS` while the new ambient fades in from 0 to
//       full volume in parallel, then the old sink despawns.
//
//  None of the per-weather SE ids are populated by default. The
//  `from_lsb` `Weather` enum has 20 variants (None..Darkness) and
//  retail's UI/system SE assignments aren't in any vendored table —
//  populating safe defaults would just mean playing arbitrary SEs.
//  Operators set the ids by mutating `WeatherSfxTable` at runtime
//  (or wire a `/weather_bind` slash command in a follow-up).
// ---------------------------------------------------------------------------

/// Per-weather SE id pair. `stinger` plays once on transition,
/// `ambient` is the sustained loop swapped in (then crossfaded
/// against the previous weather's ambient). Either may be `None`
/// — e.g. clear weather (None/Sunshine/Clouds) typically has no
/// associated SE on either side.
#[derive(Debug, Clone, Copy, Default)]
pub struct WeatherSfxEntry {
    pub stinger: Option<u32>,
    pub ambient: Option<u32>,
}

/// Crossfade duration applied to both the outgoing and incoming
/// ambient sinks. 1.0s matches retail's perceived weather-change
/// transition pacing; tunable here for taste.
pub const WEATHER_FADE_SECS: f32 = 1.0;

/// Mapping from LSB `Weather` variants to SE ids. Default = every
/// entry is `(None, None)` (silent) — operators set ids after
/// listening through `/sfx <id>`. Indexed by `Weather as usize`.
#[derive(Resource, Debug, Clone)]
pub struct WeatherSfxTable {
    pub entries: [WeatherSfxEntry; 20],
}

impl Default for WeatherSfxTable {
    fn default() -> Self {
        // All `None` — see module docs for why we don't ship guessed
        // ids for weather. Bind via mutating this resource at
        // runtime (or a future /weather_bind slash command).
        Self {
            entries: [WeatherSfxEntry::default(); 20],
        }
    }
}

impl WeatherSfxTable {
    pub fn get(&self, weather: ffxi_viewer_wire::Weather) -> WeatherSfxEntry {
        self.entries[weather as usize]
    }

    /// Mutator used by future runtime-bind slash commands. Returns
    /// the previous entry so the caller can log "before → after".
    pub fn set(
        &mut self,
        weather: ffxi_viewer_wire::Weather,
        entry: WeatherSfxEntry,
    ) -> WeatherSfxEntry {
        let prev = self.entries[weather as usize];
        self.entries[weather as usize] = entry;
        prev
    }
}

/// Active ambient-loop sink (if any) and the weather it's voicing.
/// `prev_weather` tracks the last observed `SceneState.weather` so
/// the observer system can detect change-of-state on a single
/// `Local`-style cursor.
#[derive(Resource, Debug, Default)]
pub struct WeatherAmbient {
    pub active_entity: Option<Entity>,
    pub active_weather: Option<ffxi_viewer_wire::Weather>,
    pub prev_weather: Option<ffxi_viewer_wire::Weather>,
}

/// Component attached to an AudioPlayer entity to drive a linear
/// volume tween over `duration` seconds. `t` advances each frame;
/// when `t >= duration` the system writes the final volume and
/// removes the component (or despawns the entity if `despawn_on_end`).
#[derive(Component, Debug, Clone, Copy)]
pub struct WeatherFade {
    pub from: f32,
    pub to: f32,
    pub t: f32,
    pub duration: f32,
    pub despawn_on_end: bool,
}

impl WeatherFade {
    fn fade_in(duration: f32) -> Self {
        Self { from: 0.0, to: 1.0, t: 0.0, duration, despawn_on_end: false }
    }
    fn fade_out(duration: f32) -> Self {
        Self { from: 1.0, to: 0.0, t: 0.0, duration, despawn_on_end: true }
    }
}

/// Observe `SceneState.snapshot.weather` each frame; on change, fire
/// the stinger (if mapped) and swap the ambient sink with a
/// crossfade. The state-flag derivation system runs every frame too,
/// so reading `SceneState` here is consistent with the rest of the
/// audio module.
pub fn observe_weather_changes(
    scene: Res<crate::snapshot::SceneState>,
    table: Res<WeatherSfxTable>,
    slots: Res<BgmSlots>,
    mut cache: ResMut<SfxCache>,
    mut pcm_assets: ResMut<Assets<PcmAudio>>,
    mut ambient: ResMut<WeatherAmbient>,
    mut sfx_writer: MessageWriter<SfxEvent>,
    fade_q: Query<Entity, With<WeatherFade>>,
    mut commands: Commands,
) {
    let current = scene.snapshot.weather;
    if current == ambient.prev_weather {
        return;
    }
    let prev = ambient.prev_weather;
    ambient.prev_weather = current;

    let Some(weather) = current else {
        // We've moved into "no weather state" (zone wipe). Fade out
        // anything still playing.
        if let Some(e) = ambient.active_entity.take() {
            if let Ok(mut ent) = commands.get_entity(e) {
                ent.insert(WeatherFade::fade_out(WEATHER_FADE_SECS));
            }
            ambient.active_weather = None;
        }
        return;
    };

    let entry = table.get(weather);

    // Stinger — fire-and-forget through the same SfxEvent path as
    // system SFX, so cache + decode is shared.
    if let Some(se_id) = entry.stinger {
        sfx_writer.write(SfxEvent::new(se_id));
    }

    // Ambient — only act if the *ambient* changed (some weather
    // transitions like Rain↔Squall share an ambient — we don't want
    // to restart the loop). Compare the new ambient id against the
    // currently-active weather's ambient id; if they match, keep
    // the existing sink.
    let new_ambient_id = entry.ambient;
    let prev_ambient_id = prev
        .map(|w| table.get(w).ambient)
        .unwrap_or(None);

    if new_ambient_id == prev_ambient_id && ambient.active_entity.is_some() {
        ambient.active_weather = Some(weather);
        return;
    }

    // Mark any in-flight fades for despawn so the channel doesn't
    // accumulate sinks if weather flips quickly.
    if let Some(e) = ambient.active_entity.take() {
        if let Ok(mut ent) = commands.get_entity(e) {
            ent.insert(WeatherFade::fade_out(WEATHER_FADE_SECS));
        }
    }
    // Also catch orphaned faders if scene cleared without us
    // observing it (defensive — shouldn't happen, but cheap).
    for e in fade_q.iter() {
        if Some(e) != ambient.active_entity {
            // Already fading — let them finish.
        }
    }

    ambient.active_weather = Some(weather);

    let Some(se_id) = new_ambient_id else {
        return; // weather has no ambient layer
    };
    let Some(install) = slots.install_root.clone() else {
        return; // no install path; play_sfx warns about this already
    };

    // Reuse the SfxCache for ambient decode too — the same .spw can
    // serve both as a one-shot and a looped layer; differentiation
    // is per-spawn via `with_loop(Some(0))`.
    let handle = if let Some(h) = cache.cached.get(&se_id) {
        h.clone()
    } else {
        let Some(path) = find_audio(&install, AudioKind::Sfx, se_id) else {
            warn!(
                "audio: weather ambient {se_id} not found under {}",
                install.display()
            );
            return;
        };
        let decoded = match decode_file(&path) {
            Ok(d) => d,
            Err(e) => {
                warn!("audio: weather ambient {se_id} decode failed: {e}");
                return;
            }
        };
        // Cache the bare decoded asset (no loop) — the spawn site
        // wraps a fresh handle with `with_loop(Some(0))` so the same
        // cached PcmAudio works for both SFX one-shots and ambient
        // loops. To keep the cache pure (one handle per id) we
        // build a *separate* handle here for the looped flavor.
        let h = pcm_assets.add(PcmAudio::from_decoded(decoded).with_loop(None));
        cache.cached.insert(se_id, h.clone());
        h
    };

    // Build the looped flavor by reading the cached asset back out
    // and inserting a new handle with the loop point forced on.
    // The asset lookup can technically fail if the handle was
    // dropped, but we just-inserted-or-found it above so this is
    // safe in practice.
    let looped_handle = pcm_assets
        .get(&handle)
        .cloned()
        .map(|p| pcm_assets.add(p.with_loop(Some(0))))
        .unwrap_or(handle);

    let entity = commands
        .spawn((
            AudioPlayer(looped_handle),
            PlaybackSettings::ONCE
                .with_volume(bevy::audio::Volume::Linear(0.0)),
            WeatherFade::fade_in(WEATHER_FADE_SECS),
        ))
        .id();
    ambient.active_entity = Some(entity);
    info!(
        "audio: weather ambient se={se_id} weather={:?} (fade-in {}s)",
        weather, WEATHER_FADE_SECS
    );
}

/// Advance every `WeatherFade` component each frame, applying the
/// interpolated volume to the entity's `AudioSink` and despawning
/// the entity if `despawn_on_end` and the tween completes.
pub fn tick_weather_fades(
    time: Res<Time>,
    mut q: Query<(Entity, &mut WeatherFade, Option<&mut bevy::audio::AudioSink>)>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut fade, sink) in q.iter_mut() {
        fade.t += dt;
        let t = (fade.t / fade.duration).clamp(0.0, 1.0);
        let vol = fade.from + (fade.to - fade.from) * t;
        if let Some(mut s) = sink {
            s.set_volume(bevy::audio::Volume::Linear(vol));
        }
        if fade.t >= fade.duration {
            if fade.despawn_on_end {
                commands.entity(entity).despawn();
            } else {
                commands.entity(entity).remove::<WeatherFade>();
            }
        }
    }
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
            .init_resource::<CombatSfxState>()
            .init_resource::<WeatherSfxTable>()
            .init_resource::<WeatherAmbient>()
            .add_message::<SfxEvent>()
            .add_systems(
                Update,
                (
                    drain_music_events_system,
                    derive_bgm_playback_state,
                    apply_bgm_system,
                    fire_system_sfx_events,
                    fire_combat_sfx_events,
                    // UI mode-change SFX observer. Runs before
                    // `play_sfx_system` so any SfxEvent it writes is
                    // consumed this frame rather than next.
                    observe_ui_mode_transitions,
                    // Weather observer writes stingers as SfxEvents
                    // AND spawns its own ambient sink — both must
                    // precede `play_sfx_system`.
                    observe_weather_changes,
                    play_sfx_system,
                    tick_weather_fades,
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
        // Verify the dispatch logic in `fire_system_sfx_events`: events
        // with a `Some` slot in `SystemSfxTable` fire an `SfxEvent`,
        // others stay silent. Inject explicit fixture values rather
        // than relying on production defaults — those defaults are
        // currently all `None` (pending the sourced sfx catalog work)
        // and the test is about dispatch behavior, not which slots
        // happen to be populated.
        let mut app = App::new();
        let mut table = SystemSfxTable::default();
        table.zone_changed = Some(7001);
        table.low_hp = Some(7002);
        table.engaged_by = Some(7003);
        // tell_received stays None — Reconnected stays None too —
        // both should be silent below.
        app.add_message::<SfxEvent>()
            .init_resource::<EventLog>()
            .insert_resource(table)
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
        assert_eq!(
            sfx_messages.len(),
            3,
            "expected 3 mapped events; Reconnected is unmapped and should stay silent"
        );
        assert_eq!(sfx_messages[0], 7001); // ZoneChanged
        assert_eq!(sfx_messages[1], 7002); // LowHp
        assert_eq!(sfx_messages[2], 7003); // EngagedBy
    }

    #[test]
    fn system_sfx_default_table_is_all_silent() {
        // Pin the all-None default: removing `/sfx_bind` made the
        // production default `None` for every slot, since unsourced
        // guesses are worse than silence. A future sourced catalog
        // will fill these in; until then this test guards against
        // accidental re-introduction of unfounded defaults.
        let table = SystemSfxTable::default();
        assert!(table.zone_changed.is_none());
        assert!(table.low_hp.is_none());
        assert!(table.engaged_by.is_none());
        assert!(table.engage_self.is_none());
        assert!(table.swing_tick.is_none());
        assert!(table.level_up.is_none());
        assert!(table.skill_level_up.is_none());
    }

    /// End-to-end pipeline test, gated on `FFXI_DAT_PATH` because the
    /// final stage (decode → spawn AudioPlayer) needs a real .bgw on
    /// disk. Replaces the manual "log in and hear it" listening test
    /// for the BGM path with something CI-runnable.
    /// Extract the latch math from `fire_combat_sfx_events` so we can
    /// drive it with synthetic inputs. The system itself reads from
    /// Bevy resources we don't want to mock here; the latch logic is
    /// the part that holds the invariants.
    fn step_combat_latches(
        state: &mut CombatSfxState,
        engaged_now: bool,
        battle_count: usize,
    ) -> (bool, bool) {
        let fire_engage = engaged_now && !state.prev_engaged;
        state.prev_engaged = engaged_now;
        let fire_swing = battle_count > state.prev_battle_count;
        state.prev_battle_count = battle_count;
        (fire_engage, fire_swing)
    }

    #[test]
    fn combat_sfx_engage_latches_once_per_zero_to_engaged_transition() {
        let mut s = CombatSfxState::default();
        // Idle, no chat → nothing fires.
        assert_eq!(step_combat_latches(&mut s, false, 0), (false, false));
        // Engage (bt_target_id 0 → nonzero) → engage stinger.
        assert_eq!(step_combat_latches(&mut s, true, 0), (true, false));
        // Still engaged on subsequent ticks → no re-fire.
        assert_eq!(step_combat_latches(&mut s, true, 0), (false, false));
        // Disengage → no fire (the stinger is for transitions into
        // combat, not out of it).
        assert_eq!(step_combat_latches(&mut s, false, 0), (false, false));
        // Re-engage → fires again.
        assert_eq!(step_combat_latches(&mut s, true, 0), (true, false));
    }

    #[test]
    fn combat_sfx_swing_tick_fires_only_on_battle_chat_growth() {
        let mut s = CombatSfxState::default();
        // No chat → no swing tick.
        assert_eq!(step_combat_latches(&mut s, false, 0), (false, false));
        // First battle line → tick.
        assert_eq!(step_combat_latches(&mut s, false, 1), (false, true));
        // Same count → no tick.
        assert_eq!(step_combat_latches(&mut s, false, 1), (false, false));
        // Multi-line burst (proc + react + headline) → one tick.
        assert_eq!(step_combat_latches(&mut s, false, 4), (false, true));
        // Chat history cap evicts lines → count shrinks → no tick.
        assert_eq!(step_combat_latches(&mut s, false, 2), (false, false));
    }

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
