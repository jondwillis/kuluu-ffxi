//! FieldDebug resource + 120-tick ring buffer (plan §4 step 2).
//!
//! Dispatch records one tick of ramp-field data at each `walker::step` site;
//! the gizmo system draws it in-world (`stair_draw`) and the snapshot system
//! summarizes it into `kuluu_render::hud::stair_debug::StairDebugSnapshot` for
//! the status panel (`stair_debug`). All heights here are y-up (bevy) — the
//! frame `field.rs` works in; wire z grows down.

use bevy::prelude::*;
use kuluu_render::{
    dat_mzb::MzbCollisionGeometry, ffxi_to_bevy, hud::stair_debug::StairDebugSnapshot,
};
use kuluu_snapshot::Vec3 as WireVec3;

use super::consts::{LATERAL_OFFSET, LOOKAHEAD, LOOKBEHIND};
use super::field::{self, Field, SupportProbe};
use super::{StepResult, VerticalDecision, WalkMode};

/// Ring length: 120 ticks = 2 s at the production 60 Hz.
pub const RING_LEN: usize = 120;

// Manual Default: arrays longer than 32 have no `Default` impl on this
// toolchain, so the ring is built with from_fn.
#[derive(Resource)]
pub struct FieldDebug {
    /// The last recorded tick's ramp field (None until dispatch has run).
    pub field: Option<Field>,
    /// Support probe for the same tick.
    pub probe: Option<SupportProbe>,
    /// Feet height in y-up at record time; 0 until the first tick.
    pub feet_y: f32,
    /// The speed variable (yalms/s) that paced that tick's vertical move.
    pub speed_yps: f32,
    /// Walk mode after the last recorded tick's vertical pass.
    pub mode: WalkMode,
    /// Fall velocity (yalms/s, negative = down); 0 when grounded.
    pub vy: f32,
    /// Last two ticks' vertical decisions, oldest first (panel header).
    decisions: [Option<VerticalDecision>; 2],
    ring: [Option<(Option<f32>, Option<f32>, f32)>; RING_LEN], // (h0, target, y)

    head: usize,
    count: usize,
}

impl Default for FieldDebug {
    fn default() -> Self {
        Self {
            field: None,
            probe: None,
            feet_y: 0.0,
            speed_yps: 0.0,
            mode: WalkMode::default(),
            vy: 0.0,
            decisions: [None, None],
            ring: std::array::from_fn(|_| None),
            head: 0,
            count: 0,
        }
    }
}

impl FieldDebug {
    /// Push one tick's (h0, target, y) into the ring.
    pub fn record(&mut self, h0: Option<f32>, target: Option<f32>, y: f32) {
        self.ring[self.head] = Some((h0, target, y));
        self.head = (self.head + 1) % RING_LEN;
        if self.count < RING_LEN {
            self.count += 1;
        }
    }

    /// Ring contents oldest-first as (h0, target, y).
    pub fn history(&self) -> impl Iterator<Item = (Option<f32>, Option<f32>, f32)> + '_ {
        let start = if self.count == RING_LEN { self.head } else { 0 };
        (0..self.count).map(move |i| match &self.ring[(start + i) % RING_LEN] {
            Some((h0, t, y)) => (*h0, *t, *y),
            None => unreachable!("record fills every slot before advancing"),
        })
    }

    /// Sign flips of consecutive nonzero dy over the ring (the "zig" counter;
    /// step 4's live tests assert on this same number).
    pub fn reversals(&self) -> u32 {
        let ys: Vec<f32> = self.history().map(|(_, _, y)| y).collect();
        let mut flips = 0u32;
        let mut last_sign = 0i8;
        for w in ys.windows(2) {
            let dy = w[1] - w[0];
            if dy.abs() < 1e-6 {
                continue;
            }
            let s = if dy > 0.0 { 1 } else { -1 };
            if last_sign != 0 && s != last_sign {
                flips += 1;
            }
            last_sign = s;
        }
        flips
    }

    /// Max |second difference| of y over the ring (a straight merge reads ~0).
    pub fn max_d2y(&self) -> f32 {
        let ys: Vec<f32> = self.history().map(|(_, _, y)| y).collect();
        ys.windows(3)
            .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
            .fold(0.0f32, f32::max)
    }
}

/// Sample the field at a wire position and record it into the ring. Called from
/// dispatch at both `walker::step` sites so panel and gizmos show the same
/// tick's data that moved the player; `res` is that tick's StepResult (mode,
/// vy, decision).
pub fn record_tick(
    dbg: &mut FieldDebug,
    geom: &MzbCollisionGeometry,
    x_wire: f32,
    y_wire: f32,
    z_wire: f32,
    heading: u8,
    speed_yps: f32,
    res: &StepResult,
) {
    let p = ffxi_to_bevy(WireVec3 {
        x: x_wire,
        y: y_wire,
        z: z_wire,
    });
    let feet_xz = Vec2::new(p.x, p.z);
    let feet_y = p.y;
    // Facing in bevy xz: the wire forward (cos a, -sin a) maps to
    // (fwd_x, -fwd_y) under bevy.z = -wire.y.
    let (fwd_x, fwd_y) = crate::view_native::input::heading_to_forward(heading);
    let m = Vec2::new(fwd_x, -fwd_y);

    let sampler = |xz: Vec2, ceiling: f32| geom.ground_raycast(xz, ceiling);
    dbg.probe = Some(field::support_probe(&sampler, feet_xz, feet_y));
    dbg.field = Some(field::sample_field(&sampler, feet_xz, feet_y, m));
    dbg.feet_y = feet_y;
    dbg.speed_yps = speed_yps;
    let (h0, target) = match &dbg.field {
        Some(f) => (f.h0, field::field_target(f)),
        None => (None, None),
    };
    dbg.record(h0, target, feet_y);

    // Mode / vy / decision for the panel header.
    dbg.mode = res.mode;
    dbg.vy = match res.mode {
        WalkMode::Airborne { vy } => vy,
        _ => 0.0,
    };
    let prev = dbg.decisions[1];
    dbg.decisions[0] = prev;
    dbg.decisions[1] = Some(res.decision);
}

/// In-world ramp-field gizmos behind the `stair_draw` toggle. bevy_gizmos has
/// no world-anchored text (only z=0-plane `text_2d`, which would not track a
/// moving character), so the riser count and decisions live in the panel; this
/// draws geometry only.
pub fn draw_walker_field_gizmos(
    dbg: Res<FieldDebug>,
    panels: Res<kuluu_render::hud::HudPanels>,
    mut gizmos: Gizmos,
) {
    if !panels.stair_draw {
        return;
    }
    let Some(field) = &dbg.field else {
        return;
    };

    // Sample spheres at (xz, h_k), colored by status. A dead-band window dims
    // everything blue: the field carries no ramp and nothing here is a signal.
    for s in &field.samples {
        let pos = Vec3::new(s.xz.x, 0.0, s.xz.y);
        match s.status {
            field::SampleStatus::Valid => {
                if let Some(h) = s.filtered.or(s.raw) {
                    gizmos.sphere(
                        Isometry3d::from_translation(pos + Vec3::Y * (h + 0.05)),
                        0.05,
                        Color::srgb(1.0, 1.0, 1.0),
                    );
                }
            }
            field::SampleStatus::LipFiltered => {
                if let Some(h) = s.filtered.or(s.raw) {
                    gizmos.sphere(
                        Isometry3d::from_translation(pos + Vec3::Y * (h + 0.05)),
                        0.05,
                        Color::srgb(1.0, 0.9, 0.2),
                    );
                }
                // Thin line to the raw height: how much the filter moved it.
                if let (Some(raw), Some(filt)) = (s.raw, s.filtered) {
                    gizmos.line(
                        pos + Vec3::Y * raw,
                        pos + Vec3::Y * filt,
                        Color::srgb(1.0, 0.9, 0.2),
                    );
                }
            }
            field::SampleStatus::RejectDrop => {
                if let Some(h) = s.raw {
                    gizmos.sphere(
                        Isometry3d::from_translation(pos + Vec3::Y * (h + 0.05)),
                        0.06,
                        Color::srgb(1.0, 0.2, 0.2),
                    );
                }
            }
            field::SampleStatus::Miss => {
                // Red X at the feet height: no floor under this column.
                let c = Color::srgb(1.0, 0.2, 0.2);
                let a = pos + Vec3::Y * dbg.feet_y;
                gizmos.line(
                    a - Vec3::new(0.08, 0.08, 0.0),
                    a + Vec3::new(0.08, 0.08, 0.0),
                    c,
                );
                gizmos.line(
                    a - Vec3::new(0.08, -0.08, 0.0),
                    a + Vec3::new(0.08, -0.08, 0.0),
                    c,
                );
            }
            field::SampleStatus::WallAhead => {
                if let Some(h) = s.raw {
                    gizmos.sphere(
                        Isometry3d::from_translation(pos + Vec3::Y * (h + 0.05)),
                        0.06,
                        Color::srgb(1.0, 0.2, 1.0),
                    );
                }
            }
        }
    }

    // Envelope: a green segment along m from -LOOKBEHIND to +LOOKAHEAD at
    // target + g.x * d (the plane of gradient g touching the highest sample).
    if let Some(target) = field.target {
        let back = Vec3::new(
            field.feet_xz.x - field.m.x * LOOKBEHIND,
            target - field.g.x * LOOKBEHIND + 0.12,
            field.feet_xz.y - field.m.y * LOOKBEHIND,
        );
        let front = Vec3::new(
            field.feet_xz.x + field.m.x * LOOKAHEAD,
            target + field.g.x * LOOKAHEAD + 0.12,
            field.feet_xz.y + field.m.y * LOOKAHEAD,
        );
        gizmos.line(back, front, Color::srgb(0.3, 1.0, 0.4));
        // A small quad when the lateral gradient is non-zero: the plane tilts
        // off the move line by g.y * LATERAL_OFFSET at each end (outline only;
        // bevy_gizmos has no filled-quad primitive).
        if field.g.y.abs() > 1e-4 {
            let l = LATERAL_OFFSET;
            let perp = Vec2::new(-field.m.y, field.m.x);
            let corners = [(l, l), (l, -l), (-l, l), (-l, -l)].map(|(d, lat)| {
                let xz = field.feet_xz + field.m * d + perp * lat;
                Vec3::new(xz.x, target + field.g.x * d + field.g.y * lat + 0.12, xz.y)
            });
            for i in 0..4 {
                gizmos.line(corners[i], corners[(i + 1) % 4], Color::srgb(0.3, 1.0, 0.4));
            }
        }
    }

    // Support probes: five spheres at feet +- FOOT_RADIUS, green accepted /
    // red not; a short bar at h0.
    if let Some(probe) = &dbg.probe {
        for (i, xz) in field::support_positions(field.feet_xz)
            .into_iter()
            .enumerate()
        {
            let col = if probe.accepted[i] {
                Color::srgb(0.3, 1.0, 0.4)
            } else {
                Color::srgb(1.0, 0.2, 0.2)
            };
            gizmos.sphere(
                Isometry3d::from_translation(Vec3::new(xz.x, dbg.feet_y + 0.05, xz.y)),
                0.04,
                col,
            );
        }
        if let Some(h0) = probe.h0 {
            let c = Vec3::new(field.feet_xz.x, 0.0, field.feet_xz.y);
            gizmos.line(
                c + Vec3::Y * (h0 - 0.15),
                c + Vec3::Y * (h0 + 0.15),
                Color::srgb(1.0, 1.0, 1.0),
            );
        }
    }

    // Three bars at the feet xz: wire Y cyan, target green, h0 white. Coincident
    // within 1e-3 collapse to one bar (the dedup below).
    let mut bars: Vec<(f32, Color)> = vec![(dbg.feet_y, Color::srgb(0.2, 0.9, 1.0))];
    if let Some(t) = field.target {
        bars.push((t, Color::srgb(0.3, 1.0, 0.4)));
    }
    if let Some(h0) = field.h0 {
        bars.push((h0, Color::srgb(1.0, 1.0, 1.0)));
    }
    bars.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let c = Vec3::new(field.feet_xz.x, 0.0, field.feet_xz.y);
    for (i, &(h, col)) in bars.iter().enumerate() {
        if i > 0 && (h - bars[i - 1].0).abs() < 1e-3 {
            continue; // coincident with the bar just drawn
        }
        gizmos.line(c + Vec3::Y * (h - 0.2), c + Vec3::Y * (h + 0.2), col);
    }

    // Move direction arrow at the feet.
    let a = Vec3::new(field.feet_xz.x, dbg.feet_y + 0.1, field.feet_xz.y);
    gizmos.line(
        a,
        a + Vec3::new(field.m.x * 0.5, 0.0, field.m.y * 0.5),
        Color::srgb(0.2, 0.9, 1.0),
    );
}

/// Zone/DAT header cache for the snapshot system: resolve on effective-zone-key
/// change only (effective key = (zone_id, myroom model) so mog-house interiors
/// show the interior's DAT, matching `effective_zone_file_id`).
#[derive(Resource, Default)]
pub struct StairDebugZoneCache {
    last_key: Option<(Option<u16>, Option<u16>)>,
    zone_id: u16,
    zone_name: String,
    dat_path: String,
}

/// Summarize FieldDebug into the render-crate snapshot every frame. The panel
/// shows defaults until dispatch has recorded a tick.
pub fn update_stair_debug_snapshot_system(
    dbg: Res<FieldDebug>,
    panels: Res<kuluu_render::hud::HudPanels>,
    scene: Res<kuluu_render::snapshot::SceneState>,
    mut zone_cache: ResMut<StairDebugZoneCache>,
    mut snap: ResMut<StairDebugSnapshot>,
) {
    if !panels.stair_debug {
        return;
    }
    let snap_ref = &scene.snapshot;
    let key = (snap_ref.zone_id, snap_ref.myroom.map(|m| m.model));
    if zone_cache.last_key != Some(key) {
        zone_cache.last_key = Some(key);
        zone_cache.zone_id = snap_ref.zone_id.unwrap_or(0);
        zone_cache.zone_name = snap_ref
            .zone_id
            .and_then(kuluu_nav::zone_name)
            .unwrap_or("")
            .to_string();
        zone_cache.dat_path = match (
            kuluu_render::snapshot::effective_zone_file_id(snap_ref),
            ffxi_dat::DatRoot::from_env_or_default().ok(),
        ) {
            (Some(fid), Some(root)) => root
                .resolve(fid)
                .ok()
                .map(|loc| {
                    format!(
                        "{}/{}/{}.DAT",
                        loc.rom_dir, loc.sub_path.dir, loc.sub_path.file
                    )
                })
                .unwrap_or_default(),
            _ => String::new(),
        };
    }
    snap.zone_id = zone_cache.zone_id;
    snap.zone_name = zone_cache.zone_name.clone();
    snap.dat_path = zone_cache.dat_path.clone();
    snap.drawing_enabled = panels.stair_draw;

    let p = ffxi_to_bevy(snap_ref.self_pos.pos);
    snap.player_xz = Vec2::new(p.x, p.z);
    snap.player_y = p.y;

    let Some(field) = &dbg.field else {
        return;
    };
    snap.field = Some(kuluu_render::hud::stair_debug::FieldSnapshot {
        mode: format!("{:?}", dbg.mode),
        vy: dbg.vy,
        decisions: [
            dbg.decisions[0].map(|d| d.label()),
            dbg.decisions[1].map(|d| d.label()),
        ],
        h0: field.h0,
        target: field.target,
        g_along: field.g.x,
        g_lateral: field.g.y,
        riser_count: field.riser_count,
        range: field.range,
        poof: field.poof,
        grounded: dbg.probe.map(|pr| pr.grounded).unwrap_or(false),
        speed_yps: dbg.speed_yps,
        samples: field
            .samples
            .iter()
            .map(|s| kuluu_render::hud::stair_debug::SampleRow {
                along: s.along,
                lateral: s.lateral,
                raw: s.raw,
                filtered: s.filtered,
                status: format!("{:?}", s.status),
            })
            .collect(),
        history: dbg.history().collect(),
        reversals_120: dbg.reversals(),
        max_d2y_120: dbg.max_d2y(),
    });
}
