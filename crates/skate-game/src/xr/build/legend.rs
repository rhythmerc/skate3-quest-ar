//! Build mode's legend: beside each controller, what each of its sticks and
//! buttons does right now. It follows what is being done (pointing at a
//! piece, holding the left grip for the second set of actions), so the
//! controls don't have to be learned from a page first; a row is only there
//! while its action is. "Build hints" in the menu (Build a park) turns it off.
//!
//! Drawn as the trick readout is (`xr::ui::hud`): a small UI camera into an
//! image on a quad, one per hand, kept beside the controller and turned to
//! the eyes.
use super::BuildMode;
use crate::xr::actions::Hand;
use crate::xr::ar_settings::ArSettings;
use crate::xr::panel::{XrPanel, XrPanelCamera};
use crate::xr::rig::Rig;
use bevy::camera::RenderTarget;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy_mod_xr::camera::XrCamera;

/// The most rows a hand shows.
pub(crate) const ROWS: usize = 8;
const WIDTH_PX: u32 = 600;
const ROW_PX: f32 = 46.0;
const HEIGHT_PX: u32 = (ROW_PX as u32) * ROWS as u32 + 24;
/// The panel's real width, metres.
const WIDTH_M: f32 = 0.12;
/// Where a panel's middle is from its controller's aim pose, metres: out to
/// the hand's own side, level with the grip. (The left one mirrors x.)
const OFFSET: Vec3 = Vec3::new(0.095, -0.005, 0.06);

/// One control and what it does now.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    /// The control, as it is named on every row for it ("Stick ↔", "Grip").
    pub control: &'static str,
    pub action: String,
}

impl Row {
    pub(crate) fn new(control: &'static str, action: impl Into<String>) -> Self {
        Self { control, action: action.into() }
    }
}

/// What the legend shows; build mode writes it every frame.
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub(crate) struct Legend {
    pub left: Vec<Row>,
    pub right: Vec<Row>,
}

#[derive(Component, Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
}

#[derive(Component)]
struct LegendQuad;
#[derive(Component)]
struct LegendCamera;
#[derive(Component)]
struct LegendRow(usize);
#[derive(Component)]
struct LegendControl(usize);
#[derive(Component)]
struct LegendAction(usize);

pub(super) struct LegendPlugin;

impl Plugin for LegendPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Legend>()
            .add_systems(Startup, setup)
            .add_systems(Update, draw)
            .add_systems(PostUpdate, place.before(bevy::transform::TransformSystems::Propagate));
    }
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
) {
    let font: Handle<Font> = assets.load(crate::xr::ui::UI_FONT);
    let text = |size: f32, weight: u16| TextFont { font: font.clone(), font_size: size, weight: FontWeight(weight), ..default() };
    let quad = meshes.add(Rectangle::new(WIDTH_M, WIDTH_M * HEIGHT_PX as f32 / WIDTH_PX as f32));
    for side in [Side::Left, Side::Right] {
        let mut image = Image::new_target_texture(WIDTH_PX, HEIGHT_PX, TextureFormat::Rgba8UnormSrgb, None);
        image.sampler = ImageSampler::linear();
        let image = images.add(image);
        let camera = commands
            .spawn((
                XrPanelCamera,
                LegendCamera,
                side,
                Camera2d,
                Camera { order: -12, clear_color: ClearColorConfig::Custom(Color::NONE), is_active: false, ..default() },
                RenderTarget::Image(image.clone().into()),
            ))
            .id();
        commands.spawn((
            LegendQuad,
            side,
            Mesh3d(quad.clone()),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color_texture: Some(image),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                double_sided: true,
                cull_mode: None,
                ..default()
            })),
            Transform::default(),
            Visibility::Hidden,
        ));
        commands
            .spawn((
                Node {
                    width: percent(100),
                    height: percent(100),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::FlexStart,
                    padding: UiRect::all(px(12)),
                    ..default()
                },
                UiTargetCamera(camera),
            ))
            .with_children(|panel| {
                for i in 0..ROWS {
                    panel
                        .spawn((
                            LegendRow(i),
                            side,
                            Node {
                                height: px(ROW_PX - 6.0),
                                margin: UiRect::bottom(px(6)),
                                padding: UiRect::horizontal(px(12)),
                                align_items: AlignItems::Center,
                                border_radius: BorderRadius::all(px(10)),
                                ..default()
                            },
                            BackgroundColor(Color::srgba(0.03, 0.05, 0.07, 0.82)),
                        ))
                        .with_children(|row| {
                            row.spawn((
                                LegendControl(i),
                                side,
                                Text::new(""),
                                text(25.0, 800),
                                TextColor(Color::srgb(0.45, 0.9, 0.82)),
                                Node { width: px(195), ..default() },
                            ));
                            row.spawn((LegendAction(i), side, Text::new(""), text(25.0, 500), TextColor(Color::WHITE)));
                        });
                }
            });
    }
}

#[allow(clippy::type_complexity)]
fn draw(
    legend: Res<Legend>,
    mut drawn: Local<Option<Legend>>,
    mut rows: Query<(&LegendRow, &Side, &mut Node)>,
    mut texts: ParamSet<(Query<(&LegendControl, &Side, &mut Text)>, Query<(&LegendAction, &Side, &mut Text)>)>,
) {
    if drawn.as_ref() == Some(&*legend) {
        return;
    }
    *drawn = Some(legend.clone());
    let of = |side: &Side| if *side == Side::Left { &legend.left } else { &legend.right };
    for (row, side, mut node) in &mut rows {
        let display = if row.0 < of(side).len() { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    for (slot, side, mut text) in &mut texts.p0() {
        **text = of(side).get(slot.0).map_or(String::new(), |r| r.control.to_string());
    }
    for (slot, side, mut text) in &mut texts.p1() {
        **text = of(side).get(slot.0).map_or(String::new(), |r| r.action.clone());
    }
}

#[allow(clippy::type_complexity)]
fn place(
    rig: Res<Rig>,
    mode: Res<BuildMode>,
    settings: Res<ArSettings>,
    panel: Res<XrPanel>,
    legend: Res<Legend>,
    hands: Query<(&Hand, &Transform), (Without<LegendQuad>, Without<XrCamera>)>,
    eyes: Query<&Transform, (With<XrCamera>, Without<LegendQuad>)>,
    mut quads: Query<(&Side, &mut Transform, &mut Visibility), With<LegendQuad>>,
    mut cameras: Query<(&Side, &mut Camera), With<LegendCamera>>,
) {
    let head = (eyes.iter().count() > 0).then(|| eyes.iter().map(|e| e.translation).sum::<Vec3>() / eyes.iter().count() as f32);
    let hand = |side: Side| {
        hands
            .iter()
            .find(|(h, _)| matches!((h, side), (Hand::Left, Side::Left) | (Hand::Right, Side::Right)))
            .map(|(_, t)| *t)
    };
    let on = mode.active && settings.build_hints && !panel.visible;
    let rows = |side: Side| if side == Side::Left { legend.left.len() } else { legend.right.len() };
    for (side, mut camera) in &mut cameras {
        let show = on && rows(*side) > 0 && hand(*side).is_some();
        if camera.is_active != show {
            camera.is_active = show;
        }
    }
    for (side, mut transform, mut visibility) in &mut quads {
        let (Some(root), Some(head), Some(hand), true) = (rig.root_transform(), head, hand(*side), on && rows(*side) > 0) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let offset = if *side == Side::Left { OFFSET * Vec3::new(-1.0, 1.0, 1.0) } else { OFFSET };
        let at = hand.transform_point(offset);
        // Its front (+Z) to the eyes, upright in the room.
        let pose = Transform::from_translation(at).looking_to(at - head, Vec3::Y);
        let target = root.mul_transform(pose);
        if *transform != target {
            *transform = target;
        }
        visibility.set_if_neq(Visibility::Visible);
    }
}
