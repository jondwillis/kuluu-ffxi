//! Top-center "current target" HUD panel.
//!
//! Reads `Target.id`, looks up the matching `Entity` in
//! `SceneState.snapshot.entities`, and surfaces a compact frame:
//!
//! ```text
//! ┌─────────────────────────────┐
//! │ Mandragora     [mob]        │
//! │ HP: ████░░░░░░ 42%   d=12y  │
//! └─────────────────────────────┘
//! ```
//!
//! Visibility: `Display::None` when no target is selected (or the selected
//! id is missing from the latest snapshot). Toggling display avoids
//! spawn/despawn churn at 60 Hz.
//!
//! Distance is computed in FFXI yalms, not Bevy units, by subtracting in
//! wire-space (`snapshot.self_pos.pos` and `entity.pos`). 1 yalm ≈ the
//! native unit of FFXI's coord system; the `ffxi_to_bevy` swap that the
//! 3D scene uses is irrelevant to a scalar distance.
//!
//! HP bar coloring follows the same green/yellow/red bands as classic
//! FFXI target frames so the operator's at-a-glance read matches client
//! muscle memory.

use bevy::prelude::*;
use ffxi_viewer_wire::{Entity as WireEntity, EntityKind, ReactorGoal};

use crate::hud::palette;
use crate::scene::Target;
use crate::snapshot::SceneState;

/// Marker on the panel root.
#[derive(Component)]
pub struct TargetPanel;

/// Marker on the row that carries name + kind tag.
#[derive(Component)]
pub struct TargetHeader;

/// Marker on the HP bar fill rect (width-driven).
#[derive(Component)]
pub struct TargetHpFill;

/// Marker on the HP-percent text node.
#[derive(Component)]
pub struct TargetHpText;

/// Marker on the distance-readout text node.
#[derive(Component)]
pub struct TargetDistText;

/// Marker on the small "⚔ Engaged" badge that surfaces when the reactor's
/// current goal is `Engaged{target_id}` AND that target_id matches the
/// panel's selected target. Empty string when not engaged.
#[derive(Component)]
pub struct TargetEngagedBadge;

/// Pulse latch: whenever the count of `ChatChannel::Battle` lines grows
/// (server confirmed a hit/miss/proc/etc.), `last_swing_secs` is set to
/// the current `Time::elapsed_secs()`. The badge color modulates from
/// bright-white back to base red over `PULSE_DECAY_SECS` after each
/// pulse — every server-side swing is visible as a flash.
///
/// Frame-rate independent: an inbound 0x028 → ChatLine → snapshot fold
/// happens at server tick rate; the badge flash decays at render rate.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct SwingPulse {
    pub last_swing_secs: f32,
    pub prev_battle_count: usize,
}

/// Time for the swing flash to fade back to base color. 0.25 s is short
/// enough that successive swings (auto-attack is ~3 s/swing at base, but
/// procs/additional effects can stack within a single 0x028) read as
/// distinct pulses, not a continuous glow.
const PULSE_DECAY_SECS: f32 = 0.25;

const PANEL_WIDTH_PX: f32 = 280.0;
const HP_BAR_WIDTH_PX: f32 = 200.0;
const HP_BAR_HEIGHT_PX: f32 = 6.0;

pub fn spawn_target_panel(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            TargetPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(36.0), // below the stage bar
                // Center horizontally: left = 50% then translate by half width.
                // Bevy UI doesn't have a built-in "anchor center", so we
                // split it into a percentage offset + a fixed pixel pull.
                left: Val::Percent(50.0),
                margin: UiRect {
                    left: Val::Px(-PANEL_WIDTH_PX / 2.0),
                    ..default()
                },
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            // Header row: name (left) + kind tag (right).
            p.spawn((
                TargetHeader,
                Text::new(""),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));

            // Engaged badge — empty when idle, "⚔ Engaged" in red when
            // the reactor is currently auto-attacking this target. Color
            // matches the panel border swap so the operator's eye picks
            // up the change in either spot.
            p.spawn((
                TargetEngagedBadge,
                Text::new(""),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(palette::STAGE_BAD),
            ));

            // HP bar row: track + fill + percent + distance.
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                ..default()
            })
            .with_children(|row| {
                // HP track.
                row.spawn((
                    Node {
                        width: Val::Px(HP_BAR_WIDTH_PX),
                        height: Val::Px(HP_BAR_HEIGHT_PX),
                        ..default()
                    },
                    BackgroundColor(palette::DARK),
                ))
                .with_children(|track| {
                    track.spawn((
                        TargetHpFill,
                        Node {
                            width: Val::Px(0.0),
                            height: Val::Px(HP_BAR_HEIGHT_PX),
                            ..default()
                        },
                        BackgroundColor(hp_color(100)),
                    ));
                });
                row.spawn((
                    TargetHpText,
                    Text::new("—"),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                ));
                row.spawn((
                    TargetDistText,
                    Text::new(""),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                ));
            });
        });
}

/// Per-frame: hide when no target, otherwise rewrite text + bar fill.
pub fn update_target_panel_system(
    target: Res<Target>,
    state: Res<SceneState>,
    mut panel_q: Query<
        (&mut Node, &mut BorderColor),
        (With<TargetPanel>, Without<TargetHpFill>),
    >,
    mut header_q: Query<
        &mut Text,
        (
            With<TargetHeader>,
            Without<TargetHpText>,
            Without<TargetDistText>,
            Without<TargetEngagedBadge>,
        ),
    >,
    mut hp_fill_q: Query<
        (&mut Node, &mut BackgroundColor),
        (With<TargetHpFill>, Without<TargetPanel>),
    >,
    mut hp_text_q: Query<
        (&mut Text, &mut TextColor),
        (
            With<TargetHpText>,
            Without<TargetHeader>,
            Without<TargetDistText>,
            Without<TargetEngagedBadge>,
        ),
    >,
    mut dist_q: Query<
        &mut Text,
        (
            With<TargetDistText>,
            Without<TargetHeader>,
            Without<TargetHpText>,
            Without<TargetEngagedBadge>,
        ),
    >,
    mut engaged_q: Query<
        &mut Text,
        (
            With<TargetEngagedBadge>,
            Without<TargetHeader>,
            Without<TargetHpText>,
            Without<TargetDistText>,
        ),
    >,
) {
    if !state.is_changed() && !target.is_changed() {
        return;
    }

    let Ok((mut panel_node, mut panel_border)) = panel_q.single_mut() else {
        return;
    };

    let Some(target_id) = target.id else {
        if panel_node.display != Display::None {
            panel_node.display = Display::None;
        }
        return;
    };

    let snap = &state.snapshot;
    let Some(ent) = snap.entities.iter().find(|e| e.id == target_id) else {
        // Target id no longer in snapshot (despawned or out of range).
        if panel_node.display != Display::None {
            panel_node.display = Display::None;
        }
        return;
    };

    if panel_node.display == Display::None {
        panel_node.display = Display::Flex;
    }

    // Engagement state. Server-authoritative: the local PC entity's
    // `bt_target_id` is the server's notion of "what am I auto-attacking"
    // and is set by 0x00D updates regardless of how engagement started
    // (F-keybind, /attack typed, mob aggro pulling us into combat, MCP
    // reactor goal, …). If `bt_target_id == panel's selected target_id`,
    // the player is currently swinging at this target — flag engaged.
    //
    // Fallback to `current_goal == Engaged{target}` for the brief window
    // after pressing F before the first 0x00D update reflects the new
    // bt_target_id server-side. That keeps the badge from flickering
    // off in the gap.
    let self_engaged_on_target = snap
        .self_char_id
        .and_then(|sid| snap.entities.iter().find(|e| e.id == sid))
        .map(|self_pc| self_pc.bt_target_id == target_id)
        .unwrap_or(false);
    let goal_engaged_on_target = matches!(
        snap.current_goal,
        Some(ReactorGoal::Engaged { target_id: g, .. }) if g == target_id
    );
    let engaged_on_this = self_engaged_on_target || goal_engaged_on_target;
    let want_border = if engaged_on_this {
        palette::STAGE_BAD // red — engaged
    } else {
        palette::ACCENT // cyan — targeted but not engaged
    };
    if panel_border.left != want_border {
        *panel_border = BorderColor::all(want_border);
    }
    if let Ok(mut text) = engaged_q.single_mut() {
        let want = if engaged_on_this { "⚔ Engaged" } else { "" };
        if text.as_str() != want {
            **text = want.to_string();
        }
    }

    if let Ok(mut text) = header_q.single_mut() {
        let want = format_header(ent);
        if **text != want {
            **text = want;
        }
    }

    let pct = ent.hp_pct.unwrap_or(0);
    if let Ok((mut fill, mut bg)) = hp_fill_q.single_mut() {
        let want_w = HP_BAR_WIDTH_PX * (pct as f32 / 100.0).clamp(0.0, 1.0);
        if fill.width != Val::Px(want_w) {
            fill.width = Val::Px(want_w);
        }
        let want_color = hp_color(pct);
        if bg.0 != want_color {
            bg.0 = want_color;
        }
    }

    if let Ok((mut text, _tc)) = hp_text_q.single_mut() {
        let want = match ent.hp_pct {
            Some(p) => format!("{p}%"),
            None => "—".into(),
        };
        if **text != want {
            **text = want;
        }
    }

    if let Ok(mut text) = dist_q.single_mut() {
        let dx = snap.self_pos.pos.x - ent.pos.x;
        let dy = snap.self_pos.pos.y - ent.pos.y;
        let dz = snap.self_pos.pos.z - ent.pos.z;
        let d = (dx * dx + dy * dy + dz * dz).sqrt();
        let want = format!("d={d:.1}y");
        if **text != want {
            **text = want;
        }
    }
}

/// Latch [`SwingPulse::last_swing_secs`] whenever the count of
/// `ChatChannel::Battle` lines in the rendered chat grows. Each grow
/// corresponds to a server-confirmed combat event (hit / miss / proc /
/// additional effect / reaction) — every one of those should pulse the
/// engaged badge so the operator sees the rhythm of combat.
///
/// Uses the same `prev_*_len` idiom as
/// [`crate::hud::chat_panel::ChatPanelDecay`]: snapshot the prior count,
/// compare against the live one, latch on growth. Frame-rate independent
/// — the latch sets a timestamp, decay happens in
/// [`pulse_engaged_badge_color_system`].
pub fn detect_swing_pulse_system(
    time: Res<Time>,
    state: Res<crate::snapshot::SceneState>,
    mut pulse: ResMut<SwingPulse>,
) {
    let count = crate::snapshot::rendered_chat(&state)
        .iter()
        .filter(|l| l.channel == ffxi_viewer_wire::ChatChannel::Battle)
        .count();
    if count > pulse.prev_battle_count {
        pulse.last_swing_secs = time.elapsed_secs();
    }
    pulse.prev_battle_count = count;
}

/// Per-frame: modulate the engaged badge's text color from bright-white
/// (just-pulsed) back to base red over [`PULSE_DECAY_SECS`]. Outside the
/// decay window the color sits at base red.
///
/// Lives in its own system so it can tick every frame without piggybacking
/// on `update_target_panel_system`'s `is_changed` early-return — a pulse
/// fired by a server tick must keep decaying visually even when nothing
/// else about the snapshot has changed.
pub fn pulse_engaged_badge_color_system(
    time: Res<Time>,
    pulse: Res<SwingPulse>,
    mut q: Query<&mut TextColor, With<TargetEngagedBadge>>,
) {
    let Ok(mut tc) = q.single_mut() else {
        return;
    };
    let elapsed = (time.elapsed_secs() - pulse.last_swing_secs).max(0.0);
    let t = (elapsed / PULSE_DECAY_SECS).clamp(0.0, 1.0);
    // Linear blend from white (flash) → STAGE_BAD red (base). Bevy's
    // `Color::srgb` channels need component math here; the engaged badge
    // is text, so background blend doesn't apply.
    let base = palette::STAGE_BAD.to_srgba();
    let flash = bevy::prelude::Color::WHITE.to_srgba();
    let r = flash.red + (base.red - flash.red) * t;
    let g = flash.green + (base.green - flash.green) * t;
    let b = flash.blue + (base.blue - flash.blue) * t;
    let want = bevy::prelude::Color::srgb(r, g, b);
    if tc.0 != want {
        tc.0 = want;
    }
}

fn format_header(ent: &WireEntity) -> String {
    let name = ent.name.as_deref().unwrap_or("?");
    format!("{name}    [{}]", kind_tag(ent.kind))
}

fn kind_tag(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Pc => "pc",
        EntityKind::Npc => "npc",
        EntityKind::Mob => "mob",
        EntityKind::Pet => "pet",
        EntityKind::Other => "obj",
    }
}

fn hp_color(pct: u8) -> Color {
    match pct {
        76..=100 => Color::srgb(0.30, 0.85, 0.30), // green
        26..=75 => Color::srgb(0.95, 0.75, 0.20),  // yellow
        _ => Color::srgb(0.95, 0.20, 0.20),        // red
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::Vec3;

    fn ent(name: &str, kind: EntityKind, hp: Option<u8>, x: f32) -> WireEntity {
        WireEntity {
            id: 1,
            act_index: 0,
            kind,
            name: Some(name.into()),
            pos: Vec3 { x, y: 0.0, z: 0.0 },
            heading: 0,
            hp_pct: hp,
            bt_target_id: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
        }
    }

    #[test]
    fn header_format_includes_kind() {
        assert_eq!(
            format_header(&ent("Mandy", EntityKind::Mob, Some(50), 0.0)),
            "Mandy    [mob]"
        );
        assert_eq!(
            format_header(&ent("Selh", EntityKind::Npc, None, 0.0)),
            "Selh    [npc]"
        );
    }

    #[test]
    fn hp_bands() {
        assert_eq!(hp_color(100), Color::srgb(0.30, 0.85, 0.30));
        assert_eq!(hp_color(50), Color::srgb(0.95, 0.75, 0.20));
        assert_eq!(hp_color(10), Color::srgb(0.95, 0.20, 0.20));
    }

    #[test]
    fn swing_pulse_default_starts_at_zero() {
        let p = SwingPulse::default();
        assert_eq!(p.last_swing_secs, 0.0);
        assert_eq!(p.prev_battle_count, 0);
    }

    /// The latch logic outside the Bevy system — extracted so we can
    /// drive it with known inputs. Mirrors what `detect_swing_pulse_system`
    /// does each frame: compare count, latch on growth, update prev.
    fn step_pulse(pulse: &mut SwingPulse, count: usize, now: f32) {
        if count > pulse.prev_battle_count {
            pulse.last_swing_secs = now;
        }
        pulse.prev_battle_count = count;
    }

    #[test]
    fn swing_pulse_latches_on_battle_line_growth() {
        let mut p = SwingPulse::default();
        // Zone-in with no battle: no latch.
        step_pulse(&mut p, 0, 1.0);
        assert_eq!(p.last_swing_secs, 0.0);
        // First swing arrives — latch.
        step_pulse(&mut p, 1, 2.0);
        assert_eq!(p.last_swing_secs, 2.0);
        // Same count next frame — no re-latch.
        step_pulse(&mut p, 1, 2.5);
        assert_eq!(p.last_swing_secs, 2.0);
        // Another swing — latch updates.
        step_pulse(&mut p, 3, 3.0); // proc + headline = 2 new lines
        assert_eq!(p.last_swing_secs, 3.0);
    }

    #[test]
    fn swing_pulse_does_not_latch_on_count_shrink() {
        // ChatChannel::Battle line count can shrink if the chat history
        // cap evicts old lines — that's not a swing event, must not
        // latch.
        let mut p = SwingPulse {
            last_swing_secs: 5.0,
            prev_battle_count: 256,
        };
        step_pulse(&mut p, 200, 10.0); // 56 lines aged out
        assert_eq!(p.last_swing_secs, 5.0, "shrink must not latch");
        assert_eq!(p.prev_battle_count, 200);
    }
}
