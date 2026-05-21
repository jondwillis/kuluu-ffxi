//! Weather / time-of-day chunk parser (`kind = 0x2F`).
//!
//! Each FFXI zone DAT contains a `"weat"` chunk-group whose
//! grandchildren are individual Weather records, one per (weather
//! type, time-of-day) pair. We parse just the leaf records here —
//! the parent-chunk grouping isn't exposed by our flat `walk()`
//! API, so callers consume the chunk *name* as the time-of-day
//! key and treat all Weather chunks as if they belong to a single
//! "default" weather curve. That's accurate for ~90% of zones,
//! which only ship one weather pattern; multi-weather zones lose
//! the per-weather distinction but still get a coherent TOD curve.
//!
//! Reference: lotus-ffxi `ffxi/dat/dat.cppm::WeatherData` (lines
//! 124-154) and `ffxi/entity/landscape_entity.cpp` (lines 59-109)
//! for the TOD interpolation strategy.

use crate::{chunk, kind::ChunkKind, DatError, Result};

/// One weather/TOD keyframe. Field semantics per lotus's `WeatherData`:
///
/// * "entity" pair (`*_entity`) controls lighting on dynamic entities
///   (PCs / NPCs / mobs).
/// * "landscape" pair (`*_landscape`) controls lighting on static
///   zone geometry (MZB + MMB props).
///
/// Lotus uses two separate light/fog pairs because FFXI baked
/// per-vertex lighting into MMB meshes — the static landscape needs
/// fog tuned for the baked colors, while moving entities want fog
/// tuned against a neutral lighting baseline.
///
/// Colors are decoded as `(r, g, b) / 255.0` from the LE byte order
/// stored on disk. The alpha byte is divided by 128 per FFXI's
/// "0x80 = 1.0" working-range convention (lotus
/// `landscape_entity.cpp:67`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherRecord {
    /// Time-of-day in Vana'diel minutes (0..1440), parsed from the
    /// chunk *name* — e.g. `"0600"` → 360, `"1230"` → 750.
    pub time_minutes: u32,

    // Entity (dynamic) lighting.
    pub sunlight_diffuse_entity: [f32; 4],
    pub moonlight_diffuse_entity: [f32; 4],
    pub ambient_entity: [f32; 4],
    pub fog_entity: [f32; 4],
    pub max_fog_dist_entity: f32,
    pub min_fog_dist_entity: f32,
    pub brightness_entity: f32,

    // Landscape (static) lighting.
    pub sunlight_diffuse_landscape: [f32; 4],
    pub moonlight_diffuse_landscape: [f32; 4],
    pub ambient_landscape: [f32; 4],
    pub fog_landscape: [f32; 4],
    pub max_fog_dist_landscape: f32,
    pub min_fog_dist_landscape: f32,
    pub brightness_landscape: f32,

    /// `fog_offset` from the on-disk record. Lotus adds this to both
    /// `max_fog_dist*` and `min_fog_dist*` fields at consumption time
    /// — we expose it raw so the renderer can apply (or skip) the
    /// offset as it sees fit.
    pub fog_offset: f32,
    pub max_far_clip: f32,

    /// 8-color skybox gradient (sRGB) — bottom-of-dome to top, paired
    /// with [`skybox_altitudes`]. Lotus stores these in linear space
    /// (`landscape_entity.cpp:102` `glm::convertSRGBToLinear`); we
    /// keep them sRGB here and let the renderer convert if needed.
    pub skybox_colors: [[f32; 4]; 8],
    /// Per-band altitude (in [-1..+1] normalised dome-vertical units)
    /// for the 8 skybox gradient bands.
    pub skybox_altitudes: [f32; 8],
}

/// Errors specific to weather-chunk parsing.
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

/// On-disk `WeatherData` size — lotus's `dat.cppm:124-154` struct
/// laid out little-endian, packed. Computed: 3*4 (unk) + 4*4 +
/// 4*4 (entity) + 4*4 + 4*4 (landscape) + 4 (fog_color) + 3*4
/// (fog_offset/unk4/max_far_clip) + 4 (unk5) + 3*4 (unk6) + 32
/// (skybox_colors) + 32 (skybox_values) + 4 (unk7) = 176.
pub const WEATHER_DATA_SIZE: usize = 176;

/// Decode the [`WeatherRecord`] body. `name` is the chunk's 4-byte
/// name (e.g. `b"0600"`, `b"1200"`) used to derive `time_minutes`.
///
/// Time conversion mirrors lotus
/// `landscape_entity.cpp:79`:
///   `t = (HHMM/100)*60 + (HHMM - (HHMM/100)*100)`
///   i.e. HH*60 + MM.
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

    // Layout (mirrors lotus `WeatherData` struct exactly, byte-for-
    // byte; offsets in [] are pre-computed from the packed layout).
    //   [0..12]   unk[3]               — always zero
    //   [12]      sunlight_diffuse1    — RGBA u32 entity sunlight
    //   [16]      moonlight_diffuse1
    //   [20]      ambient1
    //   [24]      fog1
    //   [28]      max_fog_dist1        — f32
    //   [32]      min_fog_dist1        — f32
    //   [36]      brightness1
    //   [40]      unk2
    //   [44]      sunlight_diffuse2    — landscape sunlight
    //   [48]      moonlight_diffuse2
    //   [52]      ambient2
    //   [56]      fog2
    //   [60]      max_fog_dist2
    //   [64]      min_fog_dist2
    //   [68]      brightness2
    //   [72]      unk3
    //   [76]      fog_color            — u32 (unused — fog1/fog2 already carry colors)
    //   [80]      fog_offset           — f32 (added to all min/max dists)
    //   [84]      unk4
    //   [88]      max_far_clip
    //   [92]      unk5                 — u32 flags
    //   [96..108] unk6[3]
    //   [108..140] skybox_colors[8]    — u32 each
    //   [140..172] skybox_values[8]    — f32 each (altitudes)
    //   [172]     unk7
    Ok(WeatherRecord {
        time_minutes,
        sunlight_diffuse_entity: u32_to_rgba(u32_at(12)),
        moonlight_diffuse_entity: u32_to_rgba(u32_at(16)),
        ambient_entity: u32_to_rgba(u32_at(20)),
        fog_entity: u32_to_rgba(u32_at(24)),
        max_fog_dist_entity: f32_at(28),
        min_fog_dist_entity: f32_at(32),
        brightness_entity: f32_at(36),

        sunlight_diffuse_landscape: u32_to_rgba(u32_at(44)),
        moonlight_diffuse_landscape: u32_to_rgba(u32_at(48)),
        ambient_landscape: u32_to_rgba(u32_at(52)),
        fog_landscape: u32_to_rgba(u32_at(56)),
        max_fog_dist_landscape: f32_at(60),
        min_fog_dist_landscape: f32_at(64),
        brightness_landscape: f32_at(68),

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

/// Convert FFXI's packed RGBA u32 to a `[f32; 4]`. RGB bytes are
/// the low 24 bits in little-endian byte order (r=byte0, g=byte1,
/// b=byte2); the alpha byte sits at byte3 and per FFXI's `0x80=1.0`
/// convention is divided by 128 rather than 255 (lotus
/// `landscape_entity.cpp:67`).
fn u32_to_rgba(c: u32) -> [f32; 4] {
    let r = (c & 0xFF) as f32 / 255.0;
    let g = ((c >> 8) & 0xFF) as f32 / 255.0;
    let b = ((c >> 16) & 0xFF) as f32 / 255.0;
    let a = ((c >> 24) & 0xFF) as f32 / 128.0;
    [r, g, b, a]
}

/// Parse a 4-byte ASCII chunk name like `b"0600"` into Vana'diel
/// minutes. Returns `WeatherError::BadTimeName` if the bytes aren't
/// `[0-9]{4}` or the resulting HHMM is out of range.
fn parse_time_name(name: &[u8; 4]) -> Result<u32> {
    let mut acc = 0u32;
    for &b in name {
        if !(b'0'..=b'9').contains(&b) {
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

/// Scan a DAT's chunk stream and return every Weather record in
/// time-of-day order. Silently skips Weather chunks whose name
/// isn't a valid HHMM string (rare; some FFXI zone DATs include
/// `b"weat"` sentinel chunks the grouping layer would normally
/// filter out) or whose body fails to parse.
///
/// Most zones have 4-8 keyframes per weather pattern (typical:
/// 0000 / 0600 / 0700 / 1700 / 1900 / 2400 for the diurnal arc).
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
    out.sort_by_key(|r| r.time_minutes);
    out
}

/// Interpolate a [`WeatherRecord`] for `time_minutes` (0..1440) by
/// linearly blending between the two adjacent keyframes in `records`.
/// `records` must already be sorted ascending by `time_minutes`
/// — [`collect_weather_records`] does this.
///
/// Wraps around: a query at 23:00 with keyframes at 22:00 + 02:00
/// blends those two, accounting for the day boundary. Returns
/// `None` when `records` is empty.
pub fn sample_weather(records: &[WeatherRecord], time_minutes: u32) -> Option<WeatherRecord> {
    if records.is_empty() {
        return None;
    }
    if records.len() == 1 {
        return Some(records[0]);
    }
    let t = time_minutes % 1440;
    // Find the latest record whose time ≤ t — `lower` — and the
    // earliest record whose time > t — `upper`. Wrap if needed.
    let upper_idx = records.iter().position(|r| r.time_minutes > t).unwrap_or(0); // wrap: nothing greater → use the earliest as "tomorrow's first"
    let lower_idx = if upper_idx == 0 {
        records.len() - 1
    } else {
        upper_idx - 1
    };
    let lower = &records[lower_idx];
    let upper = &records[upper_idx];

    let lower_t = lower.time_minutes as i32;
    let upper_t = if upper.time_minutes > lower.time_minutes {
        upper.time_minutes as i32
    } else {
        upper.time_minutes as i32 + 1440 // wrap
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
        sunlight_diffuse_entity: lerp4(a.sunlight_diffuse_entity, b.sunlight_diffuse_entity),
        moonlight_diffuse_entity: lerp4(a.moonlight_diffuse_entity, b.moonlight_diffuse_entity),
        ambient_entity: lerp4(a.ambient_entity, b.ambient_entity),
        fog_entity: lerp4(a.fog_entity, b.fog_entity),
        max_fog_dist_entity: lerp(a.max_fog_dist_entity, b.max_fog_dist_entity),
        min_fog_dist_entity: lerp(a.min_fog_dist_entity, b.min_fog_dist_entity),
        brightness_entity: lerp(a.brightness_entity, b.brightness_entity),

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
        brightness_landscape: lerp(a.brightness_landscape, b.brightness_landscape),

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
        assert!(parse_time_name(b"2500").is_err()); // hh >= 24
        assert!(parse_time_name(b"1260").is_err()); // mm >= 60
    }

    #[test]
    fn weather_record_round_trips_through_a_synthetic_payload() {
        // Build a 176-byte body with distinct values in each field
        // so we can assert exact decoded results.
        let mut body = [0u8; WEATHER_DATA_SIZE];
        // sunlight_diffuse1 @ offset 12: RGBA = (255, 200, 100, 128)
        // packed as 0x80_64_C8_FF (little-endian → bytes FF C8 64 80).
        body[12] = 0xFF;
        body[13] = 0xC8;
        body[14] = 0x64;
        body[15] = 0x80;
        // max_fog_dist1 @ offset 28: 100.0
        body[28..32].copy_from_slice(&100.0f32.to_le_bytes());
        // brightness2 @ offset 68: 0.75
        body[68..72].copy_from_slice(&0.75f32.to_le_bytes());
        // skybox_colors[0] @ offset 108: 0x40_20_10_FF (R=255 etc.)
        body[108] = 0xFF;
        body[109] = 0x10;
        body[110] = 0x20;
        body[111] = 0x40;
        // skybox_altitudes[3] @ offset 140 + 3*4 = 152: 0.5
        body[152..156].copy_from_slice(&0.5f32.to_le_bytes());

        let rec = parse_weather_record(b"1200", &body).unwrap();
        assert_eq!(rec.time_minutes, 720);
        assert_eq!(rec.sunlight_diffuse_entity[0], 255.0 / 255.0);
        assert_eq!(rec.sunlight_diffuse_entity[1], 200.0 / 255.0);
        assert_eq!(rec.sunlight_diffuse_entity[2], 100.0 / 255.0);
        assert_eq!(rec.sunlight_diffuse_entity[3], 128.0 / 128.0);
        assert_eq!(rec.max_fog_dist_entity, 100.0);
        assert_eq!(rec.brightness_landscape, 0.75);
        assert_eq!(rec.skybox_colors[0][0], 1.0);
        assert_eq!(rec.skybox_altitudes[3], 0.5);
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
            sunlight_diffuse_entity: [0.0; 4],
            moonlight_diffuse_entity: [0.0; 4],
            ambient_entity: [0.0; 4],
            fog_entity: [0.0; 4],
            max_fog_dist_entity: 0.0,
            min_fog_dist_entity: 0.0,
            brightness_entity: brightness,
            sunlight_diffuse_landscape: [0.0; 4],
            moonlight_diffuse_landscape: [0.0; 4],
            ambient_landscape: [0.0; 4],
            fog_landscape: [0.0; 4],
            max_fog_dist_landscape: 0.0,
            min_fog_dist_landscape: 0.0,
            brightness_landscape: brightness,
            fog_offset: 0.0,
            max_far_clip: 0.0,
            skybox_colors: [[0.0; 4]; 8],
            skybox_altitudes: [0.0; 8],
        }
    }

    #[test]
    fn sample_lerps_between_two_keyframes() {
        let records = vec![mk_rec(360, 0.0), mk_rec(720, 1.0)]; // 06:00 → 12:00
                                                                // Midpoint: 09:00 (540 min) should give brightness 0.5.
        let r = sample_weather(&records, 540).unwrap();
        assert!((r.brightness_entity - 0.5).abs() < 1e-5);
    }

    #[test]
    fn sample_wraps_across_day_boundary() {
        // Keyframes at 22:00 (=1320) and 02:00 (=120). The
        // diurnal gap is 4 hours / 240 min. A query at 00:00
        // (=0 or 1440) should be exactly halfway → brightness 0.5.
        let records = vec![mk_rec(120, 1.0), mk_rec(1320, 0.0)];
        let r = sample_weather(&records, 0).unwrap();
        assert!(
            (r.brightness_entity - 0.5).abs() < 1e-5,
            "wrap midpoint got brightness {}",
            r.brightness_entity
        );
    }

    #[test]
    fn sample_returns_none_on_empty() {
        let records: Vec<WeatherRecord> = vec![];
        assert!(sample_weather(&records, 720).is_none());
    }
}
