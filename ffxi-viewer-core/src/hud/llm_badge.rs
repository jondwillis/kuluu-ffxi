//! Right-anchored LLM-decision badge mirroring the chrome `draw_llm_badge`
//! layout (`ffxi-client/src/chrome.rs` — no-touch list, re-implemented here):
//!
//! ```text
//!   [●] LLM
//!   ▁▂▃▅▇█▇▅▃▂▁
//!   p50  4ms  p99 25ms
//!   paired 3 / solo 1
//! ```
//!
//! - **Dot** — pulses cyan-bold for ~200 ms after the most recent
//!   decision, fades to cyan→gray→dark-gray over ~2 s. Gray means the
//!   freshest decision wasn't paired with a notification (i.e. the
//!   harness acted on its own intuition rather than in response to a
//!   `notifications/resources/updated`); cyan means it was paired.
//! - **Sparkline** — last 32 latencies, window-max scaled, `▁`..`█`. Length
//!   matches sample count (4 decisions → 4 chars).
//! - **p50/p99** — nearest-rank percentile over all retained decisions.
//! - **paired/solo** — count of `ToolDispatched`s with a matching prior
//!   `NotificationFired`, vs count without one.
//!
//! # Pulse-decay clock
//!
//! `LlmDecision::at_monotonic_ms` and `SceneSnapshot::producer_monotonic_ms`
//! are both stamped in the producer process. Between snapshots, the
//! [`BadgeClock`] resource extrapolates `producer_now_ms` from Bevy's
//! real-wall clock, so the dot keeps fading even when no fresh snapshot
//! has arrived.

use std::collections::VecDeque;

use bevy::prelude::*;
use ffxi_viewer_wire::{LlmDecision, LlmDecisionKind};

use crate::hud::palette;
use crate::snapshot::SceneState;

/// Sliding window for the sparkline. Matches the producer-side cap on
/// `recent_decisions` (`state::RECENT_DECISIONS_CAP`) divided by 2 — the
/// dashboard never wants a sparkline so wide it overflows the card.
const SPARKLINE_N: usize = 32;

#[derive(Component)]
pub struct LlmBadge;

#[derive(Component)]
pub struct PulseDot;

#[derive(Component)]
pub struct Sparkline;

#[derive(Component)]
pub struct PercentileText;

#[derive(Component)]
pub struct PairingText;

/// Anchor relating producer-process monotonic ms to Bevy's real clock.
/// Refreshed on every fresh snapshot; the update system extrapolates
/// between fresh snapshots so pulse decay keeps animating.
#[derive(Resource, Default)]
pub struct BadgeClock {
    /// `producer_monotonic_ms` from the most-recent fresh snapshot.
    pub producer_ms_baseline: u64,
    /// Bevy `Time::elapsed_secs_f64()` at the moment that snapshot arrived.
    pub real_secs_baseline: f64,
    /// True once we've ever ingested a snapshot — distinguishes "process
    /// just started, no decisions ever" from "decisions exist".
    pub has_baseline: bool,
}

pub fn spawn_llm_badge(mut commands: Commands) {
    commands
        .spawn((
            LlmBadge,
            Node {
                position_type: PositionType::Absolute,
                // Top-right column, slot 3: under the Vana clock. See
                // `hud/vana_clock.rs` for the full column ordering.
                top: Val::Px(140.0),
                right: Val::Px(8.0),
                width: Val::Px(220.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
        ))
        .with_children(|p| {
            // Dot + label row.
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|row| {
                row.spawn((
                    PulseDot,
                    Text::new("●".to_string()),
                    TextFont { font_size: 14.0, ..default() },
                    TextColor(palette::DARK),
                ));
                row.spawn((
                    Text::new("LLM".to_string()),
                    TextFont { font_size: 13.0, ..default() },
                    TextColor(palette::TEXT),
                ));
            });
            p.spawn((
                Sparkline,
                Text::new("".to_string()),
                TextFont { font_size: 14.0, ..default() },
                TextColor(palette::ACCENT),
            ));
            p.spawn((
                PercentileText,
                Text::new("p50  —     p99  —".to_string()),
                TextFont { font_size: 12.0, ..default() },
                TextColor(palette::MUTED),
            ));
            p.spawn((
                PairingText,
                Text::new("—".to_string()),
                TextFont { font_size: 12.0, ..default() },
                TextColor(palette::MUTED),
            ));
        });
}

/// Capture `(producer_monotonic_ms, real_secs)` whenever a fresh snapshot
/// arrives, so the badge can extrapolate `producer_now` between snapshots.
pub fn refresh_badge_clock_system(
    state: Res<SceneState>,
    time: Res<Time<Real>>,
    mut clock: ResMut<BadgeClock>,
) {
    if !state.dirty {
        return;
    }
    clock.producer_ms_baseline = state.snapshot.producer_monotonic_ms;
    clock.real_secs_baseline = time.elapsed_secs_f64();
    clock.has_baseline = true;
}

pub fn update_llm_badge_system(
    state: Res<SceneState>,
    time: Res<Time<Real>>,
    clock: Res<BadgeClock>,
    mut q_dot: Query<
        (&mut TextColor, &mut Text),
        (
            With<PulseDot>,
            Without<Sparkline>,
            Without<PercentileText>,
            Without<PairingText>,
        ),
    >,
    mut q_spark: Query<
        &mut Text,
        (
            With<Sparkline>,
            Without<PulseDot>,
            Without<PercentileText>,
            Without<PairingText>,
        ),
    >,
    mut q_pct: Query<
        &mut Text,
        (
            With<PercentileText>,
            Without<PulseDot>,
            Without<Sparkline>,
            Without<PairingText>,
        ),
    >,
    mut q_pair: Query<
        &mut Text,
        (
            With<PairingText>,
            Without<PulseDot>,
            Without<Sparkline>,
            Without<PercentileText>,
        ),
    >,
) {
    let decisions: &VecDeque<LlmDecision> = &state.snapshot.recent_decisions_view();

    if let Ok(mut text) = q_spark.single_mut() {
        **text = sparkline_for_latencies(decisions, SPARKLINE_N);
    }
    if let Ok(mut text) = q_pct.single_mut() {
        **text = match percentile_pair(decisions) {
            Some((p50, p99)) => {
                format!("p50 {:>6}  p99 {:>6}", format_us(p50), format_us(p99))
            }
            None => "p50  —     p99  —".to_string(),
        };
    }

    let pairing = pairing_summary(decisions);

    if let Ok(mut text) = q_pair.single_mut() {
        **text = if decisions.is_empty() {
            "—".to_string()
        } else {
            format!(
                "paired {} / solo {}",
                pairing.paired_count, pairing.solo_dispatches
            )
        };
    }

    if let Ok((mut color, mut text)) = q_dot.single_mut() {
        let now_producer_ms = if clock.has_baseline {
            let elapsed = (time.elapsed_secs_f64() - clock.real_secs_baseline).max(0.0);
            clock.producer_ms_baseline + (elapsed * 1000.0) as u64
        } else {
            0
        };
        let last_age = decisions
            .back()
            .map(|d| now_producer_ms.saturating_sub(d.at_monotonic_ms));
        let (c, glyph) = pulse_glyph(last_age, pairing.latest_paired);
        color.0 = c;
        **text = glyph.to_string();
    }
}

/// Extension trait keeps the system body tidy. `recent_decisions` is a
/// plain `Vec<LlmDecision>` on the wire; the badge logic was written
/// against a `VecDeque` (matching the chrome side) — we adapt with a
/// zero-cost view rather than allocate.
trait RecentDecisionsView {
    fn recent_decisions_view(&self) -> VecDeque<LlmDecision>;
}

impl RecentDecisionsView for ffxi_viewer_wire::SceneSnapshot {
    fn recent_decisions_view(&self) -> VecDeque<LlmDecision> {
        // VecDeque<T>: From<Vec<T>> is O(1) (moves the buffer). Cloning
        // to keep the snapshot immutable; could be avoided by passing
        // `&[LlmDecision]` everywhere, at the cost of `pop_back`/`pop_front`
        // ergonomics in `pairing_summary`. The vec is capped at 64 entries
        // — clone cost is negligible.
        VecDeque::from(self.recent_decisions.clone())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PairingSummary {
    paired_count: usize,
    solo_dispatches: usize,
    /// `true` if the most-recent dispatch in the log paired with a prior
    /// notification. Drives the dot color.
    latest_paired: bool,
}

/// Walk the decision log oldest→newest, LIFO-pairing each `ToolDispatched`
/// with the most-recent unmatched `NotificationFired`. URI matching is
/// recency-only since `ToolDispatched` doesn't carry a URI on the wire.
fn pairing_summary(decisions: &VecDeque<LlmDecision>) -> PairingSummary {
    let mut unmatched_notifs: Vec<()> = Vec::new();
    let mut paired = 0;
    let mut solo = 0;
    let mut latest_was_dispatch = false;
    let mut latest_paired = false;

    for d in decisions {
        match &d.kind {
            LlmDecisionKind::NotificationFired { .. } => {
                unmatched_notifs.push(());
                latest_was_dispatch = false;
            }
            LlmDecisionKind::ToolDispatched { .. } => {
                if unmatched_notifs.pop().is_some() {
                    paired += 1;
                    latest_paired = true;
                } else {
                    solo += 1;
                    latest_paired = false;
                }
                latest_was_dispatch = true;
            }
        }
    }

    if !latest_was_dispatch {
        latest_paired = false;
    }

    PairingSummary {
        paired_count: paired,
        solo_dispatches: solo,
        latest_paired,
    }
}

/// Render the last `n` latencies as `▁..█`. Length matches sample count,
/// so 4 decisions → 4 chars (vs padded → 32 chars).
fn sparkline_for_latencies(decisions: &VecDeque<LlmDecision>, n: usize) -> String {
    const RAMP: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let take_from = decisions.len().saturating_sub(n);
    let window: Vec<u64> = decisions
        .iter()
        .skip(take_from)
        .map(|d| d.latency_us)
        .collect();
    if window.is_empty() {
        return String::new();
    }
    let max = window.iter().copied().max().unwrap_or(1).max(1);
    window
        .iter()
        .map(|&v| {
            let r = ((v as f64 / max as f64) * (RAMP.len() - 1) as f64).round() as usize;
            RAMP[r.min(RAMP.len() - 1)]
        })
        .collect()
}

/// Nearest-rank percentile (p50, p99). `None` when the log is empty.
fn percentile_pair(decisions: &VecDeque<LlmDecision>) -> Option<(u64, u64)> {
    if decisions.is_empty() {
        return None;
    }
    let mut sorted: Vec<u64> = decisions.iter().map(|d| d.latency_us).collect();
    sorted.sort_unstable();
    let n = sorted.len();
    let p50 = sorted[((n * 50 + 99) / 100).saturating_sub(1).min(n - 1)];
    let p99 = sorted[((n * 99 + 99) / 100).saturating_sub(1).min(n - 1)];
    Some((p50, p99))
}

fn pulse_glyph(age_ms: Option<u64>, paired: bool) -> (Color, char) {
    let Some(age) = age_ms else {
        return (palette::DARK, '○');
    };
    let color = match (age, paired) {
        (a, true) if a < 200 => palette::ACCENT,
        (a, true) if a < 600 => palette::ACCENT,
        (a, false) if a < 600 => palette::TEXT,
        (a, _) if a < 2_000 => palette::MUTED,
        _ => palette::DARK,
    };
    let glyph = if age < 200 { '◉' } else { '●' };
    (color, glyph)
}

fn format_us(us: u64) -> String {
    if us < 1_000 {
        format!("{us}μs")
    } else if us < 1_000_000 {
        format!("{}ms", us / 1_000)
    } else {
        format!("{:.1}s", us as f64 / 1_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nf(uri: &str, ts: u64) -> LlmDecision {
        LlmDecision {
            kind: LlmDecisionKind::NotificationFired { uri: uri.into() },
            latency_us: 200,
            at_monotonic_ms: ts,
        }
    }
    fn td(tool: &str, ts: u64, lat_us: u64) -> LlmDecision {
        LlmDecision {
            kind: LlmDecisionKind::ToolDispatched { tool: tool.into() },
            latency_us: lat_us,
            at_monotonic_ms: ts,
        }
    }

    #[test]
    fn pairing_lifo_matches_dispatch_to_most_recent_notification() {
        let v: VecDeque<LlmDecision> = vec![
            nf("scene://current", 100),
            nf("party://members", 110),
            td("engage", 200, 25_000),
        ]
        .into();
        let p = pairing_summary(&v);
        assert_eq!(p.paired_count, 1);
        assert_eq!(p.solo_dispatches, 0);
        assert!(p.latest_paired);
    }

    #[test]
    fn pairing_solo_dispatch_when_no_prior_notif() {
        let v: VecDeque<LlmDecision> = vec![td("engage", 200, 25_000)].into();
        let p = pairing_summary(&v);
        assert_eq!(p.paired_count, 0);
        assert_eq!(p.solo_dispatches, 1);
        assert!(!p.latest_paired);
    }

    #[test]
    fn pairing_latest_is_notif_means_unpaired_pulse() {
        // Last entry is a notification; the dot reflects "responded yet?"
        // — so latest_paired is false until the dispatch arrives.
        let v: VecDeque<LlmDecision> =
            vec![td("engage", 100, 25_000), nf("scene://current", 200)].into();
        let p = pairing_summary(&v);
        assert_eq!(p.paired_count, 0);
        assert_eq!(p.solo_dispatches, 1);
        assert!(!p.latest_paired);
    }

    #[test]
    fn sparkline_length_matches_sample_count() {
        let v: VecDeque<LlmDecision> =
            vec![td("a", 1, 100), td("b", 2, 200), td("c", 3, 300)].into();
        let s = sparkline_for_latencies(&v, 32);
        assert_eq!(s.chars().count(), 3);
    }

    #[test]
    fn sparkline_empty_for_empty_log() {
        let v: VecDeque<LlmDecision> = VecDeque::new();
        assert_eq!(sparkline_for_latencies(&v, 32), "");
    }

    #[test]
    fn sparkline_window_max_scaling() {
        let v: VecDeque<LlmDecision> =
            vec![td("a", 1, 1), td("b", 2, 1_000_000)].into();
        let s = sparkline_for_latencies(&v, 32);
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(chars.len(), 2);
        // Smallest renders at ▁; largest at █.
        assert_eq!(chars[0], '▁');
        assert_eq!(chars[1], '█');
    }

    #[test]
    fn percentile_pair_basic() {
        // 100 entries, latency = i*100 (in us).
        let mut v = VecDeque::new();
        for i in 1..=100u64 {
            v.push_back(td("t", i, i * 100));
        }
        let (p50, p99) = percentile_pair(&v).unwrap();
        // Nearest-rank: p50 → index 49 → value 5000; p99 → index 98 → 9900.
        assert_eq!(p50, 5000);
        assert_eq!(p99, 9900);
    }

    #[test]
    fn percentile_pair_none_when_empty() {
        let v: VecDeque<LlmDecision> = VecDeque::new();
        assert!(percentile_pair(&v).is_none());
    }

    #[test]
    fn pulse_glyph_dark_when_no_decisions() {
        let (c, g) = pulse_glyph(None, false);
        assert_eq!(c, palette::DARK);
        assert_eq!(g, '○');
    }

    #[test]
    fn pulse_glyph_bright_paired_within_200ms() {
        let (c, g) = pulse_glyph(Some(50), true);
        assert_eq!(c, palette::ACCENT);
        assert_eq!(g, '◉');
    }

    #[test]
    fn pulse_glyph_fades_to_dark_after_2s() {
        let (c, _) = pulse_glyph(Some(3_000), true);
        assert_eq!(c, palette::DARK);
    }

    #[test]
    fn format_us_units_scale() {
        assert_eq!(format_us(412), "412μs");
        assert_eq!(format_us(25_000), "25ms");
        assert_eq!(format_us(1_500_000), "1.5s");
    }
}
