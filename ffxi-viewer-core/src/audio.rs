use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bevy::asset::Asset;
use bevy::audio::{AddAudioSource, Decodable, Source as RodioSource};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use ffxi_audio::{decode_file, find_audio, AudioKind, DecodedAudio};
use ffxi_viewer_wire::ViewerEvent;

#[derive(Asset, Debug, Clone, TypePath)]
pub struct PcmAudio {
    pub samples: std::sync::Arc<[f32]>,
    pub sample_rate: u32,
    pub channels: u16,
    pub loop_start_sample: Option<usize>,

    pub loop_count: Option<Arc<AtomicU64>>,
}

impl PcmAudio {
    pub fn from_decoded(d: DecodedAudio) -> Self {
        let channels = d.channels as usize;

        let loop_start_sample = d.loop_start_sample.map(|frame| frame as usize * channels);
        Self {
            samples: d.samples.into(),
            sample_rate: d.sample_rate as u32,
            channels: d.channels as u16,
            loop_start_sample,
            loop_count: None,
        }
    }

    pub fn with_loop(mut self, loop_start_sample: Option<usize>) -> Self {
        self.loop_start_sample = loop_start_sample;
        self
    }

    pub fn with_loop_counter(mut self, counter: Arc<AtomicU64>) -> Self {
        self.loop_count = Some(counter);
        self
    }
}

pub struct PcmSource {
    samples: std::sync::Arc<[f32]>,
    sample_rate: u32,
    channels: u16,
    pos: usize,
    loop_start_sample: Option<usize>,
    loop_count: Option<Arc<AtomicU64>>,
}

impl Iterator for PcmSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.samples.len() {
            let loop_to = self.loop_start_sample?;

            if loop_to >= self.samples.len() {
                return None;
            }
            self.pos = loop_to;
            if let Some(c) = &self.loop_count {
                c.fetch_add(1, Ordering::Relaxed);
            }
        }
        let s = self.samples[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl RodioSource for PcmSource {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.samples.len().saturating_sub(self.pos))
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
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
            loop_count: self.loop_count.clone(),
        }
    }
}

use crate::components::InGameEntity;
use crate::snapshot::EventLog;

pub const SLOT_COUNT: usize = 8;

#[derive(Resource, Debug)]
pub struct BgmSlots {
    pub tracks: [Option<u16>; SLOT_COUNT],

    pub slot_gain: [f32; SLOT_COUNT],

    pub install_root: Option<PathBuf>,

    pub event_cursor: usize,

    pub active: Option<(u8, u16)>,

    pub active_entity: Option<Entity>,

    pub bgm_loop_counter: Option<Arc<AtomicU64>>,

    pub bgm_loops_reported: u64,
}

impl Default for BgmSlots {
    fn default() -> Self {
        Self {
            tracks: [None; SLOT_COUNT],
            slot_gain: [1.0; SLOT_COUNT],
            install_root: resolve_install_root(),
            event_cursor: 0,
            active: None,
            active_entity: None,
            bgm_loop_counter: None,
            bgm_loops_reported: 0,
        }
    }
}

fn resolve_install_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("FFXI_DAT_PATH") {
        return Some(PathBuf::from(root));
    }
    let fallback = PathBuf::from(ffxi_dat::archive::DEFAULT_INSTALL_DIR);
    if fallback.join("sound/win").is_dir() {
        return Some(fallback);
    }
    None
}

fn resolve_audible_slot(slots: &BgmSlots, state: &BgmPlaybackState) -> Option<(u8, u16)> {
    let zone_pref: [u8; 2] = if state.is_night { [1, 0] } else { [0, 1] };
    let candidates: [(u8, bool); SLOT_COUNT] = [
        (5, state.dead),
        (4, state.mounted),
        (6, state.in_mog_house),
        (3, state.engaged_party),
        (2, state.engaged_solo),
        (7, state.fishing),
        (zone_pref[0], true),
        (zone_pref[1], true),
    ];
    for (slot, eligible) in candidates {
        if !eligible {
            continue;
        }
        if let Some(track) = slots.tracks[slot as usize] {
            if track == 0 {
                return None;
            }
            return Some((slot, track));
        }
    }
    None
}

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct AudioMuteState {
    pub bgm: bool,

    pub sfx: bool,
}

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

pub fn derive_bgm_playback_state(
    scene: Res<crate::snapshot::SceneState>,
    sky: Res<crate::sun_moon::VanaSky>,
    mut state: ResMut<BgmPlaybackState>,
    mut last_engage_log: Local<Option<(bool, u32, u8, bool)>>,
) {
    const EFFECT_KO: u16 = 0;
    const EFFECT_FISHING_IMAGERY: u16 = 235;
    const EFFECT_MOUNTED: u16 = 252;

    let snap = &scene.snapshot;
    let self_id = snap.self_char_id;
    let self_entity = self_id.and_then(|id| snap.entities.iter().find(|e| e.id == id));
    let self_bt_target = self_entity.map(|e| e.bt_target_id).unwrap_or(0);
    let self_status = self_entity.map(|e| e.status).unwrap_or(0);
    let goal_engaged = matches!(
        snap.current_goal,
        Some(ffxi_viewer_wire::ReactorGoal::Engaged { .. })
    );
    let engaged = self_bt_target != 0;
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

    let engage_key = (engaged, self_bt_target, self_status, goal_engaged);
    if *last_engage_log != Some(engage_key) {
        *last_engage_log = Some(engage_key);
        info!(
            target: "audio::bgm",
            self_id = ?self_id,
            engaged_signal = engaged,
            self_bt_target_id = self_bt_target,
            self_status_byte = self_status,
            reactor_goal_engaged = goal_engaged,
            in_party,
            "engage signals: bt_target drives battle music; goal/status shown for comparison"
        );
    }

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

pub fn drain_music_events_system(events: Res<EventLog>, mut slots: ResMut<BgmSlots>) {
    let total = events.pushed_total;
    let first_global = total.saturating_sub(events.recent.len() as u64);

    let start = (slots.event_cursor as u64).max(first_global);
    for g in start..total {
        match &events.recent[(g - first_global) as usize] {
            ViewerEvent::ZoneChanged { .. } => {
                slots.tracks = [None; SLOT_COUNT];
            }
            ViewerEvent::MusicChanged { slot, track_id } => {
                let s = *slot as usize;
                if s < SLOT_COUNT {
                    let (name, composer) = ffxi_audio::music_catalog::lookup(*track_id)
                        .map(|(_, n, c)| (n, c))
                        .unwrap_or(("?", "?"));
                    info!(
                        "audio: 0x05F slot={} ({}) track={} — \"{}\" by {}",
                        slot,
                        slot_name(*slot),
                        track_id,
                        name,
                        composer,
                    );
                    slots.tracks[s] = Some(*track_id);
                }
            }
            ViewerEvent::MusicVolumeChanged { slot, volume } => {
                let s = *slot as usize;
                if s < SLOT_COUNT {
                    slots.slot_gain[s] = (*volume as f32 / 127.0).clamp(0.0, 1.0);
                }
            }
            _ => {}
        }
    }
    slots.event_cursor = total as usize;
}

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

fn bgm_swap_needed(prev: Option<(u8, u16)>, new: Option<(u8, u16)>) -> bool {
    prev.map(|(_, t)| t) != new.map(|(_, t)| t)
}

pub fn apply_bgm_system(
    mut slots: ResMut<BgmSlots>,
    state: Res<BgmPlaybackState>,
    mute: Res<AudioMuteState>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut pcm_assets: ResMut<Assets<PcmAudio>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    mut warned: Local<bool>,
) {
    let Some(install) = slots.install_root.clone() else {
        if !*warned {
            warn!(
                "audio: BGM disabled — neither FFXI_DAT_PATH is set \
                 nor does ./vendor/game-files/SquareEnix/FINAL FANTASY XI/sound/win exist. \
                 Set FFXI_DAT_PATH to the install root to enable BGM."
            );
            *warned = true;
        }
        return;
    };

    let resolved = if mute.bgm {
        None
    } else {
        resolve_audible_slot(&slots, &state)
    };

    if !bgm_swap_needed(slots.active, resolved) {
        slots.active = resolved;
        return;
    }

    info!(
        target: "audio::bgm",
        "transition: {:?} → {:?} | slots={:?} state={:?}",
        slots.active, resolved, slots.tracks, *state
    );

    if let Some(e) = slots.active_entity.take() {
        if let Ok(mut ent) = commands.get_entity(e) {
            ent.insert(AudioFade::fade_out(BGM_FADE_SECS));
        }
    }
    slots.active = resolved;

    slots.bgm_loop_counter = None;
    slots.bgm_loops_reported = 0;

    let Some((slot, track_id)) = resolved else {
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

    let frames = decoded.frames();
    let sr = decoded.sample_rate;
    let ch = decoded.channels;
    let file_loop_frame = decoded.loop_start_sample;
    let mut pcm = PcmAudio::from_decoded(decoded);

    if pcm.loop_start_sample.is_none() {
        pcm = pcm.with_loop(Some(0));
    }
    let loop_counter = Arc::new(AtomicU64::new(0));
    pcm = pcm.with_loop_counter(loop_counter.clone());
    slots.bgm_loop_counter = Some(loop_counter);
    let handle = pcm_assets.add(pcm);

    let entity = commands
        .spawn((
            InGameEntity,
            AudioPlayer(handle),
            PlaybackSettings::ONCE.with_volume(bevy::audio::Volume::Linear(0.0)),
            AudioFade::fade_in(BGM_FADE_SECS),
        ))
        .id();
    slots.active_entity = Some(entity);
    info!(
        "audio: bgm {track_id} started ({} frames @ {:.0}Hz {}ch, loop_frame={:?})",
        frames, sr, ch, file_loop_frame
    );

    let (track_name, composer) = ffxi_audio::music_catalog::lookup(track_id)
        .map(|(_, n, c)| (n, c))
        .unwrap_or(("?", "?"));
    toasts.write(crate::snapshot::ToastEvent::system(format!(
        "♪ Now playing: \"{}\" by {} [track #{}, slot={}]",
        track_name,
        composer,
        track_id,
        slot_name(slot),
    )));
    let _ = asset_server;
}

pub fn report_bgm_loops_system(
    mut slots: ResMut<BgmSlots>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
) {
    let Some(counter) = slots.bgm_loop_counter.as_ref() else {
        return;
    };
    let now = counter.load(Ordering::Relaxed);
    if now <= slots.bgm_loops_reported {
        return;
    }
    let track_id = slots.active.map(|(_, t)| t).unwrap_or(0);

    for n in (slots.bgm_loops_reported + 1)..=now {
        toasts.write(crate::snapshot::ToastEvent::debug(format!(
            "♪ Loop: track #{} ({} loops since start)",
            track_id, n,
        )));
    }
    slots.bgm_loops_reported = now;
}

#[derive(Message, Debug, Clone, Copy)]
pub struct SfxEvent {
    pub se_id: u32,

    pub volume: f32,
}

impl SfxEvent {
    pub fn new(se_id: u32) -> Self {
        Self { se_id, volume: 1.0 }
    }
}

#[derive(Resource, Default, Debug)]
pub struct SeRegistry {
    pub by_name: std::collections::HashMap<[u8; 4], u32>,
}

impl SeRegistry {
    pub fn lookup(&self, name: [u8; 4]) -> Option<u32> {
        self.by_name.get(&name).copied()
    }
}

#[derive(Resource, Default)]
pub struct SfxCache {
    cached: std::collections::HashMap<u32, Handle<PcmAudio>>,
}

pub fn play_sfx_system(
    mut events: MessageReader<SfxEvent>,
    slots: Res<BgmSlots>,
    mute: Res<AudioMuteState>,
    mut cache: ResMut<SfxCache>,
    mut pcm_assets: ResMut<Assets<PcmAudio>>,
    mut commands: Commands,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    mut last_chat: Local<Option<(u32, std::time::Instant)>>,
    mut warned: Local<bool>,
) {
    if events.is_empty() {
        return;
    }
    if mute.sfx {
        for _ in events.read() {}
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
                warn!(
                    "audio: sfx {} not found under {}",
                    ev.se_id,
                    install.display()
                );
                continue;
            };
            let decoded = match decode_file(&path) {
                Ok(d) => d,
                Err(e) => {
                    warn!("audio: sfx {} decode failed: {e}", ev.se_id);
                    continue;
                }
            };

            let h = pcm_assets.add(PcmAudio::from_decoded(decoded).with_loop(None));
            cache.cached.insert(ev.se_id, h.clone());
            h
        };
        commands.spawn((
            InGameEntity,
            AudioPlayer(handle),
            PlaybackSettings::DESPAWN
                .with_volume(bevy::audio::Volume::Linear(ev.volume.clamp(0.0, 1.0))),
        ));

        let now = std::time::Instant::now();
        let dup = matches!(
            *last_chat,
            Some((id, t)) if id == ev.se_id
                && now.saturating_duration_since(t) < std::time::Duration::from_millis(250)
        );
        if !dup {
            toasts.write(crate::snapshot::ToastEvent::debug(format!(
                "✦ SFX #{}",
                ev.se_id
            )));
            *last_chat = Some((ev.se_id, now));
        }
    }
}

#[derive(Resource, Debug, Clone, Default)]
pub struct SystemSfxTable {
    pub zone_changed: Option<u32>,

    pub low_hp: Option<u32>,

    pub engaged_by: Option<u32>,

    pub engage_self: Option<u32>,

    pub swing_tick: Option<u32>,

    pub tell_received: Option<u32>,

    pub level_up: Option<u32>,

    pub skill_level_up: Option<u32>,

    pub ui_chat_open: Option<u32>,

    pub ui_chat_send: Option<u32>,

    pub ui_chat_cancel: Option<u32>,

    pub ui_menu_open: Option<u32>,

    pub ui_menu_move: Option<u32>,

    pub ui_menu_confirm: Option<u32>,

    pub ui_menu_cancel: Option<u32>,

    pub ui_command_ok: Option<u32>,

    pub ui_command_err: Option<u32>,
}

#[derive(Resource, Default)]
pub struct SystemSfxCursor {
    pos: usize,
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct CombatSfxState {
    pub prev_engaged: bool,
    pub prev_battle_count: usize,
}

pub fn fire_combat_sfx_events(
    scene: Res<crate::snapshot::SceneState>,
    table: Res<SystemSfxTable>,
    mut state: ResMut<CombatSfxState>,
    mut writer: MessageWriter<SfxEvent>,
) {
    let snap = &scene.snapshot;

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

#[derive(Default, PartialEq, Eq, Clone, Copy, Debug)]
pub enum InputModeKind {
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

            crate::InputMode::TargetAction(_) => Self::QuickAction,
            crate::InputMode::Dialog(_) => Self::Dialog,
            crate::InputMode::PassiveCursor(_) => Self::PassiveCursor,
        }
    }
}

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
            (_, InputModeKind::Chat) => table.ui_chat_open,

            (InputModeKind::Chat, _) => table.ui_chat_cancel,

            (_, InputModeKind::Menu) | (_, InputModeKind::QuickAction) => table.ui_menu_open,

            (InputModeKind::Menu, _) | (InputModeKind::QuickAction, _) => table.ui_menu_cancel,
            _ => None,
        };
        if let Some(se_id) = id {
            writer.write(SfxEvent::new(se_id));
        }
        *prev_kind = new_kind;
    } else if new_kind == InputModeKind::Menu && new_menu_depth != *prev_menu_depth {
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

#[derive(Debug, Clone, Copy, Default)]
pub struct WeatherSfxEntry {
    pub stinger: Option<u32>,
    pub ambient: Option<u32>,
}

pub const WEATHER_FADE_SECS: f32 = 1.0;

pub const BGM_FADE_SECS: f32 = 1.5;

#[derive(Resource, Debug, Clone, Default)]
pub struct WeatherSfxTable {
    pub entries: [WeatherSfxEntry; 20],
}

impl WeatherSfxTable {
    pub fn get(&self, weather: ffxi_viewer_wire::Weather) -> WeatherSfxEntry {
        self.entries[weather as usize]
    }

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

#[derive(Resource, Debug, Default)]
pub struct WeatherAmbient {
    pub active_entity: Option<Entity>,
    pub active_weather: Option<ffxi_viewer_wire::Weather>,
    pub prev_weather: Option<ffxi_viewer_wire::Weather>,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct AudioFade {
    pub from: f32,
    pub to: f32,
    pub t: f32,
    pub duration: f32,
    pub despawn_on_end: bool,
}

impl AudioFade {
    fn fade_in(duration: f32) -> Self {
        Self {
            from: 0.0,
            to: 1.0,
            t: 0.0,
            duration,
            despawn_on_end: false,
        }
    }
    fn fade_out(duration: f32) -> Self {
        Self {
            from: 1.0,
            to: 0.0,
            t: 0.0,
            duration,
            despawn_on_end: true,
        }
    }
}

pub fn observe_weather_changes(
    scene: Res<crate::snapshot::SceneState>,
    table: Res<WeatherSfxTable>,
    slots: Res<BgmSlots>,
    mut cache: ResMut<SfxCache>,
    mut pcm_assets: ResMut<Assets<PcmAudio>>,
    mut ambient: ResMut<WeatherAmbient>,
    mut sfx_writer: MessageWriter<SfxEvent>,
    fade_q: Query<Entity, With<AudioFade>>,
    mut commands: Commands,
) {
    let current = scene.snapshot.weather;
    if current == ambient.prev_weather {
        return;
    }
    let prev = ambient.prev_weather;
    ambient.prev_weather = current;

    let Some(weather) = current else {
        if let Some(e) = ambient.active_entity.take() {
            if let Ok(mut ent) = commands.get_entity(e) {
                ent.insert(AudioFade::fade_out(WEATHER_FADE_SECS));
            }
            ambient.active_weather = None;
        }
        return;
    };

    let entry = table.get(weather);

    if let Some(se_id) = entry.stinger {
        sfx_writer.write(SfxEvent::new(se_id));
    }

    let new_ambient_id = entry.ambient;
    let prev_ambient_id = prev.map(|w| table.get(w).ambient).unwrap_or(None);

    if new_ambient_id == prev_ambient_id && ambient.active_entity.is_some() {
        ambient.active_weather = Some(weather);
        return;
    }

    if let Some(e) = ambient.active_entity.take() {
        if let Ok(mut ent) = commands.get_entity(e) {
            ent.insert(AudioFade::fade_out(WEATHER_FADE_SECS));
        }
    }

    for e in fade_q.iter() {
        if Some(e) != ambient.active_entity {}
    }

    ambient.active_weather = Some(weather);

    let Some(se_id) = new_ambient_id else {
        return;
    };
    let Some(install) = slots.install_root.clone() else {
        return;
    };

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

        let h = pcm_assets.add(PcmAudio::from_decoded(decoded).with_loop(None));
        cache.cached.insert(se_id, h.clone());
        h
    };

    let looped_handle = pcm_assets
        .get(&handle)
        .cloned()
        .map(|p| pcm_assets.add(p.with_loop(Some(0))))
        .unwrap_or(handle);

    let entity = commands
        .spawn((
            InGameEntity,
            AudioPlayer(looped_handle),
            PlaybackSettings::ONCE.with_volume(bevy::audio::Volume::Linear(0.0)),
            AudioFade::fade_in(WEATHER_FADE_SECS),
        ))
        .id();
    ambient.active_entity = Some(entity);
    info!(
        "audio: weather ambient se={se_id} weather={:?} (fade-in {}s)",
        weather, WEATHER_FADE_SECS
    );
}

pub fn tick_audio_fades(
    time: Res<Time>,
    mut q: Query<(Entity, &mut AudioFade, Option<&mut bevy::audio::AudioSink>)>,
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
                commands.entity(entity).remove::<AudioFade>();
            }
        }
    }
}

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<PcmAudio>();
        app.init_resource::<BgmSlots>()
            .init_resource::<BgmPlaybackState>()
            .init_resource::<AudioMuteState>()
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
                    report_bgm_loops_system,
                    fire_system_sfx_events,
                    fire_combat_sfx_events,
                    observe_ui_mode_transitions,
                    observe_weather_changes,
                    play_sfx_system,
                    tick_audio_fades,
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
    fn default_state_picks_zone_not_combat_when_both_filled() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101);
        slots.tracks[2] = Some(99);
        let state = BgmPlaybackState::default();
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
        slots.tracks[5] = Some(70);
        let state = BgmPlaybackState {
            engaged_solo: true,
            dead: true,
            ..Default::default()
        };
        assert_eq!(resolve_audible_slot(&slots, &state), Some((5, 70)));
    }

    #[test]
    fn dead_track_zero_means_silence_not_fallthrough() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101);
        slots.tracks[5] = Some(0);
        let state = BgmPlaybackState {
            dead: true,
            ..Default::default()
        };
        assert_eq!(resolve_audible_slot(&slots, &state), None);
    }

    #[test]
    fn swap_skipped_when_only_slot_changes() {
        assert!(!bgm_swap_needed(Some((0, 24)), Some((1, 24))));
        assert!(!bgm_swap_needed(Some((1, 24)), Some((0, 24))));

        assert!(bgm_swap_needed(Some((0, 24)), Some((1, 25))));

        assert!(bgm_swap_needed(None, Some((0, 24))));
        assert!(bgm_swap_needed(Some((0, 24)), None));

        assert!(!bgm_swap_needed(None, None));
    }

    #[test]
    fn night_prefers_zone_night_over_zone_day() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101);
        slots.tracks[1] = Some(102);
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
        let mut app = App::new();
        app.init_resource::<EventLog>()
            .init_resource::<BgmSlots>()
            .add_systems(Update, drain_music_events_system);

        {
            let mut events = app.world_mut().resource_mut::<EventLog>();
            events.recent.push_back(ViewerEvent::MusicChanged {
                slot: 2,
                track_id: 99,
            });
            events.recent.push_back(ViewerEvent::MusicVolumeChanged {
                slot: 2,
                volume: 64,
            });
            events.pushed_total = 2;
        }
        app.update();

        let slots = app.world().resource::<BgmSlots>();
        assert_eq!(slots.tracks[2], Some(99));
        assert!((slots.slot_gain[2] - 64.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn zone_change_clears_stale_music_slots() {
        let mut app = App::new();
        app.init_resource::<EventLog>()
            .init_resource::<BgmSlots>()
            .add_systems(Update, drain_music_events_system);

        {
            let mut events = app.world_mut().resource_mut::<EventLog>();
            events.recent.push_back(ViewerEvent::MusicChanged {
                slot: 5,
                track_id: 204,
            });
            events.pushed_total = 1;
        }
        app.update();
        assert_eq!(app.world().resource::<BgmSlots>().tracks[5], Some(204));

        {
            let mut events = app.world_mut().resource_mut::<EventLog>();
            events.recent.push_back(ViewerEvent::ZoneChanged {
                from: Some(1),
                to: 241,
            });
            events.recent.push_back(ViewerEvent::MusicChanged {
                slot: 0,
                track_id: 151,
            });
            events.pushed_total = 3;
        }
        app.update();

        let slots = app.world().resource::<BgmSlots>();
        assert_eq!(
            slots.tracks[5], None,
            "stale Dead-slot music must clear on a zone change so it can't outlive the homepoint warp"
        );
        assert_eq!(
            slots.tracks[0],
            Some(151),
            "the new zone's MusicNum (which follows ZoneChanged in the queue) must repopulate"
        );
    }

    #[test]
    fn drain_cursor_survives_event_log_front_popping() {
        let mut app = App::new();
        app.init_resource::<EventLog>()
            .init_resource::<BgmSlots>()
            .add_systems(Update, drain_music_events_system);

        {
            let mut events = app.world_mut().resource_mut::<EventLog>();
            events.recent.push_back(ViewerEvent::MusicChanged {
                slot: 0,
                track_id: 10,
            });
            events.pushed_total = 1;
        }
        app.update();
        assert_eq!(app.world().resource::<BgmSlots>().tracks[0], Some(10));

        {
            let mut events = app.world_mut().resource_mut::<EventLog>();
            events.recent.pop_front();
            events.recent.push_back(ViewerEvent::MusicChanged {
                slot: 1,
                track_id: 20,
            });
            events.pushed_total = 2;
        }
        app.update();

        assert_eq!(
            app.world().resource::<BgmSlots>().tracks[1],
            Some(20),
            "a positional cursor would skip this; the pushed_total cursor must still see it"
        );
    }

    #[test]
    fn system_sfx_fires_only_for_mapped_events() {
        let mut app = App::new();
        let table = SystemSfxTable {
            zone_changed: Some(7001),
            low_hp: Some(7002),
            engaged_by: Some(7003),
            ..Default::default()
        };

        app.add_message::<SfxEvent>()
            .init_resource::<EventLog>()
            .insert_resource(table)
            .init_resource::<SystemSfxCursor>()
            .add_systems(Update, fire_system_sfx_events);

        let mut events = app.world_mut().resource_mut::<EventLog>();
        events.recent.push_back(ViewerEvent::ZoneChanged {
            from: None,
            to: 100,
        });
        events.recent.push_back(ViewerEvent::LowHp { pct: 15 });

        events
            .recent
            .push_back(ViewerEvent::Reconnected { downtime_ms: 500 });
        events
            .recent
            .push_back(ViewerEvent::EngagedBy { entity_id: 42 });

        app.update();

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
        assert_eq!(sfx_messages[0], 7001);
        assert_eq!(sfx_messages[1], 7002);
        assert_eq!(sfx_messages[2], 7003);
    }

    #[test]
    fn system_sfx_default_table_is_all_silent() {
        let table = SystemSfxTable::default();
        assert!(table.zone_changed.is_none());
        assert!(table.low_hp.is_none());
        assert!(table.engaged_by.is_none());
        assert!(table.engage_self.is_none());
        assert!(table.swing_tick.is_none());
        assert!(table.level_up.is_none());
        assert!(table.skill_level_up.is_none());
    }

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
    fn bgm_mute_resolves_to_silence_regardless_of_slot_state() {
        let mut slots = BgmSlots::default();
        slots.tracks[0] = Some(101);
        let state = BgmPlaybackState::default();

        let mute = AudioMuteState::default();
        let resolved = if mute.bgm {
            None
        } else {
            resolve_audible_slot(&slots, &state)
        };
        assert_eq!(resolved, Some((0, 101)));

        let mute = AudioMuteState {
            bgm: true,
            sfx: false,
        };
        let resolved = if mute.bgm {
            None
        } else {
            resolve_audible_slot(&slots, &state)
        };
        assert_eq!(resolved, None);
    }

    #[test]
    fn combat_sfx_engage_latches_once_per_zero_to_engaged_transition() {
        let mut s = CombatSfxState::default();

        assert_eq!(step_combat_latches(&mut s, false, 0), (false, false));

        assert_eq!(step_combat_latches(&mut s, true, 0), (true, false));

        assert_eq!(step_combat_latches(&mut s, true, 0), (false, false));

        assert_eq!(step_combat_latches(&mut s, false, 0), (false, false));

        assert_eq!(step_combat_latches(&mut s, true, 0), (true, false));
    }

    #[test]
    fn combat_sfx_swing_tick_fires_only_on_battle_chat_growth() {
        let mut s = CombatSfxState::default();

        assert_eq!(step_combat_latches(&mut s, false, 0), (false, false));

        assert_eq!(step_combat_latches(&mut s, false, 1), (false, true));

        assert_eq!(step_combat_latches(&mut s, false, 1), (false, false));

        assert_eq!(step_combat_latches(&mut s, false, 4), (false, true));

        assert_eq!(step_combat_latches(&mut s, false, 2), (false, false));
    }

    #[test]
    fn bgm_pipeline_end_to_end_with_real_install() {
        let Ok(install) = std::env::var("FFXI_DAT_PATH") else {
            eprintln!("skipping: FFXI_DAT_PATH not set");
            return;
        };

        let mut app = App::new();

        app.add_plugins(bevy::MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<PcmAudio>();

        let slots = BgmSlots {
            install_root: Some(std::path::PathBuf::from(install)),
            ..Default::default()
        };
        app.insert_resource(slots)
            .init_resource::<EventLog>()
            .init_resource::<BgmPlaybackState>()
            .init_resource::<AudioMuteState>()
            .init_resource::<crate::snapshot::SceneState>()
            .add_systems(
                Update,
                (drain_music_events_system, apply_bgm_system).chain(),
            );

        let mut events = app.world_mut().resource_mut::<EventLog>();
        events.recent.push_back(ViewerEvent::MusicChanged {
            slot: 0,
            track_id: 101,
        });

        app.update();

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
