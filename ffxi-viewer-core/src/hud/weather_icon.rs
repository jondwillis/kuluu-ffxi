// Placeholder icons. Replace with retail DAT-sourced sprites once
// `ffxi-dat` exposes them; see Track A research output for the DAT path.
//
//! Weather icon HUD widget.
//!
//! Reads `SceneState.snapshot.weather` (populated from packet 0x057 by
//! a parallel track) and renders a single glyph + tooltip label.
//!
//! The HUD framework is Bevy UI (matching `status_ribbon`, `vana_clock`,
//! `logout_countdown`, etc. — no egui in this crate). Layout slot is
//! the top-right column: just to the left of `vana_clock` (which sits at
//! `top: 104, right: 8`). When `weather` is `None` or `Weather::None`
//! the node is hidden via `Display::None`, matching the convention used
//! by `status_ribbon` chips.

use bevy::prelude::*;
use ffxi_viewer_wire::Weather;

use crate::hud::palette;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct WeatherIconPanel;

#[derive(Component)]
pub struct WeatherIconGlyph;

/// Placeholder Unicode glyph for each weather variant. Real retail icon
/// sprites live in DAT files whose IDs are not yet known — replace once
/// `ffxi-dat` exposes the path. `Weather::None` returns an empty string
/// and the widget hides itself.
pub fn weather_glyph(w: Weather) -> &'static str {
    match w {
        Weather::None => "",
        Weather::Sunshine => "\u{2600}",                         // ☀
        Weather::Clouds => "\u{2601}",                           // ☁
        Weather::Fog => "\u{1F32B}",                             // 🌫
        Weather::HotSpell | Weather::HeatWave => "\u{1F525}",    // 🔥
        Weather::Rain | Weather::Squall => "\u{1F327}",          // 🌧
        Weather::DustStorm | Weather::SandStorm => "\u{1F32A}",  // 🌪
        Weather::Wind | Weather::Gales => "\u{1F4A8}",           // 💨
        Weather::Snow | Weather::Blizzards => "\u{2744}",        // ❄
        Weather::Thunder | Weather::Thunderstorms => "\u{26A1}", // ⚡
        Weather::Auroras => "\u{1F30C}",                         // 🌌
        Weather::StellarGlare => "\u{2728}",                     // ✨
        Weather::Gloom | Weather::Darkness => "\u{25CF}",        // ●
    }
}

/// Human-readable name used as the tooltip / accessible label. Hand-
/// written so capitalisation matches the in-game text ("HotSpell" →
/// "Hot Spell" rather than a naive `to_string` of the variant).
pub fn weather_label(w: Weather) -> &'static str {
    match w {
        Weather::None => "",
        Weather::Sunshine => "Sunshine",
        Weather::Clouds => "Clouds",
        Weather::Fog => "Fog",
        Weather::HotSpell => "Hot Spell",
        Weather::HeatWave => "Heat Wave",
        Weather::Rain => "Rain",
        Weather::Squall => "Squall",
        Weather::DustStorm => "Dust Storm",
        Weather::SandStorm => "Sand Storm",
        Weather::Wind => "Wind",
        Weather::Gales => "Gales",
        Weather::Snow => "Snow",
        Weather::Blizzards => "Blizzards",
        Weather::Thunder => "Thunder",
        Weather::Thunderstorms => "Thunderstorms",
        Weather::Auroras => "Auroras",
        Weather::StellarGlare => "Stellar Glare",
        Weather::Gloom => "Gloom",
        Weather::Darkness => "Darkness",
    }
}

/// Spawn the weather icon as a child of the bottom-left flex stack
/// (above the minimap), matching retail's layout where weather sits
/// in the same corner as the compass-equivalent. Width is auto so
/// the chip hugs its glyph + label.
///
/// Hidden by default via `Display::None`; `update_weather_icon`
/// flips it to `Flex` whenever `SceneState.snapshot.weather` is a
/// non-`None` variant. (In LSB dev environments the weather snapshot
/// often stays `None` until a 0x057 packet arrives — that's why the
/// chip can look "always hidden" without a server emitting weather.)
pub fn spawn_weather_icon_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        WeatherIconPanel,
        Node {
            // `flex_shrink: 0` matches the minimap so the bottom-left
            // stack doesn't compress this chip when the chat panel
            // expands.
            flex_shrink: 0.0,
            padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
            border: UiRect::all(Val::Px(1.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            display: Display::None,
            ..default()
        },
        BackgroundColor(palette::BACKGROUND),
        BorderColor::all(palette::BORDER),
    ))
    .with_children(|p| {
        p.spawn((
            WeatherIconGlyph,
            // The tooltip label is folded into the same Text node
            // as a trailing " name" suffix, matching the chip-style
            // single-line layout used by `vana_clock`. A real
            // tooltip-on-hover system is out of scope for this stub.
            Text::new(""),
            TextFont {
                font_size: 14.0,
                ..default()
            },
            TextColor(palette::TEXT),
        ));
    });
}

pub fn update_weather_icon(
    state: Res<SceneState>,
    mut panel_q: Query<&mut Node, With<WeatherIconPanel>>,
    mut text_q: Query<&mut Text, With<WeatherIconGlyph>>,
) {
    if !state.dirty {
        return;
    }
    let weather = state.snapshot.weather.unwrap_or(Weather::None);
    let glyph = weather_glyph(weather);

    let Ok(mut node) = panel_q.single_mut() else {
        return;
    };
    let Ok(mut text) = text_q.single_mut() else {
        return;
    };

    if glyph.is_empty() {
        if node.display != Display::None {
            node.display = Display::None;
        }
        return;
    }

    if node.display != Display::Flex {
        node.display = Display::Flex;
    }
    let want = format!("{glyph} {}", weather_label(weather));
    if **text != want {
        **text = want;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_variant_has_empty_glyph_and_label() {
        assert_eq!(weather_glyph(Weather::None), "");
        assert_eq!(weather_label(Weather::None), "");
    }

    #[test]
    fn every_non_none_variant_has_a_glyph() {
        // Cheap exhaustiveness check — any new Weather variant added in
        // ffxi-viewer-wire will fall into the `_` arm of `weather_glyph`
        // if we forget to extend the match, returning the empty string
        // and tripping this assertion.
        let all = [
            Weather::Sunshine,
            Weather::Clouds,
            Weather::Fog,
            Weather::HotSpell,
            Weather::HeatWave,
            Weather::Rain,
            Weather::Squall,
            Weather::DustStorm,
            Weather::SandStorm,
            Weather::Wind,
            Weather::Gales,
            Weather::Snow,
            Weather::Blizzards,
            Weather::Thunder,
            Weather::Thunderstorms,
            Weather::Auroras,
            Weather::StellarGlare,
            Weather::Gloom,
            Weather::Darkness,
        ];
        for w in all {
            assert!(!weather_glyph(w).is_empty(), "{w:?} missing glyph");
            assert!(!weather_label(w).is_empty(), "{w:?} missing label");
        }
    }
}
