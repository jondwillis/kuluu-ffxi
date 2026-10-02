use bevy::asset::RenderAssetUsages;
use bevy::feathers::theme::ThemedText;
use bevy::image::{CompressedImageFormats, ImageFormat, ImageSampler, ImageType, TextureError};
use bevy::prelude::*;

const EMBLEM_PNG: &[u8] = include_bytes!("../../../assets/branding/png/kuluu-256.png");

const EMBLEM_PX: f32 = 72.0;
const WORDMARK_FONT_PX: f32 = 30.0;
const MARK_GAP_PX: f32 = 8.0;
const MARK_BOTTOM_MARGIN_PX: f32 = 16.0;
/// Matches `common::title`, so the wordmark reads as the same family as the
/// screen headings below it.
const WORDMARK_COLOR: Color = Color::srgb(0.0, 1.0, 1.0);

const WORDMARK: &str = "KULUU";

/// Uploaded once when the plugin is built rather than per screen: the login
/// screen rebuilds itself on every form change, and the launcher can be
/// re-entered after logout, so neither an `OnEnter` upload nor a per-spawn
/// decode fits. A `PreStartup` system is too late - `bevy_state` runs the
/// initial `StateTransition` there too, so `OnEnter(Login)` can beat it.
#[derive(Resource, Default)]
pub(super) struct BrandMark(Option<Handle<Image>>);

fn decode_emblem() -> Result<Image, TextureError> {
    Image::from_buffer(
        EMBLEM_PNG,
        ImageType::Format(ImageFormat::Png),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::default(),
    )
}

/// Uploads the embedded emblem. On decode failure the wordmark still
/// renders; a corrupt embedded asset must not brick the launcher.
fn upload_brand_mark(app: &mut App) -> BrandMark {
    let Some(mut images) = app.world_mut().get_resource_mut::<Assets<Image>>() else {
        tracing::warn!("no image assets yet; launcher emblem omitted");
        return BrandMark(None);
    };
    match decode_emblem() {
        Ok(image) => BrandMark(Some(images.add(image))),
        Err(e) => {
            tracing::warn!(error = %e, "launcher emblem failed to decode");
            BrandMark(None)
        }
    }
}

/// The emblem plus wordmark retail shows above its world-selection screen.
/// Spawn as the first child of a `common::screen_root`, before the panel.
/// The emblem's explicit size is load-bearing, not decorative: ImageNode's
/// intrinsic size is the 256px source art.
pub(super) fn spawn_brand_mark(parent: &mut ChildSpawnerCommands, mark: &BrandMark) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            row_gap: Val::Px(MARK_GAP_PX),
            margin: UiRect::bottom(Val::Px(MARK_BOTTOM_MARGIN_PX)),
            flex_shrink: 0.0,
            ..default()
        })
        .with_children(|column| {
            if let Some(emblem) = &mark.0 {
                column.spawn((
                    ImageNode::new(emblem.clone()),
                    Node {
                        width: Val::Px(EMBLEM_PX),
                        height: Val::Px(EMBLEM_PX),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            }
            column.spawn((
                Text::new(WORDMARK),
                TextFont {
                    font_size: WORDMARK_FONT_PX.into(),
                    ..default()
                },
                TextColor(WORDMARK_COLOR),
                ThemedText,
                Pickable::IGNORE,
            ));
        });
}

pub(super) fn register(app: &mut App) {
    let mark = upload_brand_mark(app);
    app.insert_resource(mark);
}
