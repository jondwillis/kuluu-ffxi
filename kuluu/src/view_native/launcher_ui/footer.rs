use bevy::asset::RenderAssetUsages;
use bevy::feathers::cursor::EntityCursor;
use bevy::feathers::theme::ThemedText;
use bevy::image::{CompressedImageFormats, ImageFormat, ImageSampler, ImageType, TextureError};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::window::SystemCursorIcon;

use super::client_era_check::{self, ClientEraStatus, EraVerdict};
use super::common::{hint, open_url, PANEL_BG, PANEL_BORDER_COLOR};
use crate::view_native::AppPhase;

const FOOTER_HEIGHT_PX: f32 = 34.0;
const FOOTER_PAD_X_PX: f32 = 16.0;
const FOOTER_TOP_BORDER_PX: f32 = 1.0;
const FOOTER_LINK_GAP_PX: f32 = 14.0;
const FOOTER_ICON_LABEL_GAP_PX: f32 = 6.0;
const FOOTER_ICON_PX: f32 = 16.0;
const FOOTER_LINK_PAD_X_PX: f32 = 8.0;
const FOOTER_LINK_PAD_Y_PX: f32 = 3.0;
const FOOTER_LINK_RADIUS_PX: f32 = 4.0;
/// Matches `common::hint`, so the labels sit on the version text's baseline.
const FOOTER_FONT_PX: f32 = 12.0;

/// Bottom inset the launcher screens reserve so a centered panel never slides
/// under the footer. Consumed by `common::screen_root` and `char_list`.
pub(super) const FOOTER_RESERVED_PX: f32 = FOOTER_HEIGHT_PX;
const _: () = assert!(FOOTER_RESERVED_PX >= FOOTER_HEIGHT_PX);

/// Above the launcher screen roots and the update banner (both implicit 0),
/// below the in-game HUD overlays at 10.
const FOOTER_Z: i32 = 5;

const DISCORD_BLURPLE: Color = Color::srgb_u8(0x58, 0x65, 0xF2);
const GITHUB_WHITE: Color = Color::srgb_u8(0xFF, 0xFF, 0xFF);
const PATREON_CORAL: Color = Color::srgb_u8(0xFF, 0x42, 0x4D);

const HOVER_LIGHTEN: f32 = 0.12;

/// Carries the hover affordance for GitHub, whose #FFFFFF mark cannot brighten
/// (`Luminance::lighter` clamps at white).
const FOOTER_LINK_HOVER_BG: Color = Color::srgba(1.0, 1.0, 1.0, 0.10);

const VERSION_SEP: &str = " | ";

// `[profile.dist]` inherits release, so dist and release both stamp "release".
// Intended: cargo's `PROFILE` is unreliable for custom profiles, and the useful
// distinction here is optimized-vs-not.
const BUILD_PROFILE: &str = if cfg!(debug_assertions) {
    "debug"
} else {
    "release"
};
const TARGET_TRIPLE: &str = env!("KULUU_TARGET_TRIPLE");

struct BrandLink {
    label: &'static str,
    url: &'static str,
    png: &'static [u8],
    tint: Color,
}

const BRAND_LINK_COUNT: usize = 3;

const BRAND_LINKS: [BrandLink; BRAND_LINK_COUNT] = [
    BrandLink {
        label: "Discord",
        url: "https://discord.gg/H2RdVASxfZ",
        png: include_bytes!("../../../assets/ui/brand/discord.png"),
        tint: DISCORD_BLURPLE,
    },
    BrandLink {
        label: "GitHub",
        url: "https://github.com/jondwillis/kuluu-ffxi",
        png: include_bytes!("../../../assets/ui/brand/github.png"),
        tint: GITHUB_WHITE,
    },
    BrandLink {
        label: "Patreon",
        url: "https://www.patreon.com/jondwillis",
        png: include_bytes!("../../../assets/ui/brand/patreon.png"),
        tint: PATREON_CORAL,
    },
];

#[derive(Component)]
struct LauncherFooter;

#[derive(Component)]
struct FooterInstallLabel;

fn install_status_text(era: &ClientEraStatus) -> String {
    let Some(install) = era.install.as_ref() else {
        return "Install: none selected".to_string();
    };
    let mut text = install.label();
    match (era.verdict, era.server_name.as_deref()) {
        (EraVerdict::Refused, Some(server)) => {
            text.push_str(&format!("{VERSION_SEP}era refused by {server}"));
        }
        (EraVerdict::Warn, Some(server)) => {
            text.push_str(&format!("{VERSION_SEP}era check: see {server} sign-in"));
        }
        _ => {}
    }
    text
}

#[derive(Component)]
struct FooterLink {
    idle: Color,
    hover: Color,
}

#[derive(Resource)]
struct BrandIcons([Handle<Image>; BRAND_LINK_COUNT]);

// `common::hint` already carries its own TextColor; a brand-tinted label has to
// build the bundle rather than layer a second one on top.
fn link_label(text: &'static str, tint: Color) -> impl Bundle {
    (
        Text::new(text),
        TextFont {
            font_size: FOOTER_FONT_PX.into(),
            ..default()
        },
        TextColor(tint),
        ThemedText,
    )
}

fn version_line(version: &str, profile: &str, target: &str) -> String {
    format!("v{version}{VERSION_SEP}{profile}{VERSION_SEP}{target}")
}

fn footer_version_text() -> String {
    version_line(env!("CARGO_PKG_VERSION"), BUILD_PROFILE, TARGET_TRIPLE)
}

fn decode_brand_icon(png: &[u8]) -> Result<Image, TextureError> {
    Image::from_buffer(
        png,
        ImageType::Format(ImageFormat::Png),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::default(),
    )
}

fn upload_brand_icons(images: &mut Assets<Image>) -> BrandIcons {
    BrandIcons(std::array::from_fn(|i| {
        let link = &BRAND_LINKS[i];
        match decode_brand_icon(link.png) {
            Ok(image) => images.add(image),
            // The label still renders and the link still opens; a corrupt
            // embedded asset must not brick the launcher.
            Err(e) => {
                tracing::warn!(error = %e, brand = link.label, "brand icon failed to decode");
                Handle::default()
            }
        }
    }))
}

fn spawn_footer(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    loaded: Option<Res<crate::view_native::DatRootRes>>,
) {
    let icons = upload_brand_icons(&mut images);
    let install = client_era_check::active_install(
        &crate::launcher_store::load().settings,
        loaded.as_ref().and_then(|r| r.0.as_deref()),
    );
    let install_text = install
        .map(|i| i.label())
        .unwrap_or_else(|| "Install: none selected".to_string());

    commands
        .spawn((
            LauncherFooter,
            GlobalZIndex(FOOTER_Z),
            BackgroundColor(PANEL_BG),
            BorderColor::all(PANEL_BORDER_COLOR),
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Px(FOOTER_HEIGHT_PX),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                padding: UiRect::horizontal(Val::Px(FOOTER_PAD_X_PX)),
                border: UiRect::top(Val::Px(FOOTER_TOP_BORDER_PX)),
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(FOOTER_LINK_GAP_PX),
                ..default()
            })
            .with_children(|left| {
                left.spawn(hint(footer_version_text()));
                left.spawn((FooterInstallLabel, hint(install_text)));
            });
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(FOOTER_LINK_GAP_PX),
                ..default()
            })
            .with_children(|links| {
                for (i, link) in BRAND_LINKS.iter().enumerate() {
                    links
                        .spawn((
                            FooterLink {
                                idle: link.tint,
                                hover: link.tint.lighter(HOVER_LIGHTEN),
                            },
                            Hovered::default(),
                            EntityCursor::System(SystemCursorIcon::Pointer),
                            BackgroundColor(Color::NONE),
                            Node {
                                flex_direction: FlexDirection::Row,
                                align_items: AlignItems::Center,
                                column_gap: Val::Px(FOOTER_ICON_LABEL_GAP_PX),
                                padding: UiRect::axes(
                                    Val::Px(FOOTER_LINK_PAD_X_PX),
                                    Val::Px(FOOTER_LINK_PAD_Y_PX),
                                ),
                                border_radius: BorderRadius::all(Val::Px(FOOTER_LINK_RADIUS_PX)),
                                ..default()
                            },
                        ))
                        .observe(move |_ev: On<Pointer<Click>>| open_url(link.url))
                        .with_children(|row| {
                            row.spawn((
                                ImageNode {
                                    color: link.tint,
                                    ..ImageNode::new(icons.0[i].clone())
                                },
                                // ImageNode's intrinsic size is the 48px source
                                // art, so this is load-bearing, not decorative.
                                Node {
                                    width: Val::Px(FOOTER_ICON_PX),
                                    height: Val::Px(FOOTER_ICON_PX),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ));
                            row.spawn((link_label(link.label, link.tint), Pickable::IGNORE));
                        });
                }
            });
        });

    commands.insert_resource(icons);
}

fn despawn_footer(mut commands: Commands, q: Query<Entity, With<LauncherFooter>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
    // Without this the three Image handles (and their GPU textures) outlive the
    // launcher for the rest of the process.
    commands.remove_resource::<BrandIcons>();
}

fn retint_footer_links(
    links: Query<(Entity, &FooterLink, &Hovered, &Children), Changed<Hovered>>,
    mut backgrounds: Query<&mut BackgroundColor>,
    mut icons: Query<&mut ImageNode>,
    mut labels: Query<&mut TextColor>,
) {
    for (entity, link, hovered, children) in links.iter() {
        let tint = if hovered.get() { link.hover } else { link.idle };
        if let Ok(mut bg) = backgrounds.get_mut(entity) {
            bg.0 = if hovered.get() {
                FOOTER_LINK_HOVER_BG
            } else {
                Color::NONE
            };
        }
        for child in children.iter() {
            if let Ok(mut icon) = icons.get_mut(child) {
                icon.color = tint;
            }
            if let Ok(mut label) = labels.get_mut(child) {
                label.0 = tint;
            }
        }
    }
}

fn refresh_install_label(
    era: Res<ClientEraStatus>,
    mut labels: Query<&mut Text, With<FooterInstallLabel>>,
) {
    if !era.is_changed() || era.install.is_none() {
        return;
    }
    let text = install_status_text(&era);
    for mut label in labels.iter_mut() {
        label.0 = text.clone();
    }
}

pub(super) fn register(app: &mut App) {
    app.add_systems(OnEnter(AppPhase::Launcher), spawn_footer)
        .add_systems(OnExit(AppPhase::Launcher), despawn_footer)
        .add_systems(
            Update,
            (retint_footer_links, refresh_install_label).run_if(in_state(AppPhase::Launcher)),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_MAGIC: &[u8] = b"\x89PNG";

    /// Source-art size every brand mask must ship at.
    const BRAND_ICON_SOURCE_PX: u32 = 48;

    #[test]
    fn version_line_formats_all_three_fields() {
        assert_eq!(
            version_line("0.4.0", "debug", "aarch64-apple-darwin"),
            "v0.4.0 | debug | aarch64-apple-darwin",
        );
    }

    #[test]
    fn install_status_text_names_the_server_only_when_the_era_check_flags_it() {
        use super::super::client_era_check::ActiveInstall;
        let mut era = ClientEraStatus {
            server_name: Some("lsb".into()),
            install: Some(ActiveInstall {
                name: "hxi".into(),
                client: "horizonxi-2023".into(),
                patch_version: Some("30230905_0".into()),
                known_retail: false,
            }),
            ..Default::default()
        };
        era.verdict = EraVerdict::Ok;
        assert_eq!(
            install_status_text(&era),
            "Install: hxi (horizonxi-2023, patch 30230905_0)"
        );
        era.verdict = EraVerdict::Refused;
        assert!(install_status_text(&era).ends_with(" | era refused by lsb"));
        era.install = None;
        assert_eq!(install_status_text(&era), "Install: none selected");
    }

    #[test]
    fn build_stamp_fields_are_populated() {
        // env! is a compile error when the var is missing, but not when it is
        // empty - this is what actually guards the build.rs contract.
        assert!(
            TARGET_TRIPLE.matches('-').count() >= 2,
            "not a target triple: {TARGET_TRIPLE:?}",
        );
        assert!(matches!(BUILD_PROFILE, "debug" | "release"));
    }

    #[test]
    fn brand_icons_decode_to_spec() {
        for link in BRAND_LINKS.iter() {
            assert_eq!(&link.png[..PNG_MAGIC.len()], PNG_MAGIC, "{}", link.label);
            let image = decode_brand_icon(link.png).expect(link.label);
            assert_eq!(image.width(), BRAND_ICON_SOURCE_PX, "{}", link.label);
            assert_eq!(image.height(), BRAND_ICON_SOURCE_PX, "{}", link.label);

            // ImageNode::color multiplies, so only a white mask reproduces the
            // brand hex exactly. A black or tinted mask would render wrong.
            let data = image.data.as_ref().expect("decoded image has pixel data");
            for px in data.chunks_exact(4) {
                if px[3] != 0 {
                    assert_eq!(
                        &px[..3],
                        &[255, 255, 255],
                        "{} mask is not white",
                        link.label
                    );
                }
            }
        }
    }

    #[test]
    fn brand_link_urls_are_https() {
        for link in BRAND_LINKS.iter() {
            assert!(
                link.url.starts_with("https://"),
                "{} url is not https: {}",
                link.label,
                link.url,
            );
        }
    }
}
