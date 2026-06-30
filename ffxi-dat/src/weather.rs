use std::collections::HashMap;

use crate::{
    chunk::{self, ChunkNode},
    kind::ChunkKind,
    DatError, Result,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherRecord {
    pub time_minutes: u32,

    // research/xim EnvironmentSection.kt:275-277 indoorFlag@0 (==1 => indoors).
    pub indoors: bool,

    pub sunlight_diffuse_entity: [f32; 4],
    pub moonlight_diffuse_entity: [f32; 4],
    pub ambient_entity: [f32; 4],
    pub fog_entity: [f32; 4],
    pub max_fog_dist_entity: f32,
    pub min_fog_dist_entity: f32,
    // research/xim EnvironmentSection.kt:248 LightConfig.diffuseMultiplier (model block @36).
    pub diffuse_mul_entity: f32,

    pub sunlight_diffuse_landscape: [f32; 4],
    pub moonlight_diffuse_landscape: [f32; 4],
    pub ambient_landscape: [f32; 4],
    pub fog_landscape: [f32; 4],
    pub max_fog_dist_landscape: f32,
    pub min_fog_dist_landscape: f32,
    // research/xim EnvironmentSection.kt:248 LightConfig.diffuseMultiplier (terrain block @68).
    pub diffuse_mul_landscape: f32,

    pub fog_offset: f32,
    pub max_far_clip: f32,

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
        ambient_entity: ambient_to_color(u32_at(20)),
        fog_entity: u32_to_rgba(u32_at(24)),
        max_fog_dist_entity: f32_at(28),
        min_fog_dist_entity: f32_at(32),
        diffuse_mul_entity,

        sunlight_diffuse_landscape: diffuse_to_color(u32_at(44), diffuse_mul_landscape),
        moonlight_diffuse_landscape: diffuse_to_color(u32_at(48), diffuse_mul_landscape),
        ambient_landscape: ambient_to_color(u32_at(52)),
        fog_landscape: u32_to_rgba(u32_at(56)),
        max_fog_dist_landscape: f32_at(60),
        min_fog_dist_landscape: f32_at(64),
        diffuse_mul_landscape,

        fog_offset: f32_at(80),
        max_far_clip: f32_at(88),

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
    let r = (c & 0xFF) as f32 / 255.0;
    let g = ((c >> 8) & 0xFF) as f32 / 255.0;
    let b = ((c >> 16) & 0xFF) as f32 / 255.0;
    let a = ((c >> 24) & 0xFF) as f32 / 128.0;
    [r, g, b, a]
}

// research/xim EnvironmentSection.kt:123-127,184-204.
const COLOR_BIAS: [f32; 3] = [1.4, 1.36, 1.45];
const BIAS_THRESHOLD_BYTE: u32 = 0xCC;
const BIAS_THRESHOLD_F: f32 = 0xCC as f32 / 0xFF as f32;

// research/xim EnvironmentSection.kt:184-193 diffuseToColor: byte/255*mul, then
// channel-wise colorBias iff every multiplied channel < 0xCC/0xFF, clamped [0,1]
// only when bias applied.
pub fn diffuse_to_color(byte_rgba: u32, mul: f32) -> [f32; 4] {
    let mut r = (byte_rgba & 0xFF) as f32 / 255.0 * mul;
    let mut g = ((byte_rgba >> 8) & 0xFF) as f32 / 255.0 * mul;
    let mut b = ((byte_rgba >> 16) & 0xFF) as f32 / 255.0 * mul;
    let a = ((byte_rgba >> 24) & 0xFF) as f32 / 128.0;

    let apply_bias = r < BIAS_THRESHOLD_F && g < BIAS_THRESHOLD_F && b < BIAS_THRESHOLD_F;
    if apply_bias {
        r = (r * COLOR_BIAS[0]).clamp(0.0, 1.0);
        g = (g * COLOR_BIAS[1]).clamp(0.0, 1.0);
        b = (b * COLOR_BIAS[2]).clamp(0.0, 1.0);
    }
    [r, g, b, a]
}

// research/xim EnvironmentSection.kt:195-204 ambientToColor: bias iff every RAW
// byte < 0xCC, channel = bias*byte/510, then upper-ceiling 0.5 (Color.clamp(0.5)
// == coerceIn(0,0.5), a max not a min).
pub fn ambient_to_color(byte_rgba: u32) -> [f32; 4] {
    let rb = byte_rgba & 0xFF;
    let gb = (byte_rgba >> 8) & 0xFF;
    let bb = (byte_rgba >> 16) & 0xFF;
    let ab = (byte_rgba >> 24) & 0xFF;

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

// LSB weather id ordering: vendor/server/src/map/enums/weather.h:24-46 (None=0 ..
// Darkness=19). The 4-char target is the `weat/<type>` subdir DatId; the five
// string constants come from research/xim DatResource.kt:110-114
// (suny/wind/rain/dust/fine), and clod/mist/thdr from the tree-weat.txt legend.
// XIM is a server reimpl with no numeric-id table, so these rows are authored
// from the id ordering above, not copied. Fallback for an out-of-range id is
// `suny` (research/xim EnvironmentManager.kt:271 firstOrNull{==suny} ?: first).
const WEATHER_TYPE_IDS: [WeatherTypeId; 20] = [
    *b"suny", // 0  None
    *b"suny", // 1  Sunshine
    *b"clod", // 2  Clouds
    *b"mist", // 3  Fog
    *b"suny", // 4  HotSpell
    *b"suny", // 5  HeatWave
    *b"rain", // 6  Rain
    *b"rain", // 7  Squall
    *b"dust", // 8  DustStorm
    *b"dust", // 9  SandStorm
    *b"wind", // 10 Wind
    *b"wind", // 11 Gales
    *b"snow", // 12 Snow
    *b"snow", // 13 Blizzards
    *b"thdr", // 14 Thunder
    *b"thdr", // 15 Thunderstorms
    *b"fine", // 16 Auroras
    *b"fine", // 17 StellarGlare
    *b"fine", // 18 Gloom
    *b"fine", // 19 Darkness
];

pub const WEATHER_TYPE_FALLBACK: WeatherTypeId = *b"suny";

// Map an LSB weather id (vendor/server/src/map/enums/weather.h ordering) onto the
// `weat/<type>` subdir DatId. wire::Weather shares this discriminant order, so the
// viewer passes `weather as u16` straight through.
pub fn weather_type_id(lsb_weather_id: u16) -> WeatherTypeId {
    *WEATHER_TYPE_IDS
        .get(lsb_weather_id as usize)
        .unwrap_or(&WEATHER_TYPE_FALLBACK)
}

#[derive(Debug, Clone, Default)]
pub struct WeatherSet {
    pub outdoor: Vec<WeatherRecord>,
    pub indoor: Vec<WeatherRecord>,
}

impl WeatherSet {
    pub fn is_empty(&self) -> bool {
        self.outdoor.is_empty() && self.indoor.is_empty()
    }
}

// research/xim EnvironmentManager.kt:509-515 getAreaEnvironmentDirectories keys
// the per-weather environment sets by the weather DatId subdirectory under the
// zone root's `weat` directory; each carries its own per-hour 0x2F record set and
// an `indo` indoor variant. We mirror that grouping here instead of the flat
// sort+dedup collapse that loses the weather-type/indoor distinction.
#[derive(Debug, Clone, Default)]
pub struct ZoneWeatherSets {
    pub by_type: HashMap<WeatherTypeId, WeatherSet>,

    // Flat fallback for zones with no `weat` subtree (records harvested by a
    // plain chunk walk + nonblack retain).
    pub flat: Vec<WeatherRecord>,
}

impl ZoneWeatherSets {
    pub fn is_empty(&self) -> bool {
        self.by_type.is_empty() && self.flat.is_empty()
    }
}

const WEAT_DIR: WeatherTypeId = *b"weat";
const INDO_DIR: WeatherTypeId = *b"indo";

pub fn collect_zone_weather_sets(dat_bytes: &[u8]) -> ZoneWeatherSets {
    let tree = chunk::walk_tree(dat_bytes);
    let mut by_type: HashMap<WeatherTypeId, WeatherSet> = HashMap::new();

    // The `weat` directory sits under the zone root dir (e.g. f_ro/weat), not at
    // the file's top level, so we search the whole dir tree for it. The separate
    // `ev01` event-environment subtree is intentionally skipped: those records
    // are event-scoped overrides, not the ambient per-weather sets we sample.
    find_weat_dirs(&tree, &mut by_type);

    for set in by_type.values_mut() {
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

    ZoneWeatherSets { by_type, flat }
}

fn find_weat_dirs(node: &ChunkNode, by_type: &mut HashMap<WeatherTypeId, WeatherSet>) {
    for child in &node.children {
        if child.chunk.kind != 0x01 {
            continue;
        }
        if child.chunk.name == WEAT_DIR {
            harvest_weat_dir(child, by_type);
        } else {
            find_weat_dirs(child, by_type);
        }
    }
}

fn harvest_weat_dir(weat: &ChunkNode, by_type: &mut HashMap<WeatherTypeId, WeatherSet>) {
    for type_node in &weat.children {
        if type_node.chunk.kind != 0x01 {
            continue;
        }
        let set = by_type.entry(type_node.chunk.name).or_default();
        push_weather_records(type_node, &mut set.outdoor);
        for child in &type_node.children {
            if child.chunk.kind == 0x01 && child.chunk.name == INDO_DIR {
                push_weather_records(child, &mut set.indoor);
            }
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

// research/xim EnvironmentManager.kt:425-438: env resources are keyed by whole
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
        ambient_landscape: lerp4(a.ambient_landscape, b.ambient_landscape),
        fog_landscape: lerp4(a.fog_landscape, b.fog_landscape),
        max_fog_dist_landscape: lerp(a.max_fog_dist_landscape, b.max_fog_dist_landscape),
        min_fog_dist_landscape: lerp(a.min_fog_dist_landscape, b.min_fog_dist_landscape),
        diffuse_mul_landscape: lerp(a.diffuse_mul_landscape, b.diffuse_mul_landscape),

        fog_offset: lerp(a.fog_offset, b.fog_offset),
        max_far_clip: lerp(a.max_far_clip, b.max_far_clip),
        skybox_colors: sk_c,
        skybox_altitudes: sk_a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(parse_time_name(b"2500").is_err());
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
            ambient_entity: [0.0; 4],
            fog_entity: [0.0; 4],
            max_fog_dist_entity: 0.0,
            min_fog_dist_entity: 0.0,
            diffuse_mul_entity: brightness,
            sunlight_diffuse_landscape: [0.0; 4],
            moonlight_diffuse_landscape: [0.0; 4],
            ambient_landscape: [0.0; 4],
            fog_landscape: [0.0; 4],
            max_fog_dist_landscape: 0.0,
            min_fog_dist_landscape: 0.0,
            diffuse_mul_landscape: brightness,
            fog_offset: 0.0,
            max_far_clip: 0.0,
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
        let value = (size_units << 7) | (kind as u32 & 0x7F);

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
        assert_eq!(weather_type_id(0), *b"suny"); // None
        assert_eq!(weather_type_id(1), *b"suny"); // Sunshine
        assert_eq!(weather_type_id(2), *b"clod"); // Clouds
        assert_eq!(weather_type_id(3), *b"mist"); // Fog
        assert_eq!(weather_type_id(6), *b"rain"); // Rain
        assert_eq!(weather_type_id(8), *b"dust"); // DustStorm
        assert_eq!(weather_type_id(10), *b"wind"); // Wind
        assert_eq!(weather_type_id(12), *b"snow"); // Snow
        assert_eq!(weather_type_id(14), *b"thdr"); // Thunder
        assert_eq!(weather_type_id(19), *b"fine"); // Darkness
    }

    #[test]
    fn weather_type_id_falls_back_to_suny_out_of_range() {
        assert_eq!(weather_type_id(20), WEATHER_TYPE_FALLBACK);
        assert_eq!(weather_type_id(255), WEATHER_TYPE_FALLBACK);
        assert_eq!(WEATHER_TYPE_FALLBACK, *b"suny");
    }

    #[test]
    fn weather_sets_fall_back_to_flat_without_weat_subtree() {
        let buf = synth_chunk(b"1200", 0x2F, &weather_body(0x10));
        let sets = collect_zone_weather_sets(&buf);
        assert!(sets.by_type.is_empty());
        assert_eq!(sets.flat.len(), 1);
    }
}
