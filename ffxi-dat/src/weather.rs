use crate::{chunk, kind::ChunkKind, DatError, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherRecord {
    pub time_minutes: u32,

    pub sunlight_diffuse_entity: [f32; 4],
    pub moonlight_diffuse_entity: [f32; 4],
    pub ambient_entity: [f32; 4],
    pub fog_entity: [f32; 4],
    pub max_fog_dist_entity: f32,
    pub min_fog_dist_entity: f32,
    pub brightness_entity: f32,

    pub sunlight_diffuse_landscape: [f32; 4],
    pub moonlight_diffuse_landscape: [f32; 4],
    pub ambient_landscape: [f32; 4],
    pub fog_landscape: [f32; 4],
    pub max_fog_dist_landscape: f32,
    pub min_fog_dist_landscape: f32,
    pub brightness_landscape: f32,

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

fn u32_to_rgba(c: u32) -> [f32; 4] {
    let r = (c & 0xFF) as f32 / 255.0;
    let g = ((c >> 8) & 0xFF) as f32 / 255.0;
    let b = ((c >> 16) & 0xFF) as f32 / 255.0;
    let a = ((c >> 24) & 0xFF) as f32 / 128.0;
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

pub fn sample_weather(records: &[WeatherRecord], time_minutes: u32) -> Option<WeatherRecord> {
    if records.is_empty() {
        return None;
    }
    if records.len() == 1 {
        return Some(records[0]);
    }
    let t = time_minutes % 1440;

    let upper_idx = records.iter().position(|r| r.time_minutes > t).unwrap_or(0);
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
        upper.time_minutes as i32 + 1440
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

        body[68..72].copy_from_slice(&0.75f32.to_le_bytes());

        body[108] = 0xFF;
        body[109] = 0x10;
        body[110] = 0x20;
        body[111] = 0x40;

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
        let records = vec![mk_rec(360, 0.0), mk_rec(720, 1.0)];

        let r = sample_weather(&records, 540).unwrap();
        assert!((r.brightness_entity - 0.5).abs() < 1e-5);
    }

    #[test]
    fn sample_wraps_across_day_boundary() {
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
