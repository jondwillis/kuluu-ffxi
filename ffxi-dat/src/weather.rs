use std::collections::HashMap;

use crate::{
    chunk::{self, ChunkNode},
    kind::ChunkKind,
    mmb::D3DCOLOR_CHANNEL_MASK,
    mzb::AreaResourceId,
    DatError, DatRoot, Result,
};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WeatherRecord {
    pub time_minutes: u32,

    // research/xim EnvironmentSection.kt indoorFlag@0 (==1 => indoors).
    pub indoors: bool,

    pub sunlight_diffuse_entity: [f32; 4],
    pub moonlight_diffuse_entity: [f32; 4],
    // research/xim EnvironmentSection.kt getDirectionOfIndoorDiffuseLight:
    // when indoors, the block's moonLightColor bytes are not a color but a SIGNED
    // to-light direction (byte as i8 / 128, normalized, negated), and the single
    // static indoor DiffuseLight replaces the sun/moon arc. FFXI (Y-down) space.
    pub indoor_light_dir_entity: [f32; 3],
    pub ambient_entity: [f32; 4],
    pub fog_entity: [f32; 4],
    pub max_fog_dist_entity: f32,
    pub min_fog_dist_entity: f32,
    // research/xim EnvironmentSection.kt read LightConfig.diffuseMultiplier (model block @36).
    pub diffuse_mul_entity: f32,

    pub sunlight_diffuse_landscape: [f32; 4],
    pub moonlight_diffuse_landscape: [f32; 4],
    pub indoor_light_dir_landscape: [f32; 3],
    pub ambient_landscape: [f32; 4],
    pub fog_landscape: [f32; 4],
    pub max_fog_dist_landscape: f32,
    pub min_fog_dist_landscape: f32,
    // research/xim EnvironmentSection.kt read LightConfig.diffuseMultiplier (terrain block @68).
    pub diffuse_mul_landscape: f32,

    pub fog_offset: f32,
    pub max_far_clip: f32,

    // research/xim EnvironmentSection.kt clearColor@76 == XIClient
    // WorldParameters.BackgroundColor: the record's own backdrop. Retail's
    // DrawSky overrides it outdoors with the interpolated horizon slice
    // (XiZone.cpp DrawSky SetBackColor(bsarr[0])), so this only shows through
    // indoors, where the dome is not drawn (xim EnvironmentManager.kt getClearColor).
    pub background_color: [f32; 4],

    pub skybox_colors: [[f32; 4]; 8],

    pub skybox_altitudes: [f32; 8],
}

#[derive(Debug, thiserror::Error)]
pub enum WeatherError {
    #[error("Weather chunk too small: need at least {needed} bytes for WeatherData, got {actual}")]
    TooSmall { needed: usize, actual: usize },
    #[error("Weather chunk name {0:?} is not a valid HHMM time string")]
    BadTimeName([u8; 4]),
}

impl From<WeatherError> for DatError {
    fn from(e: WeatherError) -> Self {
        DatError::Weather(format!("{e}"))
    }
}

pub const WEATHER_DATA_SIZE: usize = 176;

pub fn parse_weather_record(name: &[u8; 4], body: &[u8]) -> Result<WeatherRecord> {
    if body.len() < WEATHER_DATA_SIZE {
        return Err(WeatherError::TooSmall {
            needed: WEATHER_DATA_SIZE,
            actual: body.len(),
        }
        .into());
    }
    let time_minutes = parse_time_name(name)?;

    let u32_at =
        |off: usize| u32::from_le_bytes([body[off], body[off + 1], body[off + 2], body[off + 3]]);
    let f32_at =
        |off: usize| f32::from_le_bytes([body[off], body[off + 1], body[off + 2], body[off + 3]]);

    let diffuse_mul_entity = f32_at(36);
    let diffuse_mul_landscape = f32_at(68);

    Ok(WeatherRecord {
        time_minutes,
        indoors: u32_at(0) == 1,
        sunlight_diffuse_entity: diffuse_to_color(u32_at(12), diffuse_mul_entity),
        moonlight_diffuse_entity: diffuse_to_color(u32_at(16), diffuse_mul_entity),
        indoor_light_dir_entity: indoor_light_direction(u32_at(16)),
        ambient_entity: ambient_to_color(u32_at(20)),
        fog_entity: u32_to_rgba(u32_at(24)),
        max_fog_dist_entity: f32_at(28),
        min_fog_dist_entity: f32_at(32),
        diffuse_mul_entity,

        sunlight_diffuse_landscape: diffuse_to_color(u32_at(44), diffuse_mul_landscape),
        moonlight_diffuse_landscape: diffuse_to_color(u32_at(48), diffuse_mul_landscape),
        indoor_light_dir_landscape: indoor_light_direction(u32_at(48)),
        ambient_landscape: ambient_to_color(u32_at(52)),
        fog_landscape: u32_to_rgba(u32_at(56)),
        max_fog_dist_landscape: f32_at(60),
        min_fog_dist_landscape: f32_at(64),
        diffuse_mul_landscape,

        fog_offset: f32_at(80),
        max_far_clip: f32_at(88),
        background_color: u32_to_rgba(u32_at(76)),

        skybox_colors: [
            u32_to_rgba(u32_at(108)),
            u32_to_rgba(u32_at(112)),
            u32_to_rgba(u32_at(116)),
            u32_to_rgba(u32_at(120)),
            u32_to_rgba(u32_at(124)),
            u32_to_rgba(u32_at(128)),
            u32_to_rgba(u32_at(132)),
            u32_to_rgba(u32_at(136)),
        ],
        skybox_altitudes: [
            f32_at(140),
            f32_at(144),
            f32_at(148),
            f32_at(152),
            f32_at(156),
            f32_at(160),
            f32_at(164),
            f32_at(168),
        ],
    })
}

fn u32_to_rgba(c: u32) -> [f32; 4] {
    let r = (c & D3DCOLOR_CHANNEL_MASK) as f32 / 255.0;
    let g = ((c >> 8) & D3DCOLOR_CHANNEL_MASK) as f32 / 255.0;
    let b = ((c >> 16) & D3DCOLOR_CHANNEL_MASK) as f32 / 255.0;
    let a = ((c >> 24) & D3DCOLOR_CHANNEL_MASK) as f32 / 128.0;
    [r, g, b, a]
}

// research/xim EnvironmentSection.kt EnvironmentLighting biasThreshold,184-204.
const COLOR_BIAS: [f32; 3] = [1.4, 1.36, 1.45];
const BIAS_THRESHOLD_BYTE: u32 = 0xCC;
const BIAS_THRESHOLD_F: f32 = 0xCC as f32 / 0xFF as f32;

// research/xim EnvironmentSection.kt diffuseToColor: byte/255*mul, then
// channel-wise colorBias iff every multiplied channel < 0xCC/0xFF, clamped [0,1]
// only when bias applied.
pub fn diffuse_to_color(byte_rgba: u32, mul: f32) -> [f32; 4] {
    let mut r = (byte_rgba & D3DCOLOR_CHANNEL_MASK) as f32 / 255.0 * mul;
    let mut g = ((byte_rgba >> 8) & D3DCOLOR_CHANNEL_MASK) as f32 / 255.0 * mul;
    let mut b = ((byte_rgba >> 16) & D3DCOLOR_CHANNEL_MASK) as f32 / 255.0 * mul;
    let a = ((byte_rgba >> 24) & D3DCOLOR_CHANNEL_MASK) as f32 / 128.0;

    let apply_bias = r < BIAS_THRESHOLD_F && g < BIAS_THRESHOLD_F && b < BIAS_THRESHOLD_F;
    if apply_bias {
        r = (r * COLOR_BIAS[0]).clamp(0.0, 1.0);
        g = (g * COLOR_BIAS[1]).clamp(0.0, 1.0);
        b = (b * COLOR_BIAS[2]).clamp(0.0, 1.0);
    }
    [r, g, b, a]
}

// research/xim EnvironmentSection.kt ambientToColor: bias iff every RAW
// byte < 0xCC, channel = bias*byte/510, then upper-ceiling 0.5 (Color.clamp(0.5)
// == coerceIn(0,0.5), a max not a min).
pub fn ambient_to_color(byte_rgba: u32) -> [f32; 4] {
    let rb = byte_rgba & D3DCOLOR_CHANNEL_MASK;
    let gb = (byte_rgba >> 8) & D3DCOLOR_CHANNEL_MASK;
    let bb = (byte_rgba >> 16) & D3DCOLOR_CHANNEL_MASK;
    let ab = (byte_rgba >> 24) & D3DCOLOR_CHANNEL_MASK;

    let bias = if rb < BIAS_THRESHOLD_BYTE && gb < BIAS_THRESHOLD_BYTE && bb < BIAS_THRESHOLD_BYTE {
        COLOR_BIAS
    } else {
        [1.0, 1.0, 1.0]
    };

    let r = (bias[0] * rb as f32 / 510.0).min(0.5);
    let g = (bias[1] * gb as f32 / 510.0).min(0.5);
    let b = (bias[2] * bb as f32 / 510.0).min(0.5);
    let a = (ab as f32 / 128.0).min(0.5);
    [r, g, b, a]
}

// research/xim EnvironmentSection.kt getDirectionOfIndoorDiffuseLight: signed bytes / 128, normalized,
// negated. Degenerate (all-zero) input yields the zero vector; consumers treat a
// zero direction as "no indoor diffuse light".
pub fn indoor_light_direction(byte_rgba: u32) -> [f32; 3] {
    let x = (byte_rgba & D3DCOLOR_CHANNEL_MASK) as u8 as i8 as f32 / 128.0;
    let y = ((byte_rgba >> 8) & D3DCOLOR_CHANNEL_MASK) as u8 as i8 as f32 / 128.0;
    let z = ((byte_rgba >> 16) & D3DCOLOR_CHANNEL_MASK) as u8 as i8 as f32 / 128.0;
    let len = (x * x + y * y + z * z).sqrt();
    if len <= f32::EPSILON {
        return [0.0, 0.0, 0.0];
    }
    [-x / len, -y / len, -z / len]
}

fn parse_time_name(name: &[u8; 4]) -> Result<u32> {
    let mut acc = 0u32;
    for &b in name {
        if !b.is_ascii_digit() {
            return Err(WeatherError::BadTimeName(*name).into());
        }
        acc = acc * 10 + (b - b'0') as u32;
    }
    let hh = acc / 100;
    let mm = acc % 100;
    if hh >= 24 || mm >= 60 {
        return Err(WeatherError::BadTimeName(*name).into());
    }
    Ok(hh * 60 + mm)
}

pub fn collect_weather_records(dat_bytes: &[u8]) -> Vec<WeatherRecord> {
    let mut out: Vec<WeatherRecord> = Vec::new();
    for c in chunk::walk(dat_bytes).filter_map(std::result::Result::ok) {
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::Weather) {
            continue;
        }
        if let Ok(r) = parse_weather_record(&c.name, c.data) {
            out.push(r);
        }
    }

    out.retain(|r| {
        !r.skybox_colors
            .iter()
            .all(|c| c[0] == 0.0 && c[1] == 0.0 && c[2] == 0.0)
    });
    out.sort_by_key(|r| r.time_minutes);

    out.dedup_by_key(|r| r.time_minutes);
    out
}

pub type WeatherTypeId = [u8; 4];

// Retail indexes this table with the raw server weather number and does not
// bounds-check it: research/XIClient/src/XIClient/source/World/Weather/
// WeatherCondition.cpp (`WeatherTable1`), read by XiZone.cpp XiZone::GetWeatherResourceID
// `GetWeatherResourceID`. Rows are transcribed in DAT byte order; XIClient
// writes them as reversed multi-char int literals ('enif' == b"fine").
// The LSB id ordering (vendor/server/data/enums/weather.yaml Weather, None=0 ..
// Darkness=19) lines up 1:1 with the table index, so row 0 is `fine` — retail
// has no `None` special case. Do NOT source these from
// WeatherCondition.cpp's sibling `WeatherKeyframeLibrary.cpp`: that array is a
// FourCC-keyed load list, is compacted before use, and its row 7 literal is a
// typo (`sqal`, which no shipped directory uses).
const WEATHER_TYPE_IDS: [WeatherTypeId; 20] = [
    *b"fine", // 0  None
    *b"suny", // 1  Sunshine
    *b"clod", // 2  Clouds
    *b"mist", // 3  Fog
    *b"dryw", // 4  HotSpell
    *b"heat", // 5  HeatWave
    *b"rain", // 6  Rain
    *b"squl", // 7  Squall
    *b"dust", // 8  DustStorm
    *b"sand", // 9  SandStorm
    *b"wind", // 10 Wind
    *b"stom", // 11 Gales
    *b"snow", // 12 Snow
    *b"bliz", // 13 Blizzards
    *b"thdr", // 14 Thunder
    *b"bolt", // 15 Thunderstorms
    *b"aura", // 16 Auroras
    *b"ligt", // 17 StellarGlare
    *b"fogd", // 18 Gloom
    *b"dark", // 19 Darkness
];

// XIClient ships a second, `1`-suffixed table selected per zone
// (WeatherCondition.cpp WeatherTable2, chosen by XiZone.cpp XiZone::SetWeatherTable `SetWeatherTable`).
// Those containers really ship — ROM4/0/11.DAT and siblings carry sun1/clo1/
// mis1/thd1/win1 as `weat/` children — but nothing here selects a table yet.

pub const WEATHER_TYPE_FALLBACK: WeatherTypeId = *b"suny";

// Map an LSB weather id (vendor/server/data/enums/weather.yaml ordering) onto the
// `weat/<type>` subdir DatId. wire::Weather shares this discriminant order, so the
// viewer passes `weather as u16` straight through.
/// The `weat/<type>` tag for a weather id we may not have received yet.
///
/// `None` means "the server has not told us", which is **not** weather id 0 —
/// id 0 is a real row (`fine`). Indexing the table with 0 as an unknown-marker
/// only looked right while the table collapsed ids 0 and 1 onto the same tag.
pub fn weather_type_id_or_default(lsb_weather_id: Option<u16>) -> WeatherTypeId {
    lsb_weather_id.map_or(WEATHER_TYPE_FALLBACK, weather_type_id)
}

pub fn weather_type_id(lsb_weather_id: u16) -> WeatherTypeId {
    *WEATHER_TYPE_IDS
        .get(lsb_weather_id as usize)
        .unwrap_or(&WEATHER_TYPE_FALLBACK)
}

/// One ambient bed keyed by the Vana'diel minute it takes over at.
///
/// research/XIClient/src/XIClient/source/World/Weather/WeatherTransition.cpp WeatherTransition::FindPrevSound
/// `FindPrevSound` — the 0x3D Seps nested under a `weat/<type>` container whose name
/// passes [`crate::sep::activate_time_minutes`] are the per-time-of-day zone ambience;
/// the ones that fail it are the payloads of the sibling sound generators, not beds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmbientCue {
    pub time_minutes: u32,
    pub se_id: u32,
    pub loops: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WeatherSet {
    pub outdoor: Vec<WeatherRecord>,
    pub indoor: Vec<WeatherRecord>,
    pub outdoor_ambient: Vec<AmbientCue>,
    pub indoor_ambient: Vec<AmbientCue>,
}

impl WeatherSet {
    pub fn is_empty(&self) -> bool {
        self.outdoor.is_empty()
            && self.indoor.is_empty()
            && self.outdoor_ambient.is_empty()
            && self.indoor_ambient.is_empty()
    }

    pub fn ambient(&self, indoor: bool) -> &[AmbientCue] {
        if indoor && !self.indoor_ambient.is_empty() {
            &self.indoor_ambient
        } else {
            &self.outdoor_ambient
        }
    }
}

/// The bed retail would be playing at `time_minutes`.
///
/// WeatherTransition.cpp WeatherTransition::FindPrevSound: the greatest activate time earlier than now, and when
/// nothing is earlier the LAST candidate enumerated — not the latest-keyed one, so `cues`
/// must stay in DAT order. Retail's `<` is on sub-minute tick times against a key whose
/// seconds are always zero (FileResource.cpp FileResource::GetActivateTime), so at whole-minute resolution the
/// key that just took over is `<=` rather than `<`.
pub fn select_ambient(cues: &[AmbientCue], time_minutes: u32) -> Option<&AmbientCue> {
    let now = time_minutes % 1440;
    cues.iter()
        .filter(|c| c.time_minutes <= now)
        .min_by_key(|c| now - c.time_minutes)
        .or_else(|| cues.last())
}

/// One area's environment: the per-weather-type record sets keyed by the
/// `weat/<type>` DatId subdirectory.
pub type WeatherSetsByType = HashMap<WeatherTypeId, WeatherSet>;

// research/xim EnvironmentManager.kt getAreaEnvironmentDirectories keys
// the per-weather environment sets by the weather DatId subdirectory under the
// zone root's `weat` directory; each carries its own per-hour 0x2F record set and
// an `indo` indoor variant. We mirror that grouping here instead of the flat
// sort+dedup collapse that loses the weather-type/indoor distinction.
#[derive(Debug, Clone, Default)]
pub struct ZoneWeatherSets {
    pub by_type: WeatherSetsByType,

    // Flat fallback for zones with no `weat` subtree (records harvested by a
    // plain chunk walk + nonblack retain).
    pub flat: Vec<WeatherRecord>,

    /// Per-area environments, keyed by the [`AreaResourceId`] the zone's MZB
    /// placements bind to (XiArea.cpp XiArea::XiArea: each area loads its own container,
    /// found in the zone container under its FourCC). Sibling directories of
    /// `weat` under the zone root — `ev01`, `ev02`, `subl` — hold the same
    /// `<type>/<hhmm>` record shape as `weat` itself, and are where retail gets
    /// the darker, sunless fog and diffuse lights it draws building interiors
    /// with (ZoneRenderer.cpp ZoneRenderer::RenderChunk2).
    pub by_area: HashMap<AreaResourceId, WeatherSetsByType>,
}

impl ZoneWeatherSets {
    pub fn is_empty(&self) -> bool {
        self.by_type.is_empty() && self.flat.is_empty()
    }

    /// The environment retail draws with for `area`.
    ///
    /// XiArea.cpp XiArea::FindAreaByFourCCAndGetFog (`FindAreaByFourCCAndGetFog`, and the identical
    /// ambient/diffuse-light accessors): a zero FourCC uses the zone's own area,
    /// and so does a FourCC that matches no loaded area — `FindAreaByFourCC`
    /// returns the zone on a miss (XiArea.cpp XiArea::FindAreaByFourCC). Zones do ship
    /// placements naming an area with no container (`ent4`, `ex02`), so the
    /// miss path is load-bearing, not defensive.
    pub fn area_by_type(&self, area: AreaResourceId) -> &WeatherSetsByType {
        if area == 0 {
            return &self.by_type;
        }
        self.by_area.get(&area).unwrap_or(&self.by_type)
    }
}

const WEAT_DIR: WeatherTypeId = *b"weat";
const INDO_DIR: WeatherTypeId = *b"indo";
// CYySepRes.cpp CYySepRes::CheckFourCC false rejects any Sep whose parent container FourCC is 'rtxe', which is
// `extr` in DAT byte order.
const EXTR_DIR: WeatherTypeId = *b"extr";

pub fn collect_zone_weather_sets(dat_bytes: &[u8]) -> ZoneWeatherSets {
    let tree = chunk::walk_tree(dat_bytes);
    let mut by_type: WeatherSetsByType = HashMap::new();
    let mut by_area: HashMap<AreaResourceId, WeatherSetsByType> = HashMap::new();

    // The `weat` directory sits under the zone root dir (e.g. f_ro/weat), not at
    // the file's top level, so we search the whole dir tree for it.
    find_weat_dirs(&tree, &mut by_type, &mut by_area);

    for set in by_type
        .values_mut()
        .chain(by_area.values_mut().flat_map(|a| a.values_mut()))
    {
        set.outdoor.sort_by_key(|r| r.time_minutes);
        set.outdoor.dedup_by_key(|r| r.time_minutes);
        set.indoor.sort_by_key(|r| r.time_minutes);
        set.indoor.dedup_by_key(|r| r.time_minutes);
    }

    let flat = if by_type.is_empty() {
        collect_weather_records(dat_bytes)
    } else {
        Vec::new()
    };

    ZoneWeatherSets {
        by_type,
        flat,
        by_area,
    }
}

fn find_weat_dirs(
    node: &ChunkNode,
    by_type: &mut WeatherSetsByType,
    by_area: &mut HashMap<AreaResourceId, WeatherSetsByType>,
) {
    // A dir holding `weat` is a zone root, so its *other* dir children are the
    // area containers retail resolves by FourCC (XiArea.cpp XiArea::XiArea). Keying off
    // the `weat` sibling instead of the record shape keeps `weat/suny` — itself
    // a dir of dirs of 0x2F records, via `indo`/`lf01` — from registering as an
    // area of its own.
    let is_zone_root = node
        .children
        .iter()
        .any(|c| c.chunk.kind == ChunkKind::Rmp as u8 && c.chunk.name == WEAT_DIR);

    for child in &node.children {
        if child.chunk.kind != ChunkKind::Rmp as u8 {
            continue;
        }
        if child.chunk.name == WEAT_DIR {
            harvest_weat_dir(child, by_type, HarvestAmbient::Yes);
            continue;
        }
        if is_zone_root {
            let mut area: WeatherSetsByType = HashMap::new();
            harvest_weat_dir(child, &mut area, HarvestAmbient::No);
            area.retain(|_, set| !set.is_empty());
            if !area.is_empty() {
                by_area
                    .entry(crate::mzb::area_resource_id_from_dir_name(
                        &child.chunk.name,
                    ))
                    .or_default()
                    .extend(area);
            }
        }
        find_weat_dirs(child, by_type, by_area);
    }
}

// Ambient beds are harvested only from a real `weat` container. The area-container arm
// of find_weat_dirs treats every dir beside `weat` as a weather-type parent, and a zone
// root's other subtrees ship Seps whose names are valid HHMM by coincidence
// (`f_tu/fser/0111` is a footstep, `f_tu/effe/ggwa/0000` a waterfall).
enum HarvestAmbient {
    Yes,
    No,
}

fn harvest_weat_dir(weat: &ChunkNode, by_type: &mut WeatherSetsByType, ambient: HarvestAmbient) {
    for type_node in &weat.children {
        if type_node.chunk.kind != ChunkKind::Rmp as u8 {
            continue;
        }
        let set = by_type.entry(type_node.chunk.name).or_default();
        push_weather_records(type_node, &mut set.outdoor);
        for child in &type_node.children {
            if child.chunk.kind == ChunkKind::Rmp as u8 && child.chunk.name == INDO_DIR {
                push_weather_records(child, &mut set.indoor);
            }
        }
        if matches!(ambient, HarvestAmbient::Yes) {
            push_ambient_cues(type_node, type_node.chunk.name, set);
        }
    }
}

// WeatherTransition.cpp WeatherTransition::FindPrevSound enumerates Seps with `PrepareFromResource(.., Sep, 0, -1)`,
// which descends the whole container, so a bed nested below the weather tag is in scope.
// :450-455 then splits them on whether their immediate parent is the `indo` child, and
// CYySepRes.cpp CYySepRes::CheckFourCC drops anything parented by `extr` outright.
fn push_ambient_cues(node: &ChunkNode, parent: WeatherTypeId, set: &mut WeatherSet) {
    for child in &node.children {
        if child.chunk.kind == ChunkKind::Rmp as u8 || !child.children.is_empty() {
            push_ambient_cues(child, child.chunk.name, set);
            continue;
        }
        if ChunkKind::from_u8(child.chunk.kind) != Some(ChunkKind::Sep) || parent == EXTR_DIR {
            continue;
        }
        let Some(time_minutes) = crate::sep::activate_time_minutes(&child.chunk.name) else {
            continue;
        };
        let Ok(sep) = crate::sep::Sep::parse(child.chunk.name, child.chunk.data) else {
            continue;
        };
        let cue = AmbientCue {
            time_minutes,
            se_id: sep.se_id,
            loops: sep.loops(),
        };
        if parent == INDO_DIR {
            set.indoor_ambient.push(cue);
        } else {
            set.outdoor_ambient.push(cue);
        }
    }
}

fn push_weather_records(dir: &ChunkNode, out: &mut Vec<WeatherRecord>) {
    for child in &dir.children {
        if ChunkKind::from_u8(child.chunk.kind) != Some(ChunkKind::Weather) {
            continue;
        }
        if let Ok(r) = parse_weather_record(&child.chunk.name, child.chunk.data) {
            out.push(r);
        }
    }
}

// research/xim EnvironmentManager.kt computeInterpolatedEnvResource currentHour: env resources are keyed by whole
// hour buckets; floorEntry is the max key <= current hour, ceilEntry the min key
// > current hour (wrapping to the first), and the blend spans [floorKey*60,
// (ceilKey==0 ? 24 : ceilKey)*60].
pub fn sample_weather(records: &[WeatherRecord], time_minutes: u32) -> Option<WeatherRecord> {
    if records.is_empty() {
        return None;
    }
    if records.len() == 1 {
        return Some(records[0]);
    }
    let t = time_minutes % 1440;
    let cur_hour = (t / 60) % 24;

    let hour_key = |r: &WeatherRecord| (r.time_minutes / 60) % 24;

    let upper_idx = records
        .iter()
        .position(|r| hour_key(r) > cur_hour)
        .unwrap_or(0);
    let lower_idx = if upper_idx == 0 {
        records.len() - 1
    } else {
        upper_idx - 1
    };
    let lower = &records[lower_idx];
    let upper = &records[upper_idx];

    let lower_hour = hour_key(lower);
    let upper_hour = hour_key(upper);

    let lower_t = (lower_hour * 60) as i32;
    let upper_t = if upper_hour > lower_hour {
        (upper_hour * 60) as i32
    } else {
        let ceil_hour = if upper_hour == 0 { 24 } else { upper_hour };
        (ceil_hour * 60) as i32 + if ceil_hour == 24 { 0 } else { 1440 }
    };
    let now_t = if (t as i32) >= lower_t {
        t as i32
    } else {
        t as i32 + 1440
    };
    let span = (upper_t - lower_t).max(1) as f32;
    let alpha = ((now_t - lower_t) as f32 / span).clamp(0.0, 1.0);

    Some(lerp_records(lower, upper, alpha, time_minutes))
}

fn lerp_records(a: &WeatherRecord, b: &WeatherRecord, t: f32, time_minutes: u32) -> WeatherRecord {
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let lerp4 = |x: [f32; 4], y: [f32; 4]| {
        [
            lerp(x[0], y[0]),
            lerp(x[1], y[1]),
            lerp(x[2], y[2]),
            lerp(x[3], y[3]),
        ]
    };
    let mut sk_c = [[0.0; 4]; 8];
    let mut sk_a = [0.0; 8];
    for i in 0..8 {
        sk_c[i] = lerp4(a.skybox_colors[i], b.skybox_colors[i]);
        sk_a[i] = lerp(a.skybox_altitudes[i], b.skybox_altitudes[i]);
    }
    WeatherRecord {
        time_minutes,
        indoors: a.indoors,
        sunlight_diffuse_entity: lerp4(a.sunlight_diffuse_entity, b.sunlight_diffuse_entity),
        moonlight_diffuse_entity: lerp4(a.moonlight_diffuse_entity, b.moonlight_diffuse_entity),
        // Static per-zone direction, not a time-varying color: no interpolation.
        indoor_light_dir_entity: a.indoor_light_dir_entity,
        ambient_entity: lerp4(a.ambient_entity, b.ambient_entity),
        fog_entity: lerp4(a.fog_entity, b.fog_entity),
        max_fog_dist_entity: lerp(a.max_fog_dist_entity, b.max_fog_dist_entity),
        min_fog_dist_entity: lerp(a.min_fog_dist_entity, b.min_fog_dist_entity),
        diffuse_mul_entity: lerp(a.diffuse_mul_entity, b.diffuse_mul_entity),

        sunlight_diffuse_landscape: lerp4(
            a.sunlight_diffuse_landscape,
            b.sunlight_diffuse_landscape,
        ),
        moonlight_diffuse_landscape: lerp4(
            a.moonlight_diffuse_landscape,
            b.moonlight_diffuse_landscape,
        ),
        indoor_light_dir_landscape: a.indoor_light_dir_landscape,
        ambient_landscape: lerp4(a.ambient_landscape, b.ambient_landscape),
        fog_landscape: lerp4(a.fog_landscape, b.fog_landscape),
        max_fog_dist_landscape: lerp(a.max_fog_dist_landscape, b.max_fog_dist_landscape),
        min_fog_dist_landscape: lerp(a.min_fog_dist_landscape, b.min_fog_dist_landscape),
        diffuse_mul_landscape: lerp(a.diffuse_mul_landscape, b.diffuse_mul_landscape),

        fog_offset: lerp(a.fog_offset, b.fog_offset),
        max_far_clip: lerp(a.max_far_clip, b.max_far_clip),
        background_color: lerp4(a.background_color, b.background_color),
        skybox_colors: sk_c,
        skybox_altitudes: sk_a,
    }
}

// ---------------------------------------------------------------------------
// 0x72 GETWEATHER: the global weather forecast table
// ---------------------------------------------------------------------------

/// The 2160-day cycle 0x72 indexes the forecast by
/// (research/XiEvents/OpCodes/0x0072.md: `work[4] % 2160`).
pub const FORECAST_DAYS: u32 = 2160;

/// The u32 offset past which the per-head forecast values begin in the data
/// DATs; 0x72 reads `WeatherData[6480 + head + 3*day]`
/// (research/XiEvents/OpCodes/0x0072.md).
pub const FORECAST_DATA_BASE: usize = 6480;

/// The three values 0x72 copies into `Work_Zone[2..5)` per day
/// (research/XiEvents/OpCodes/0x0072.md `PTR_Work_Zone[2..5)` loop).
pub const FORECAST_VALUES_PER_DAY: usize = 3;

/// The head-table entry counts: 7032 serves regions < 100 (100 entries), 7036
/// serves regions >= 100 (200 entries).
pub const FORECAST_HEADS_LOW: usize = 100;
pub const FORECAST_HEADS_HIGH: usize = 200;

/// The DAT file ids the forecast table ships in: 7032/7033 for regions < 100,
/// 7036/7037 for regions >= 100. The pairing is settled by value range: 7032's
/// head values top out at 37, which indexes 7033's 38 heads (61560 u32 =
/// 38 x 6480); 7036's top out at 51, indexing 7037's 52 heads (84240 u32 =
/// 52 x 6480). The PS2 pseudo-code's `WeatherHead`/`WeatherHead2` names are
/// swapped relative to the XIClient port (research/XIClient StringManager.cpp
/// loads 7032 into `WeatherHead`), so the pairing is taken from the measured
/// ranges, not the names.
pub const FORECAST_HEAD_LOW_FILE: u32 = 7032;
pub const FORECAST_DATA_LOW_FILE: u32 = 7033;
pub const FORECAST_HEAD_HIGH_FILE: u32 = 7036;
pub const FORECAST_DATA_HIGH_FILE: u32 = 7037;

/// The global weather forecast table 0x72 GETWEATHER reads: a per-region head
/// (which of the region group's heads the region uses) plus the per-head,
/// per-day forecast values. research/XiEvents/OpCodes/0x0072.md shows the index
/// arithmetic; the layout (head tables 7032/7036, data 7033/7037) is measured
/// from the shipped DATs. The three values are the raw u32s the opcode copies
/// into `Work_Zone[2..5)`; the script that authors 0x72 decides what they mean.
#[derive(Debug, Clone)]
pub struct WeatherForecast {
    head_low: [u8; FORECAST_HEADS_LOW],
    data_low: Vec<u32>,
    head_high: [u8; FORECAST_HEADS_HIGH],
    data_high: Vec<u32>,
}

impl WeatherForecast {
    /// Assemble a table from its four parts. The host builds this from
    /// [`load_weather_forecast`]; tests build it directly with synthetic cells.
    pub fn from_parts(
        head_low: [u8; FORECAST_HEADS_LOW],
        data_low: Vec<u32>,
        head_high: [u8; FORECAST_HEADS_HIGH],
        data_high: Vec<u32>,
    ) -> Self {
        Self {
            head_low,
            data_low,
            head_high,
            data_high,
        }
    }

    /// The three forecast values 0x72 writes into `Work_Zone[2..5)` for
    /// `region` on `day` of the 2160-day cycle
    /// (research/XiEvents/OpCodes/0x0072.md). `None` if the region or the
    /// derived index falls outside the shipped table.
    pub fn values(&self, region: u32, day: u32) -> Option<[u32; 3]> {
        let day = (day % FORECAST_DAYS) as usize;
        let (head_table, data) = if region < 100 {
            (&self.head_low[..], &self.data_low)
        } else {
            (&self.head_high[..], &self.data_high)
        };
        let head_idx = (if region < 100 { region } else { region - 100 }) as usize;
        let head = *head_table.get(head_idx)? as usize;
        let base = FORECAST_DATA_BASE + head + FORECAST_VALUES_PER_DAY * day;
        if base + FORECAST_VALUES_PER_DAY > data.len() {
            return None;
        }
        Some([data[base], data[base + 1], data[base + 2]])
    }
}

/// Load the forecast table from the install's four forecast DATs (7032/7033/
/// 7036/7037). The table is global — one copy serves every zone — so a host
/// loads it once and shares it across the event VMs it drives.
pub fn load_weather_forecast(root: &DatRoot) -> Result<WeatherForecast> {
    fn read_file(root: &DatRoot, file_id: u32) -> Result<Vec<u8>> {
        let loc = root.resolve(file_id)?;
        let path = loc.path_under(root);
        std::fs::read(&path).map_err(|e| DatError::Io {
            path: path.clone(),
            source: e,
        })
    }
    fn to_u32s(bytes: &[u8]) -> Vec<u32> {
        bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    let head_low = read_file(root, FORECAST_HEAD_LOW_FILE)?;
    let head_high = read_file(root, FORECAST_HEAD_HIGH_FILE)?;
    if head_low.len() < FORECAST_HEADS_LOW || head_high.len() < FORECAST_HEADS_HIGH {
        return Err(DatError::Weather(format!(
            "forecast head tables too small: {FORECAST_HEAD_LOW_FILE}={} bytes (need {}), {FORECAST_HEAD_HIGH_FILE}={} bytes (need {})",
            head_low.len(),
            FORECAST_HEADS_LOW,
            head_high.len(),
            FORECAST_HEADS_HIGH
        )));
    }
    let data_low = to_u32s(&read_file(root, FORECAST_DATA_LOW_FILE)?);
    let data_high = to_u32s(&read_file(root, FORECAST_DATA_HIGH_FILE)?);

    Ok(WeatherForecast {
        head_low: head_low[..FORECAST_HEADS_LOW].try_into().unwrap(),
        data_low,
        head_high: head_high[..FORECAST_HEADS_HIGH].try_into().unwrap(),
        data_high,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK_HEADER_LEN: usize = 16;
    const CHUNK_UNIT: usize = 16;
    const DIR_KIND: u8 = 0x01;
    const DIR_END_KIND: u8 = 0x00;

    fn chunk(name: &[u8; 4], kind: u8, body: &[u8]) -> Vec<u8> {
        let total = CHUNK_HEADER_LEN + body.len();
        assert_eq!(total % CHUNK_UNIT, 0);
        let mut out = name.to_vec();
        out.extend_from_slice(
            &((kind as u32) | (((total / CHUNK_UNIT) as u32) << 7)).to_le_bytes(),
        );
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(body);
        out
    }

    fn dir(name: &[u8; 4], children: Vec<u8>) -> Vec<u8> {
        let mut out = chunk(name, DIR_KIND, &[]);
        out.extend_from_slice(&children);
        out.extend_from_slice(&chunk(b"end ", DIR_END_KIND, &[]));
        out
    }

    fn weather_chunk(time: &[u8; 4], fog_r: u8) -> Vec<u8> {
        let mut body = [0u8; WEATHER_DATA_SIZE];
        body[56] = fog_r;
        chunk(time, ChunkKind::Weather as u8, &body)
    }

    fn zone_root_with_area() -> Vec<u8> {
        let zone_weat = dir(
            b"weat",
            dir(
                b"suny",
                [
                    weather_chunk(b"0000", 0x10),
                    dir(b"indo", weather_chunk(b"0000", 0x11)),
                ]
                .concat(),
            ),
        );
        let ev01 = dir(b"ev01", dir(b"suny", weather_chunk(b"0000", 0x20)));
        dir(b"t_sa", [zone_weat, ev01].concat())
    }

    #[test]
    fn area_container_beside_weat_becomes_its_own_environment() {
        let sets = collect_zone_weather_sets(&zone_root_with_area());
        let ev01 = crate::mzb::area_resource_id_from_dir_name(b"ev01");

        assert_eq!(
            sets.by_type[b"suny"].outdoor[0].fog_landscape[0],
            0x10 as f32 / 255.0
        );
        assert_eq!(
            sets.by_type[b"suny"].indoor[0].fog_landscape[0],
            0x11 as f32 / 255.0
        );
        assert_eq!(
            sets.by_area[&ev01][b"suny"].outdoor[0].fog_landscape[0],
            0x20 as f32 / 255.0
        );
    }

    #[test]
    fn weather_type_subdirs_do_not_register_as_areas() {
        // `weat/suny` is itself a directory of directories of 0x2F records (its
        // `indo` child), so only the `weat` sibling rule keeps it out of by_area.
        let sets = collect_zone_weather_sets(&zone_root_with_area());
        let suny = crate::mzb::area_resource_id_from_dir_name(b"suny");
        assert!(
            !sets.by_area.contains_key(&suny),
            "{:?}",
            sets.by_area.keys()
        );
        assert_eq!(sets.by_area.len(), 1);
    }

    #[test]
    fn unknown_and_zero_areas_fall_back_to_the_zone_environment() {
        // XiArea.cpp XiArea::FindAreaByFourCCAndGetFog: fourCC 0 short-circuits to the zone's own area, and
        // FindAreaByFourCC returns the zone on a miss — zones ship placements
        // naming areas with no container (`ent4`, `ex02`).
        let sets = collect_zone_weather_sets(&zone_root_with_area());
        let missing = crate::mzb::area_resource_id_from_dir_name(b"ent4");

        assert_eq!(
            sets.area_by_type(0)[b"suny"].outdoor[0].fog_landscape[0],
            0x10 as f32 / 255.0
        );
        assert_eq!(
            sets.area_by_type(missing)[b"suny"].outdoor[0].fog_landscape[0],
            0x10 as f32 / 255.0
        );
        assert_eq!(
            sets.area_by_type(crate::mzb::area_resource_id_from_dir_name(b"ev01"))[b"suny"].outdoor
                [0]
            .fog_landscape[0],
            0x20 as f32 / 255.0
        );
    }

    /// Southern San d'Oria: `t_sa/weat` plus the `ev01`/`ev02` interior areas its
    /// placements bind to. Pins the shipped layout the sibling rule reads, so a
    /// harvest that quietly stopped finding areas fails here and not only in a
    /// screenshot.
    #[test]
    fn real_zone_dat_area_environments_differ_from_the_zone_when_install_present() {
        const SOUTHERN_SAN_DORIA: u16 = 230;
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let file_id =
            crate::zone_dat::effective_zone_dat_file_id(Some(SOUTHERN_SAN_DORIA), None).unwrap();
        let loc = root.resolve(file_id).unwrap();
        let bytes = std::fs::read(loc.path_under(&root)).unwrap();
        let sets = collect_zone_weather_sets(&bytes);

        let zone_fog = sets.by_type[b"suny"].outdoor[0].fog_landscape;
        for area in [b"ev01", b"ev02"] {
            let id = crate::mzb::area_resource_id_from_dir_name(area);
            let by_type = sets.by_area.get(&id).unwrap_or_else(|| {
                panic!(
                    "zone {SOUTHERN_SAN_DORIA} lost area {:?}",
                    area.escape_ascii()
                )
            });
            assert_ne!(
                by_type[b"suny"].outdoor[0].fog_landscape,
                zone_fog,
                "area {:?} collapsed onto the zone environment",
                area.escape_ascii()
            );
        }
    }

    #[test]
    fn time_name_parses_hhmm_to_minutes() {
        assert_eq!(parse_time_name(b"0000").unwrap(), 0);
        assert_eq!(parse_time_name(b"0600").unwrap(), 360);
        assert_eq!(parse_time_name(b"1230").unwrap(), 750);
        assert_eq!(parse_time_name(b"2359").unwrap(), 23 * 60 + 59);
    }

    #[test]
    fn time_name_rejects_bad_input() {
        assert!(parse_time_name(b"abcd").is_err());
        assert!(parse_time_name(b"2400").is_err());
        assert!(parse_time_name(b"1260").is_err());
    }

    #[test]
    fn weather_record_round_trips_through_a_synthetic_payload() {
        let mut body = [0u8; WEATHER_DATA_SIZE];

        body[12] = 0xFF;
        body[13] = 0xC8;
        body[14] = 0x64;
        body[15] = 0x80;

        body[28..32].copy_from_slice(&100.0f32.to_le_bytes());

        body[36..40].copy_from_slice(&1.0f32.to_le_bytes());
        body[68..72].copy_from_slice(&0.75f32.to_le_bytes());

        body[76] = 0x11;
        body[77] = 0x22;
        body[78] = 0x33;

        body[108] = 0xFF;
        body[109] = 0x10;
        body[110] = 0x20;
        body[111] = 0x40;

        body[152..156].copy_from_slice(&0.5f32.to_le_bytes());

        let rec = parse_weather_record(b"1200", &body).unwrap();
        assert_eq!(rec.time_minutes, 720);
        // sunlight bytes FF,C8,64 * mul 1.0: FF/255=1.0 (>=0.8 so no bias on r);
        // since not all channels < 0.8, no bias applied to any channel.
        assert_eq!(rec.sunlight_diffuse_entity[0], 255.0 / 255.0);
        assert_eq!(rec.sunlight_diffuse_entity[1], 200.0 / 255.0);
        assert_eq!(rec.sunlight_diffuse_entity[2], 100.0 / 255.0);
        assert_eq!(rec.sunlight_diffuse_entity[3], 128.0 / 128.0);
        assert_eq!(rec.max_fog_dist_entity, 100.0);
        assert_eq!(rec.diffuse_mul_entity, 1.0);
        assert_eq!(rec.diffuse_mul_landscape, 0.75);
        assert_eq!(rec.skybox_colors[0][0], 1.0);
        assert_eq!(rec.skybox_altitudes[3], 0.5);
        assert_eq!(
            rec.background_color,
            [
                0x11 as f32 / 255.0,
                0x22 as f32 / 255.0,
                0x33 as f32 / 255.0,
                0.0
            ]
        );
    }

    // Retail-byte guard (skips without an install). The sky dome shader reads
    // these as polar-angle fractions spanning horizon -> zenith, so the domain is
    // load-bearing: if a shipped record ever went negative or non-monotonic, the
    // `asin` lookup in skybox.wgsl would bracket against garbage. Measured across
    // the shipped zone DATs: every non-empty record pins ring 0 at exactly 0.0 and
    // ring 7 at exactly 1.0, with no violations.
    #[test]
    fn real_dat_skybox_altitudes_span_zero_to_one_monotonically() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let mut records = 0u32;
        for &(zone, file_id) in crate::zone_dat::ZONE_DAT_TABLE {
            let Ok(loc) = root.resolve(file_id) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
                continue;
            };
            for set in collect_zone_weather_sets(&bytes).by_type.values() {
                for rec in set.outdoor.iter().chain(set.indoor.iter()) {
                    let a = rec.skybox_altitudes;
                    if a.iter().all(|v| *v == 0.0) {
                        continue;
                    }
                    records += 1;
                    assert_eq!(a[0], 0.0, "zone {zone} ring 0 is the horizon");
                    assert_eq!(a[7], 1.0, "zone {zone} ring 7 is the zenith");
                    assert!(
                        a.iter().all(|v| (0.0..=1.0).contains(v)),
                        "zone {zone} elevation outside [0,1]: {a:?}"
                    );
                    assert!(
                        a.windows(2).all(|w| w[1] >= w[0]),
                        "zone {zone} elevations not monotonic: {a:?}"
                    );
                }
            }
        }
        assert!(records > 1000, "expected a real corpus, saw {records}");
    }

    #[test]
    fn diffuse_color_applies_bias_only_when_all_channels_below_threshold() {
        // All channels well below 0xCC/0xFF=0.8 after mul => bias applied.
        let c = 0x00_40_40_40; // r=g=b=0x40 (64), a=0
        let out = diffuse_to_color(c, 1.0);
        let base = 64.0f32 / 255.0;
        assert!((out[0] - (base * 1.4).min(1.0)).abs() < 1e-6);
        assert!((out[1] - (base * 1.36).min(1.0)).abs() < 1e-6);
        assert!((out[2] - (base * 1.45).min(1.0)).abs() < 1e-6);

        // One channel >= 0.8 => no bias on any channel.
        let bright = 0x00_20_20_E0; // b=0xE0 (224)/255 ~0.88 >= 0.8
        let out2 = diffuse_to_color(bright, 1.0);
        assert!((out2[0] - 0xE0 as f32 / 255.0).abs() < 1e-6);
        assert!((out2[1] - 0x20 as f32 / 255.0).abs() < 1e-6);
        assert!((out2[2] - 0x20 as f32 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn ambient_color_biases_on_raw_byte_and_ceilings_at_half() {
        // Low raw bytes (<0xCC) => bias, then bias*byte/510.
        let c = 0x00_40_40_40;
        let out = ambient_to_color(c);
        assert!((out[0] - 1.4 * 64.0 / 510.0).abs() < 1e-6);
        assert!((out[1] - 1.36 * 64.0 / 510.0).abs() < 1e-6);
        assert!((out[2] - 1.45 * 64.0 / 510.0).abs() < 1e-6);

        // A channel >= 0xCC (204) => no bias; and the 0.5 ceiling caps high values.
        let hi = 0x00_FF_FF_FF; // all 255 >= 204, no bias; 255/510=0.5 exactly
        let out2 = ambient_to_color(hi);
        assert!((out2[0] - 0.5).abs() < 1e-6);
        assert!((out2[1] - 0.5).abs() < 1e-6);
        assert!((out2[2] - 0.5).abs() < 1e-6);
    }

    // research/xim EnvironmentSection.kt getDirectionOfIndoorDiffuseLight: signed byte components / 128,
    // normalized, negated; zero input is the "no indoor light" sentinel.
    #[test]
    fn indoor_direction_reads_signed_bytes() {
        // r=0, g=0x80 (-128 -> -1.0), b=0: straight "down" in signed space,
        // negated to the +y to-light vector (FFXI Y-down: light from below-ground
        // bounce is -y; +y here means the light points from below the horizon up).
        let d = indoor_light_direction(0x00_00_80_00);
        assert!((d[0]).abs() < 1e-6 && (d[1] - 1.0).abs() < 1e-6 && (d[2]).abs() < 1e-6);

        // r=0x7F (+127/128), g=b=0 -> unit -x after negation.
        let d = indoor_light_direction(0x00_00_00_7F);
        assert!((d[0] + 1.0).abs() < 1e-6);

        assert_eq!(indoor_light_direction(0), [0.0, 0.0, 0.0]);

        // Normalized regardless of magnitude.
        let d = indoor_light_direction(0x00_20_40_40);
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-5);
    }

    #[test]
    fn weather_record_rejects_short_payload() {
        let short = [0u8; WEATHER_DATA_SIZE - 1];
        let err = parse_weather_record(b"0000", &short).unwrap_err();
        assert!(matches!(err, DatError::Weather(_)));
    }

    fn mk_rec(time: u32, brightness: f32) -> WeatherRecord {
        WeatherRecord {
            time_minutes: time,
            indoors: false,
            sunlight_diffuse_entity: [0.0; 4],
            moonlight_diffuse_entity: [0.0; 4],
            indoor_light_dir_entity: [0.0; 3],
            ambient_entity: [0.0; 4],
            fog_entity: [0.0; 4],
            max_fog_dist_entity: 0.0,
            min_fog_dist_entity: 0.0,
            diffuse_mul_entity: brightness,
            sunlight_diffuse_landscape: [0.0; 4],
            moonlight_diffuse_landscape: [0.0; 4],
            indoor_light_dir_landscape: [0.0; 3],
            ambient_landscape: [0.0; 4],
            fog_landscape: [0.0; 4],
            max_fog_dist_landscape: 0.0,
            min_fog_dist_landscape: 0.0,
            diffuse_mul_landscape: brightness,
            fog_offset: 0.0,
            max_far_clip: 0.0,
            background_color: [0.0; 4],
            skybox_colors: [[0.0; 4]; 8],
            skybox_altitudes: [0.0; 8],
        }
    }

    #[test]
    fn sample_lerps_between_two_keyframes() {
        let records = vec![mk_rec(360, 0.0), mk_rec(720, 1.0)];

        let r = sample_weather(&records, 540).unwrap();
        assert!((r.diffuse_mul_entity - 0.5).abs() < 1e-5);
    }

    #[test]
    fn sample_wraps_across_day_boundary() {
        let records = vec![mk_rec(120, 1.0), mk_rec(1320, 0.0)];
        let r = sample_weather(&records, 0).unwrap();
        assert!(
            (r.diffuse_mul_entity - 0.5).abs() < 1e-5,
            "wrap midpoint got brightness {}",
            r.diffuse_mul_entity
        );
    }

    #[test]
    fn sample_returns_none_on_empty() {
        let records: Vec<WeatherRecord> = vec![];
        assert!(sample_weather(&records, 720).is_none());
    }

    fn synth_chunk(name: &[u8; 4], kind: u8, body: &[u8]) -> Vec<u8> {
        let total = 16 + body.len();
        let padded_total = total.div_ceil(16) * 16;
        let pad = padded_total - total;
        let size_units = (padded_total / 16) as u32;
        let value = (size_units << 7) | (kind as u32 & crate::chunk::CHUNK_KIND_MASK);

        let mut out = Vec::with_capacity(padded_total);
        out.extend_from_slice(name);
        out.extend_from_slice(&value.to_le_bytes());
        out.extend(std::iter::repeat_n(0u8, 8));
        out.extend_from_slice(body);
        out.extend(std::iter::repeat_n(0u8, pad));
        out
    }

    fn weather_body(skybox0_r: u8) -> [u8; WEATHER_DATA_SIZE] {
        let mut body = [0u8; WEATHER_DATA_SIZE];
        body[36..40].copy_from_slice(&1.0f32.to_le_bytes());
        body[68..72].copy_from_slice(&1.0f32.to_le_bytes());
        body[108] = skybox0_r;
        body
    }

    fn dir_open(name: &[u8; 4]) -> Vec<u8> {
        synth_chunk(name, 0x01, &[0u8; 16])
    }

    fn dir_close() -> Vec<u8> {
        synth_chunk(b"end\0", 0x00, &[0u8; 16])
    }

    // Mirrors the real f_ro tree: weat/{clod,suny}/ each with its own per-hour
    // 0x2F set and an indo/ indoor subdir.
    #[test]
    fn weather_sets_group_by_type_and_indoor() {
        let mut buf = Vec::new();
        buf.extend(dir_open(b"weat"));
        {
            buf.extend(dir_open(b"clod"));
            buf.extend(synth_chunk(b"0000", 0x2F, &weather_body(0x10)));
            buf.extend(synth_chunk(b"1200", 0x2F, &weather_body(0x11)));
            {
                buf.extend(dir_open(b"indo"));
                buf.extend(synth_chunk(b"0600", 0x2F, &weather_body(0x12)));
                buf.extend(dir_close());
            }
            buf.extend(dir_close());

            buf.extend(dir_open(b"suny"));
            buf.extend(synth_chunk(b"0600", 0x2F, &weather_body(0x20)));
            buf.extend(dir_close());
        }
        buf.extend(dir_close());

        let sets = collect_zone_weather_sets(&buf);
        assert!(
            sets.flat.is_empty(),
            "weat subtree present => no flat fallback"
        );
        assert_eq!(sets.by_type.len(), 2);

        let clod = sets.by_type.get(b"clod").expect("clod set");
        assert_eq!(clod.outdoor.len(), 2);
        assert_eq!(clod.outdoor[0].time_minutes, 0);
        assert_eq!(clod.outdoor[1].time_minutes, 720);
        assert_eq!(clod.indoor.len(), 1);
        assert_eq!(clod.indoor[0].time_minutes, 360);

        let suny = sets.by_type.get(b"suny").expect("suny set");
        assert_eq!(suny.outdoor.len(), 1);
        assert!(suny.indoor.is_empty());
    }

    // No cross-type dedup: a 0x2F record at the same hour exists independently in
    // each weather type's set (the old flat dedup_by_key collapsed these).
    #[test]
    fn weather_sets_do_not_dedup_across_types() {
        let mut buf = Vec::new();
        buf.extend(dir_open(b"weat"));
        buf.extend(dir_open(b"clod"));
        buf.extend(synth_chunk(b"1200", 0x2F, &weather_body(0x10)));
        buf.extend(dir_close());
        buf.extend(dir_open(b"suny"));
        buf.extend(synth_chunk(b"1200", 0x2F, &weather_body(0x20)));
        buf.extend(dir_close());
        buf.extend(dir_close());

        let sets = collect_zone_weather_sets(&buf);
        assert_eq!(sets.by_type.get(b"clod").unwrap().outdoor.len(), 1);
        assert_eq!(sets.by_type.get(b"suny").unwrap().outdoor.len(), 1);
        assert_ne!(
            sets.by_type.get(b"clod").unwrap().outdoor[0].skybox_colors[0][0],
            sets.by_type.get(b"suny").unwrap().outdoor[0].skybox_colors[0][0],
        );
    }

    // Pins the weather.h id ordering -> weat subdir rows.
    #[test]
    fn weather_type_id_maps_lsb_ids_to_subdirs() {
        // Every row of XIClient WeatherCondition.cpp `WeatherTable1`, in
        // DAT byte order. All 20 are distinct: retail authors one `weat/<tag>`
        // subtree per weather id, it does not collapse them onto a smaller set.
        const EXPECTED: [WeatherTypeId; 20] = [
            *b"fine", *b"suny", *b"clod", *b"mist", *b"dryw", *b"heat", *b"rain", *b"squl",
            *b"dust", *b"sand", *b"wind", *b"stom", *b"snow", *b"bliz", *b"thdr", *b"bolt",
            *b"aura", *b"ligt", *b"fogd", *b"dark",
        ];
        for (id, want) in EXPECTED.iter().enumerate() {
            assert_eq!(weather_type_id(id as u16), *want, "weather id {id}");
        }
        let distinct: std::collections::BTreeSet<_> = EXPECTED.iter().collect();
        assert_eq!(distinct.len(), EXPECTED.len());
    }

    // `None` is "no weather received", NOT weather id 0. Those were the same tag
    // only while the table collapsed ids 0 and 1; retail row 0 is `fine`, so an
    // unknown-marker of 0 silently starts asking for a container 72 of the 298
    // weat-bearing zones do not ship.
    #[test]
    fn unknown_weather_resolves_to_the_fallback_not_row_zero() {
        assert_eq!(weather_type_id_or_default(None), WEATHER_TYPE_FALLBACK);
        assert_eq!(weather_type_id_or_default(Some(0)), *b"fine");
        assert_ne!(weather_type_id_or_default(None), weather_type_id(0));
        assert_eq!(weather_type_id_or_default(Some(6)), *b"rain");
    }

    #[test]
    fn weather_type_id_falls_back_to_suny_out_of_range() {
        assert_eq!(weather_type_id(20), WEATHER_TYPE_FALLBACK);
        assert_eq!(weather_type_id(255), WEATHER_TYPE_FALLBACK);
        assert_eq!(WEATHER_TYPE_FALLBACK, *b"suny");
    }

    fn cue(time_minutes: u32, se_id: u32) -> AmbientCue {
        AmbientCue {
            time_minutes,
            se_id,
            loops: true,
        }
    }

    // WeatherTransition.cpp WeatherTransition::FindPrevSound picks the greatest activate time strictly earlier than
    // now and otherwise the LAST candidate enumerated, which is what makes a 06:00/18:00
    // pair wrap: before dawn the night bed is the trailing key, not the nearest one.
    #[test]
    fn ambient_selection_takes_the_greatest_earlier_key_else_the_last() {
        let day_night = [cue(360, 1005), cue(1080, 1007)];
        assert_eq!(select_ambient(&day_night, 359).unwrap().se_id, 1007);
        assert_eq!(select_ambient(&day_night, 360).unwrap().se_id, 1005);
        assert_eq!(select_ambient(&day_night, 1079).unwrap().se_id, 1005);
        assert_eq!(select_ambient(&day_night, 1080).unwrap().se_id, 1007);
        assert_eq!(select_ambient(&day_night, 1439).unwrap().se_id, 1007);

        // f_hi's four-key twilight set: 04:30 and 16:30 share one bed.
        let twilight = [cue(270, 10), cue(450, 11), cue(990, 12), cue(1170, 13)];
        assert_eq!(select_ambient(&twilight, 300).unwrap().se_id, 10);
        assert_eq!(select_ambient(&twilight, 1020).unwrap().se_id, 12);

        assert!(select_ambient(&[], 720).is_none());
    }

    // Retail's fallback is "last enumerated", so DAT order is load-bearing and the cue
    // lists must not be sorted the way the 0x2F record sets are.
    #[test]
    fn ambient_fallback_follows_dat_order_not_key_order() {
        let reversed = [cue(1080, 1007), cue(360, 1005)];
        assert_eq!(select_ambient(&reversed, 0).unwrap().se_id, 1005);
    }

    fn ambient_zone_dat(file_id: u32) -> Option<ZoneWeatherSets> {
        let root = crate::archive::open_test_install()?;
        let loc = root.resolve(file_id).ok()?;
        let bytes = std::fs::read(loc.path_under(&root)).ok()?;
        Some(collect_zone_weather_sets(&bytes))
    }

    // Measured beds: West Ronfaure's day/night pair plus its indoor bed, Southern San
    // d'Oria's wind pair, and Ru'Lude Gardens' thunder bed. The `thnd` Sep sitting beside
    // that last one is a sound generator's payload (`set1`/`set2` at [-120,-20,120] and
    // [100,-30,-140]) and must NOT enter the bed list.
    #[test]
    fn real_dat_ambient_beds_match_the_shipped_keys() {
        const WEST_RONFAURE: u32 = 200;
        const SOUTHERN_SAN_DORIA: u32 = 230;
        const RULUDE_GARDENS: u32 = 101;

        let Some(f_ro) = ambient_zone_dat(WEST_RONFAURE) else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let suny = &f_ro.by_type[b"suny"];
        assert_eq!(
            suny.outdoor_ambient,
            vec![cue(360, 1005), cue(1080, 1007)],
            "f_ro weat/suny beds"
        );
        assert_eq!(suny.indoor_ambient, vec![cue(0, 1056)]);
        assert_eq!(suny.ambient(true), suny.indoor_ambient);
        assert_eq!(suny.ambient(false), suny.outdoor_ambient);

        let f_tu = ambient_zone_dat(SOUTHERN_SAN_DORIA).unwrap();
        assert_eq!(
            f_tu.by_type[b"wind"].outdoor_ambient,
            vec![cue(360, 1037), cue(1080, 1039)]
        );

        let s_ju = ambient_zone_dat(RULUDE_GARDENS).unwrap();
        let thdr = &s_ju.by_type[b"thdr"];
        assert_eq!(thdr.outdoor_ambient, vec![cue(0, 1096)]);
        assert!(
            !thdr.outdoor_ambient.iter().any(|c| c.se_id == 2020),
            "`thnd` is a generator payload, not a bed: {:?}",
            thdr.outdoor_ambient
        );
    }

    // Census guard: an over- or under-collecting filter is silent otherwise. Measured over
    // every shipped zone DAT — 1,976 outdoor and 738 indoor candidates, none of them
    // parented by `extr`, and no weather-type dir carrying more than four outdoor beds.
    #[test]
    fn real_dat_ambient_census_matches_the_measured_corpus() {
        const MIN_OUTDOOR: usize = 1900;
        const MIN_INDOOR: usize = 700;
        const MAX_BEDS_PER_DIR: usize = 4;

        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let mut seen = std::collections::HashSet::new();
        let (mut outdoor, mut indoor) = (0usize, 0usize);
        for &(_zone, file_id) in crate::zone_dat::ZONE_DAT_TABLE {
            if !seen.insert(file_id) {
                continue;
            }
            let Ok(loc) = root.resolve(file_id) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
                continue;
            };
            for (tag, set) in collect_zone_weather_sets(&bytes).by_type {
                assert!(
                    set.outdoor_ambient.len() <= MAX_BEDS_PER_DIR,
                    "DAT {file_id} weat/{} has {} beds",
                    String::from_utf8_lossy(&tag),
                    set.outdoor_ambient.len()
                );
                outdoor += set.outdoor_ambient.len();
                indoor += set.indoor_ambient.len();
            }
        }
        assert!(outdoor >= MIN_OUTDOOR, "outdoor beds: {outdoor}");
        assert!(indoor >= MIN_INDOOR, "indoor beds: {indoor}");
    }

    // `f_tu/fser/0111` is a footstep Sep whose name is a valid HHMM by coincidence. The
    // area-container arm treats every dir beside `weat` as a weather-type parent, so
    // without the weat-only gate it would register `fser` as an area full of beds.
    #[test]
    fn real_dat_seps_outside_weat_are_never_beds() {
        const SOUTHERN_SAN_DORIA: u32 = 230;
        let Some(sets) = ambient_zone_dat(SOUTHERN_SAN_DORIA) else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        for (area, by_type) in &sets.by_area {
            for (tag, set) in by_type {
                assert!(
                    set.outdoor_ambient.is_empty() && set.indoor_ambient.is_empty(),
                    "area {area:#x} tag {} collected beds outside weat/",
                    String::from_utf8_lossy(tag)
                );
            }
        }
    }

    #[test]
    fn weather_sets_fall_back_to_flat_without_weat_subtree() {
        let buf = synth_chunk(b"1200", 0x2F, &weather_body(0x10));
        let sets = collect_zone_weather_sets(&buf);
        assert!(sets.by_type.is_empty());
        assert_eq!(sets.flat.len(), 1);
    }

    fn synth_forecast() -> WeatherForecast {
        let head_low = std::array::from_fn(|i| i as u8);
        let head_high = std::array::from_fn(|i| i as u8);
        let mut data_low = vec![0u32; 14000];
        let mut data_high = vec![0u32; 14000];
        data_low[6515] = 1;
        data_low[6516] = 2;
        data_low[6517] = 3;
        data_high[6485] = 4;
        data_high[6486] = 5;
        data_high[6487] = 6;
        WeatherForecast {
            head_low,
            data_low,
            head_high,
            data_high,
        }
    }

    #[test]
    fn forecast_values_index_the_head_and_day() {
        let fc = synth_forecast();
        assert_eq!(fc.values(5, 10), Some([1, 2, 3]));
        assert_eq!(fc.values(105, 0), Some([4, 5, 6]));
        assert_eq!(fc.values(5, FORECAST_DAYS), fc.values(5, 0));
        assert_eq!(fc.values(0, 0), Some([0, 0, 0]));
    }

    #[test]
    fn forecast_values_out_of_range_region_is_none() {
        let fc = synth_forecast();
        assert_eq!(fc.values(300, 0), None);
    }

    #[test]
    fn real_forecast_reads_the_shipped_base_cell() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let fc = load_weather_forecast(&root).expect("forecast loads");
        assert_eq!(
            fc.values(0, 0),
            Some([0x01FF_FF01, 0xFF01_FFFF, 0xFFFF_01FF])
        );
    }
}
