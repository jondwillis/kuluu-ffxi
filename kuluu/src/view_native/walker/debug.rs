//! Stair-debug producer: a [`FieldDebug`] ring plus three systems — enable
//! sync, in-world gizmos (`stair_draw`), and the per-frame snapshot for
//! `kuluu_render::hud::stair_debug` (`stair_debug`). Dispatch records each
//! walker tick at both `walker::step` sites; the columns sampled here are the
//! same answers today's vertical law reads (the record behind it:
//! `.agents/skills/retail-observe/references/2026-10-25-collision-query.md`).
//! All heights are y-up (bevy); wire z grows down.

use bevy::prelude::*;
use kuluu_render::{
    dat_mzb::MzbCollisionGeometry, ffxi_to_bevy, hud::stair_debug::ColumnRow,
    hud::stair_debug::StairDebugSnapshot,
};

use super::consts::{BODY_RADIUS, LOWER_CENTER_OFFSET, MIDDLE_CENTER_OFFSET, UPPER_CENTER_OFFSET};
use super::{HorizontalOutcome, StepResult, VerticalDecision, WalkMode};

/// Ring length: 120 ticks = 2 s at the production 60 Hz.
pub const RING_LEN: usize = 120;

/// Probe columns along the heading for the panel and gizmos (yalms forward
/// from the post-move feet; 0 is under the player). Half a yalm apart so one
/// step-up assist's worth sits inside column spacing, reaching 1.5 yalms —
/// past the tallest tread a walker may have to mount in one push.
pub const PROBE_COLUMN_YAMLS: [f32; 4] = [0.0, 0.5, 1.0, 1.5];

/// One sampled column along the heading, stored pre-formatted except for the
/// numeric fields so the panel can align them.
pub struct ProbeColumn {
    /// Bevy xz of the sample point (heights in `ground`/`hits` are y-up).
    pub xz: Vec2,
    /// Distance along the heading (yalms; 0 under the feet).
    pub along: f32,
    /// `ground_walkable` answer at these feet — what would hold support here.
    pub ground: Option<f32>,
    /// Every surface height crossing the column with its normal.y, sorted by
    /// |height - feet|; compact string for the panel.
    pub hits: String,
    /// Body-chain wall contacts (the same three centers `step` probes):
    /// distance, normal, face identity — slot, triangle id, sub-area shell,
    /// prop flag and vertex y-extent. Joined with " | ".
    pub walls: String,
}

/// Manual `Default`: the ring is built with `from_fn`, so a derived `Default`
/// would need the `Default` impl the long array lacks on this toolchain.
#[derive(Resource)]
pub struct FieldDebug {
    /// Columns along the heading from the last recorded tick.
    pub columns: Vec<ProbeColumn>,
    /// Feet height in y-up at record time; 0 until the first tick.
    pub feet_y: f32,
    /// The speed variable (yalms/s) that paced this tick's move.
    pub speed_yps: f32,
    /// Walk mode after the last recorded tick's vertical pass.
    pub mode: WalkMode,
    /// Support answer under the post-move feet; None airborne or unreached.
    pub h0: Option<f32>,
    /// Grounded per this tick's decision (see `grounded_of`).
    pub grounded: bool,
    /// The walker's step-up assist reach at record time (`Walker::step_assist_yamls`).
    pub assist: f32,
    /// What the tick published to `Walker::probe_measurement_y`.
    pub measurement_y: f32,
    /// Set while a stair panel or gizmos are showing; recording is skipped otherwise.
    pub enabled: bool,
    /// Last two ticks' vertical decisions, oldest first (panel header).
    decisions: [Option<VerticalDecision>; 2],
    /// Last two detected horizontal outcomes, oldest first. A no-input tick
    /// overwrites only while the newest slot is not already NoInput: idle
    /// ticks carry no new information, so they hold the last stop reason in
    /// place instead of erasing it.
    outcomes: [Option<HorizontalOutcome>; 2],
    /// Per-tick (h0, y) samples; `history` reads them oldest-first.
    ring: [Option<(Option<f32>, f32)>; RING_LEN],
    head: usize,
    count: usize,
}

impl Default for FieldDebug {
    fn default() -> Self {
        Self {
            columns: Vec::new(),
            feet_y: 0.0,
            speed_yps: 0.0,
            mode: WalkMode::default(),
            h0: None,
            grounded: false,
            assist: 0.0,
            measurement_y: 0.0,
            enabled: false,
            decisions: [None, None],
            outcomes: [None, None],
            ring: std::array::from_fn(|_| None),
            head: 0,
            count: 0,
        }
    }
}

impl FieldDebug {
    /// Push one tick's (h0, y) into the ring.
    pub fn record(&mut self, h0: Option<f32>, y: f32) {
        self.ring[self.head] = Some((h0, y));
        self.head = (self.head + 1) % RING_LEN;
        if self.count < RING_LEN {
            self.count += 1;
        }
    }

    /// Ring contents oldest-first as (h0, y).
    pub fn history(&self) -> impl Iterator<Item = (Option<f32>, f32)> + '_ {
        let start = if self.count == RING_LEN { self.head } else { 0 };
        (0..self.count).map(move |i| match &self.ring[(start + i) % RING_LEN] {
            Some((h0, y)) => (*h0, *y),
            None => unreachable!("record fills every slot before advancing"),
        })
    }

    /// Sign flips of consecutive nonzero dy over the ring (the "zig" counter).
    pub fn reversals(&self) -> u32 {
        let ys: Vec<f32> = self.history().map(|(_, y)| y).collect();
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

    /// Max |second difference| of y over the ring (a straight climb reads ~0).
    pub fn max_d2y(&self) -> f32 {
        let ys: Vec<f32> = self.history().map(|(_, y)| y).collect();
        ys.windows(3)
            .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
            .fold(0.0f32, f32::max)
    }
}

/// Airborne-ness as the panel words it: only the fall branch and a not-yet-
/// loaded zone mean "no support this tick"; `Landed` already re-acquired it.
fn grounded_of(decision: &VerticalDecision) -> bool {
    !matches!(
        decision,
        VerticalDecision::Airborne { .. } | VerticalDecision::NoGeometry
    )
}

/// Sample the probe columns at the tick's post-move feet and record the whole
/// picture into `dbg`. Called from dispatch at both `walker::step` sites so
/// panel and gizmos show the same tick that moved the player; wire coords are
/// pre-step (`x_wire, y_wire, z_wire`) plus this tick's StepResult.
pub fn record_tick(
    dbg: &mut FieldDebug,
    geom: &MzbCollisionGeometry,
    x_wire: f32,
    y_wire: f32,
    z_wire: f32,
    heading: u8,
    speed_yps: f32,
    res: &StepResult,
    assist_yamls: f32,
    measurement_y: f32,
) {
    if !dbg.enabled {
        return;
    }
    let _ = z_wire; // the tick's own result carries where the feet ended up.
    dbg.assist = assist_yamls;
    dbg.measurement_y = measurement_y;
    let post_x = x_wire + res.dx;
    let post_y = y_wire + res.dy;
    let feet_xz = Vec2::new(post_x, -post_y);
    let feet_y = -res.feet_z;
    let (fwd_x, fwd_y) = crate::view_native::input::heading_to_forward(heading);
    let m = Vec2::new(fwd_x, -fwd_y);

    dbg.columns.clear();
    for &along in PROBE_COLUMN_YAMLS.iter() {
        let xz = feet_xz + m * along;
        let ground = geom.ground_walkable(xz, feet_y);
        let mut hits: Vec<(f32, f32)> = geom
            .ground_raycast_all(xz)
            .into_iter()
            .map(|(h, n)| (h, n.y))
            .collect();
        hits.sort_by(|a, b| {
            (a.0 - feet_y)
                .abs()
                .partial_cmp(&(b.0 - feet_y).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let hits_txt = hits
            .iter()
            .take(4)
            .map(|(h, ny)| format!("{h:+.3}@{ny:.2}"))
            .collect::<Vec<_>>()
            .join(" ");
        let walls = probe_chain_walls(geom, xz, feet_y);
        dbg.columns.push(ProbeColumn {
            xz,
            along,
            ground,
            hits: hits_txt,
            walls,
        });
    }

    dbg.h0 = dbg.columns.first().and_then(|c| c.ground);
    dbg.feet_y = feet_y;
    dbg.speed_yps = speed_yps;
    dbg.mode = res.mode;
    dbg.grounded = grounded_of(&res.decision);
    let prev = dbg.decisions[1];
    dbg.decisions[0] = prev;
    dbg.decisions[1] = Some(res.decision);
    let idle_repeat = matches!(res.outcome, HorizontalOutcome::NoInput)
        && matches!(dbg.outcomes[1], Some(HorizontalOutcome::NoInput));
    if !idle_repeat {
        let prev_out = dbg.outcomes[1];
        dbg.outcomes[0] = prev_out;
        dbg.outcomes[1] = Some(res.outcome);
    }
    dbg.record(dbg.h0, feet_y);
}

/// The body chain's wall contacts at one column: the same three centers and
/// radius `step`'s authored-wall probe uses, each rendered as distance, normal
/// and the blocking face's identity. " | "-joined; empty when nothing near.
pub fn probe_chain_walls(geom: &MzbCollisionGeometry, xz: Vec2, feet_y: f32) -> String {
    let mut parts = Vec::new();
    for off in [
        LOWER_CENTER_OFFSET,
        MIDDLE_CENTER_OFFSET,
        UPPER_CENTER_OFFSET,
    ] {
        let center = Vec3::new(xz.x, feet_y + off, xz.y);
        // Overlay probes a ground pose: lift anchor 0.0, same as the plain sweep.
        if let Some((d2, n)) = geom.nearest_wall_contact(center, BODY_RADIUS, feet_y, 0.0) {
            parts.push(format!(
                "off{off:.1} d{:.2} n({:.2},{:.2}) {}",
                d2.sqrt(),
                n.x,
                n.y,
                wall_face_ident(geom, center, d2, n)
            ));
        }
    }
    parts.join(" | ")
}

/// Identity of the face behind a wall contact: find the triangle matching the
/// distance and normal among the cells around the probe box. `nearest_wall_contact`
/// answers (d², normal) only, so the identity is recovered by match — same
/// procedure as `walker/tests_shipped_stairs.rs stair_column_dump`, which is
/// what tells an architecture riser from a prop shell.
fn wall_face_ident(geom: &MzbCollisionGeometry, center: Vec3, d2: f32, n: Vec3) -> String {
    let r = BODY_RADIUS;
    let lo = center - Vec2::splat(r).extend(0.0);
    let hi = center + Vec2::splat(r).extend(0.0);
    for slot in 0..kuluu_render::dat_mzb::ZONE_BLOCK_SLOTS {
        let blk = geom.block(slot as u8);
        if blk.cell_index.is_empty() {
            continue;
        }
        for cx in (lo.x / 8.0).floor() as i32..=(hi.x / 8.0).floor() as i32 {
            for cz in (lo.z / 8.0).floor() as i32..=(hi.z / 8.0).floor() as i32 {
                let Some(ids) = blk.cell_index.get(&(cx, cz)) else {
                    continue;
                };
                for &id in ids {
                    let id = id as usize;
                    let base = id * 3;
                    let (a, b, c) = (
                        blk.positions[blk.indices[base] as usize],
                        blk.positions[blk.indices[base + 1] as usize],
                        blk.positions[blk.indices[base + 2] as usize],
                    );
                    let nn = blk.tri_normals[id];
                    if (nn.x - n.x).abs() > 5e-3 || (nn.y - n.y).abs() > 5e-3 {
                        continue;
                    }
                    let d3 = kuluu_render::dat_mzb::point_tri_dist_sq(center, a, b, c);
                    if (d3 - d2).abs() > 1e-3 {
                        continue;
                    }
                    return format!(
                        "s{slot} t{id} sub{:x} p{} y[{:.1}..{:.1}]",
                        blk.tri_sub_area.get(id).copied().unwrap_or(0),
                        blk.tri_prop_blocker.get(id).copied().unwrap_or(false) as u8,
                        a.y.min(b.y).min(c.y),
                        a.y.max(b.y).max(c.y),
                    );
                }
            }
        }
    }
    "-".to_string()
}

pub fn sync_field_debug_enabled(
    panels: Res<kuluu_render::hud::HudPanels>,
    mut dbg: ResMut<FieldDebug>,
) {
    let want = panels.stair_draw || panels.stair_debug;
    if dbg.enabled != want {
        dbg.enabled = want;
    }
}

/// In-world gizmos behind the `stair_draw` toggle. Legend: white spheres at
/// each column's standable height (with a short riser line from the feet so
/// the step picture reads in-world), red X where a forward column answers no
/// ground, three bars at the feet xz marking wire y (cyan) and h0 (white;
/// coincident bars collapse), a cyan arrow for the heading, and this tick's
/// blocking contact as a sphere + bar in magenta for walls or reject red for
/// an actor.
pub fn draw_walker_field_gizmos(
    dbg: Res<FieldDebug>,
    panels: Res<kuluu_render::hud::HudPanels>,
    mut gizmos: Gizmos,
) {
    if !panels.stair_draw {
        return;
    }
    let white = Color::srgb(1.0, 1.0, 1.0);
    for c in &dbg.columns {
        let col = Vec3::new(c.xz.x, 0.0, c.xz.y);
        match c.ground {
            Some(h) => {
                gizmos.sphere(
                    Isometry3d::from_translation(col + Vec3::Y * (h + 0.05)),
                    0.05,
                    white,
                );
                if c.along > 0.1 {
                    let from = col + Vec3::Y * (dbg.feet_y + 0.2);
                    gizmos.line(from, from + Vec3::Y * 0.1, white);
                }
            }
            None => {
                let c_red = Color::srgb(1.0, 0.2, 0.2);
                let a = col + Vec3::Y * (dbg.feet_y + 0.1);
                gizmos.line(
                    a - Vec3::new(0.08, 0.08, 0.0),
                    a + Vec3::new(0.08, 0.08, 0.0),
                    c_red,
                );
                gizmos.line(
                    a - Vec3::new(0.08, -0.08, 0.0),
                    a + Vec3::new(0.08, -0.08, 0.0),
                    c_red,
                );
            }
        }
    }

    let feet = dbg.columns.first().map(|c| c.xz).unwrap_or(Vec2::ZERO);
    let c = Vec3::new(feet.x, 0.0, feet.y);
    let mut bars: Vec<(f32, Color)> = vec![(dbg.feet_y, Color::srgb(0.2, 0.9, 1.0))];
    if let Some(h0) = dbg.h0 {
        bars.push((h0, white));
    }
    bars.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    for (i, &(h, col)) in bars.iter().enumerate() {
        if i > 0 && (h - bars[i - 1].0).abs() < 1e-3 {
            continue;
        }
        gizmos.line(c + Vec3::Y * (h - 0.2), c + Vec3::Y * (h + 0.2), col);
    }

    if let Some(outcome) = dbg.outcomes[1] {
        if let Some(pt) = outcome.contact_point() {
            let color = match outcome {
                HorizontalOutcome::ActorContact { .. } => Color::srgb(1.0, 0.2, 0.2),
                _ => Color::srgb(1.0, 0.2, 1.0),
            };
            let pos = Vec3::new(pt.x, 0.0, pt.y);
            gizmos.sphere(
                Isometry3d::from_translation(pos + Vec3::Y * (dbg.feet_y + 0.05)),
                0.08,
                color,
            );
            gizmos.line(
                pos + Vec3::Y * (dbg.feet_y - 0.15),
                pos + Vec3::Y * (dbg.feet_y + 0.25),
                color,
            );
        }
    }
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

/// Summarize FieldDebug into the render-crate snapshot every frame while a
/// stair panel is showing. The panel shows defaults until dispatch has run.
pub fn update_stair_debug_snapshot_system(
    dbg: Res<FieldDebug>,
    panels: Res<kuluu_render::hud::HudPanels>,
    scene: Res<kuluu_render::snapshot::SceneState>,
    mut zone_cache: ResMut<StairDebugZoneCache>,
    mut snap: ResMut<StairDebugSnapshot>,
    dat_root: Res<crate::view_native::DatRootRes>,
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
            dat_root.0.as_deref(),
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

    if dbg.columns.is_empty() {
        return;
    }
    snap.field = Some(kuluu_render::hud::stair_debug::FieldSnapshot {
        mode: format!("{:?}", dbg.mode),
        decisions: [
            dbg.decisions[0].map(|d| d.label()),
            dbg.decisions[1].map(|d| d.label()),
        ],
        outcomes: [
            dbg.outcomes[0].map(|o| o.label()),
            dbg.outcomes[1].map(|o| o.label()),
        ],
        h0: dbg.h0,
        assist: dbg.assist,
        measurement_y: dbg.measurement_y,
        grounded: dbg.grounded,
        speed_yps: dbg.speed_yps,
        columns: dbg
            .columns
            .iter()
            .map(|c| ColumnRow {
                along: c.along,
                ground: c.ground,
                hits: c.hits.clone(),
                walls: c.walls.clone(),
            })
            .collect(),
        history: dbg.history().collect(),
        reversals_120: dbg.reversals(),
        max_d2y_120: dbg.max_d2y(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_tick_is_a_no_op_until_a_stair_panel_enables_it() {
        let geom = MzbCollisionGeometry::default();
        let res = StepResult {
            dx: 0.0,
            dy: 0.0,
            feet_z: 0.0,
            mode: WalkMode::default(),
            decision: VerticalDecision::Flat,
            outcome: HorizontalOutcome::NoInput,
        };
        let mut dbg = FieldDebug::default();
        record_tick(&mut dbg, &geom, 0.0, 0.0, 0.0, 0, 0.0, &res, 0.4, 0.0);
        assert!(dbg.columns.is_empty());
        assert_eq!(dbg.count, 0);

        dbg.enabled = true;
        record_tick(&mut dbg, &geom, 0.0, 0.0, 0.0, 0, 0.0, &res, 0.4, 0.0);
        assert!(!dbg.columns.is_empty());
        assert_eq!(dbg.count, 1);
    }

    #[test]
    fn ring_keeps_history_and_reversals_over_its_full_length() {
        let mut dbg = FieldDebug::default();
        for i in 0..(RING_LEN + 7) {
            dbg.record(Some(0.0), [0.0, 1.0, -1.0, 2.0][i % 4]);
        }
        assert_eq!(dbg.history().count(), RING_LEN);
        assert!(dbg.reversals() > 0);
        assert!(dbg.max_d2y() > 0.0);
    }

    #[test]
    fn landed_counts_as_grounded_and_airborne_does_not() {
        assert!(grounded_of(&VerticalDecision::Landed));
        assert!(!grounded_of(&VerticalDecision::Airborne {
            delta_y: -1.3,
            lean: 1.0
        }));
    }
}
