//! Real things hide the skater and the park ("occlusion"), with Meta's
//! depth API (`XR_META_environment_depth`): the headset gives a depth image
//! of the room for each eye, and after the scene is drawn a pass over each
//! eye view takes out the pixels whose place in the room is behind what is
//! really there (`occlusion.wgsl`).
//!
//! Off unless the AR settings turn it on (the pass costs GPU time). Hard:
//! one depth sample a pixel, the cheap one, its edges stepped along the
//! depth image's texels. Soft: the way Meta's own shaders do it, each new
//! depth image filtered once (`occlusion_filter.wgsl`: the means of the
//! sixteen texels about each) and read once a pixel, which is steadier
//! from frame to frame and nearly as cheap. Soft, legacy: what soft was
//! before, sixteen samples a pixel, kept to tell the two apart. Only solid things are hidden: the
//! menu, the build legend and other see-through draws write no depth and
//! stay in front of everything.
//!
//! A bowl is under the real surface it is sunk into, which would hide all
//! of it: what is drawn below a bowl's mouth is instead hidden only by what
//! is really in front of the mouth (`Wanted::holes`).
//!
//! The depth provider is started the first time the setting is on and then
//! left running until the app closes: stopped and started again, the
//! headset's images came back wrong.
use super::ar_settings::ArSettings;
use super::rig::Rig;
use bevy::core_pipeline::FullscreenShader;
use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
use bevy::ecs::query::QueryItem;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_graph::{NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner};
use bevy::render::render_resource::binding_types::{sampler, texture_2d_array, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent, BlendFactor, BlendOperation, BlendState,
    CachedRenderPipelineId, ColorTargetState, ColorWrites, FragmentState, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType, SpecializedRenderPipeline, SpecializedRenderPipelines,
    Texture, TextureAspect, TextureFormat, TextureSampleType, TextureUsages, TextureView, TextureViewDescriptor, TextureViewDimension,
    UniformBuffer,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue};
use bevy::render::view::{ExtractedView, ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy_mod_openxr::resources::OxrFrameState;
use bevy_mod_openxr::session::OxrSession;
use bevy_mod_openxr::spaces::OxrSpaceExt;
use bevy_mod_xr::camera::XrCamera;
use bevy_mod_xr::spaces::XrPrimaryReferenceSpace;
use ash::vk::Handle as _;
use openxr::sys;
use openxr::sys::Handle as _;
use serde::{Deserialize, Serialize};

/// How real things hide virtual ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Occlusion {
    #[default]
    Off,
    /// One depth sample a pixel: quick, with stepped edges.
    Hard,
    /// One sample a pixel of the depth image filtered: smooth, steady edges.
    Soft,
    /// Sixteen samples a pixel, feathered: what soft was, at a cost in
    /// frames. (Here to compare with the new one.)
    SoftLegacy,
}

impl Occlusion {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Off => Self::Hard,
            Self::Hard => Self::Soft,
            Self::Soft => Self::SoftLegacy,
            Self::SoftLegacy => Self::Off,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Hard => "on, hard edges (quick)",
            Self::Soft => "on, soft edges",
            Self::SoftLegacy => "on, soft edges, legacy (slower)",
        }
    }
}

/// What the render world needs each frame: the setting, and the XR rig's
/// root (the game world from the room's tracking space).
#[derive(Resource, Clone, Copy, Default, ExtractResource)]
struct Wanted {
    mode: Occlusion,
    world_from_tracking: Mat4,
    /// The mouths of the sunk pieces (bowls) nearest the head: the centre,
    /// in the game world, and the radius about it.
    holes: [Vec4; HOLES],
    hole_count: u32,
}

/// How many bowls' mouths the pass knows of.
const HOLES: usize = 4;
/// What is this far under a mouth's height is in the bowl, m.
const BELOW_M: f32 = 0.01;

/// Why occlusion can't run here, for the settings page (none: it can, or
/// it hasn't been tried).
#[derive(Resource, Default, Clone)]
pub(crate) struct OcclusionProblem(pub std::sync::Arc<std::sync::Mutex<Option<&'static str>>>);

pub(crate) struct OcclusionPlugin;

impl Plugin for OcclusionPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "occlusion.wgsl");
        bevy::asset::embedded_asset!(app, "occlusion_filter.wgsl");
        let problem = OcclusionProblem::default();
        app.init_resource::<Wanted>()
            .insert_resource(problem.clone())
            .add_plugins(ExtractResourcePlugin::<Wanted>::default())
            .add_systems(PostUpdate, want)
            .add_observer(readable_eye_depth);
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        render
            .insert_resource(problem)
            .init_resource::<RoomDepth>()
            .init_resource::<SpecializedRenderPipelines<OcclusionPipeline>>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(
                Render,
                (
                    acquire.in_set(RenderSystems::PrepareResources).run_if(resource_exists::<OxrSession>),
                    prepare.in_set(RenderSystems::Prepare),
                ),
            )
            .add_render_graph_node::<ViewNodeRunner<OcclusionNode>>(Core3d, OcclusionLabel)
            .add_render_graph_edges(Core3d, (Node3d::EndMainPass, OcclusionLabel, Node3d::Tonemapping));
    }
}

/// The extension this needs, asked for when the session is made.
pub(crate) fn enable(exts: &mut openxr::ExtensionSet) {
    exts.meta_environment_depth = true;
}

fn want(
    settings: Res<ArSettings>,
    rig: Res<Rig>,
    placed: Res<super::build::PlacedPieces>,
    heads: Query<&GlobalTransform, With<XrCamera>>,
    mut wanted: ResMut<Wanted>,
) {
    let root = rig.root_transform();
    wanted.mode = if root.is_some() { settings.occlusion } else { Occlusion::Off };
    wanted.world_from_tracking = root.map_or(Mat4::IDENTITY, |r| r.to_matrix());
    wanted.hole_count = 0;
    let (Some(root), true) = (root, wanted.mode != Occlusion::Off) else { return };
    let head = heads.iter().next().map_or(root.translation, |t| t.translation());
    let mut holes: Vec<Vec4> = placed
        .items
        .iter()
        .filter(|item| item.piece.kind.sunk())
        .map(|item| {
            let at = item.world_transform(&root, placed.park_scale);
            let (min, max) = item.geometry().footprint();
            let centre = at.transform_point(Vec3::new((min.x + max.x) * 0.5, 0.0, (min.y + max.y) * 0.5));
            centre.extend((max - min).length() * 0.5 * at.scale.x)
        })
        .collect();
    holes.sort_by(|a, b| a.truncate().distance_squared(head).total_cmp(&b.truncate().distance_squared(head)));
    for (slot, hole) in wanted.holes.iter_mut().zip(&holes) {
        *slot = *hole;
    }
    wanted.hole_count = holes.len().min(HOLES) as u32;
}

/// The pass reads the eye views' depth: their depth textures must allow it,
/// from before they are first made.
fn readable_eye_depth(add: On<Add, XrCamera>, mut cameras: Query<&mut Camera3d>) {
    if let Ok(mut camera) = cameras.get_mut(add.entity) {
        camera.depth_texture_usages = (TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING).into();
    }
}

/// The depth provider and its images, once made, and the latest image.
#[derive(Resource, Default)]
struct RoomDepth {
    made: Option<Made>,
    /// Making it failed: not tried again.
    failed: bool,
    running: bool,
    latest: Option<Latest>,
    eyes: [Option<UniformBuffer<Eye>>; 2],
    logged: bool,
    /// The pass's pipeline failed (said once).
    broken: bool,
}

struct Made {
    provider: sys::EnvironmentDepthProviderMETA,
    /// Each swapchain image, both eyes' layers. (Kept for as long as the
    /// app runs: the runtime owns the images.)
    views: Vec<(Texture, TextureView)>,
    /// Each image's eyes, one at a time (what the filter reads).
    layers: Vec<[TextureView; 2]>,
    /// The latest image filtered for soft occlusion: both eyes, and each.
    filtered: (Texture, TextureView, [TextureView; 2]),
    sampler: Sampler,
}

#[derive(Clone, Copy)]
struct Latest {
    image: usize,
    near: f32,
    far: f32,
    /// Each eye's depth view: the room's tracking space to it, and its
    /// edges' tangents (left, right, down, up).
    views: [(Mat4, Vec4); 2],
}

#[derive(ShaderType, Clone, Copy, Default)]
struct Eye {
    view_from_world: Mat4,
    tangents: Vec4,
    near: f32,
    far: f32,
    layer: u32,
    hole_count: u32,
    /// [`BELOW_M`] in the game world's units.
    below: f32,
    /// 1: [`Occlusion::Hard`]. 2: [`Occlusion::Soft`]. 0: [`Occlusion::SoftLegacy`].
    mode: u32,
    holes: [Vec4; HOLES],
}

/// The filtered image's format: halves, so the far end of the depth range
/// (kept as 1 - depth) loses nothing.
const FILTERED: TextureFormat = TextureFormat::Rgba16Float;

fn ok(result: sys::Result, what: &'static str) -> Result<(), &'static str> {
    if result.into_raw() >= 0 { Ok(()) } else { Err(what) }
}

/// Makes the provider and wraps its images for the renderer.
fn make(session: &OxrSession, device: &RenderDevice) -> Result<Made, &'static str> {
    let fns = session.instance().exts().meta_environment_depth.ok_or("This headset has no depth sensing for apps.")?;
    unsafe {
        let mut provider = sys::EnvironmentDepthProviderMETA::NULL;
        let info = sys::EnvironmentDepthProviderCreateInfoMETA {
            ty: sys::EnvironmentDepthProviderCreateInfoMETA::TYPE,
            next: std::ptr::null(),
            create_flags: sys::EnvironmentDepthProviderCreateFlagsMETA::EMPTY,
        };
        ok((fns.create_environment_depth_provider)(session.as_raw(), &info, &mut provider), "The headset wouldn't start depth sensing (is Space Setup allowed?).")?;
        let mut swapchain = sys::EnvironmentDepthSwapchainMETA::NULL;
        let info = sys::EnvironmentDepthSwapchainCreateInfoMETA {
            ty: sys::EnvironmentDepthSwapchainCreateInfoMETA::TYPE,
            next: std::ptr::null(),
            create_flags: sys::EnvironmentDepthSwapchainCreateFlagsMETA::EMPTY,
        };
        ok((fns.create_environment_depth_swapchain)(provider, &info, &mut swapchain), "The headset gave no depth images.")?;
        let mut state = sys::EnvironmentDepthSwapchainStateMETA {
            ty: sys::EnvironmentDepthSwapchainStateMETA::TYPE,
            next: std::ptr::null_mut(),
            width: 0,
            height: 0,
        };
        ok((fns.get_environment_depth_swapchain_state)(swapchain, &mut state), "The headset gave no depth images.")?;
        let mut count = 0;
        ok((fns.enumerate_environment_depth_swapchain_images)(swapchain, 0, &mut count, std::ptr::null_mut()), "The headset gave no depth images.")?;
        let mut images = vec![
            sys::SwapchainImageVulkanKHR { ty: sys::SwapchainImageVulkanKHR::TYPE, next: std::ptr::null_mut(), image: 0 };
            count as usize
        ];
        ok(
            (fns.enumerate_environment_depth_swapchain_images)(swapchain, count, &mut count, images.as_mut_ptr().cast()),
            "The headset gave no depth images.",
        )?;
        let size = wgpu::Extent3d { width: state.width, height: state.height, depth_or_array_layers: 2 };
        let wgpu_device = device.wgpu_device();
        let mut views = Vec::new();
        let mut layers = Vec::new();
        for image in &images {
            let hal = {
                let hal_device = wgpu_device.as_hal::<wgpu_hal::vulkan::Api>().ok_or("Occlusion needs the Vulkan renderer.")?;
                hal_device.texture_from_raw(
                    ash::vk::Image::from_raw(image.image),
                    &wgpu_hal::TextureDescriptor {
                        label: Some("room depth"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: TextureFormat::Depth16Unorm,
                        usage: wgpu::TextureUses::RESOURCE,
                        memory_flags: wgpu_hal::MemoryFlags::empty(),
                        view_formats: vec![],
                    },
                    // (the runtime's image: never ours to destroy)
                    Some(Box::new(|| {})),
                )
            };
            let texture = wgpu_device.create_texture_from_hal::<wgpu_hal::vulkan::Api>(
                hal,
                &wgpu::TextureDescriptor {
                    label: Some("room depth"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: TextureFormat::Depth16Unorm,
                    usage: TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            );
            let texture = Texture::from(texture);
            let view = texture.create_view(&TextureViewDescriptor {
                dimension: Some(TextureViewDimension::D2Array),
                aspect: TextureAspect::DepthOnly,
                ..default()
            });
            layers.push([0, 1].map(|layer| {
                texture.create_view(&TextureViewDescriptor {
                    dimension: Some(TextureViewDimension::D2),
                    aspect: TextureAspect::DepthOnly,
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..default()
                })
            }));
            views.push((texture, view));
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("room depth, filtered"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FILTERED,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let both = texture.create_view(&TextureViewDescriptor { dimension: Some(TextureViewDimension::D2Array), ..default() });
        let each = [0, 1].map(|layer| {
            texture.create_view(&TextureViewDescriptor {
                dimension: Some(TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..default()
            })
        });
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("room depth, filtered"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..default()
        });
        info!("Occlusion: room depth {}x{}, {} images", state.width, state.height, views.len());
        Ok(Made { provider, views, layers, filtered: (texture, both, each), sampler })
    }
}

/// Starts the provider the first time the setting is on, and takes the
/// latest depth image for each frame it is.
fn acquire(
    wanted: Option<Res<Wanted>>,
    session: Res<OxrSession>,
    frame: Option<Res<OxrFrameState>>,
    space: Option<Res<XrPrimaryReferenceSpace>>,
    device: Res<RenderDevice>,
    problem: Res<OcclusionProblem>,
    mut depth: ResMut<RoomDepth>,
) {
    let on = wanted.is_some_and(|w| w.mode != Occlusion::Off);
    if on && depth.made.is_none() && !depth.failed {
        match make(&session, &device) {
            Ok(made) => depth.made = Some(made),
            Err(why) => {
                warn!("Occlusion: {why}");
                depth.failed = true;
                *problem.0.lock().unwrap() = Some(why);
            }
        }
    }
    let Some(fns) = session.instance().exts().meta_environment_depth else { return };
    let Some(provider) = depth.made.as_ref().map(|m| m.provider) else { return };
    if !on {
        return;
    }
    if !depth.running {
        let result = unsafe { (fns.start_environment_depth_provider)(provider) };
        info!("Occlusion: depth sensing started ({result:?})");
        depth.running = result.into_raw() >= 0;
        if !depth.running {
            if !depth.failed {
                *problem.0.lock().unwrap() = Some("The headset wouldn't start depth sensing.");
            }
            depth.failed = true;
            return;
        }
    }
    let (true, Some(frame), Some(space)) = (depth.running, frame, space) else { return };
    let view = sys::EnvironmentDepthImageViewMETA {
        ty: sys::EnvironmentDepthImageViewMETA::TYPE,
        next: std::ptr::null(),
        fov: sys::Fovf { angle_left: 0.0, angle_right: 0.0, angle_up: 0.0, angle_down: 0.0 },
        pose: sys::Posef::IDENTITY,
    };
    let mut image = sys::EnvironmentDepthImageMETA {
        ty: sys::EnvironmentDepthImageMETA::TYPE,
        next: std::ptr::null(),
        swapchain_index: 0,
        near_z: 0.0,
        far_z: 0.0,
        views: [view; 2],
    };
    let info = sys::EnvironmentDepthImageAcquireInfoMETA {
        ty: sys::EnvironmentDepthImageAcquireInfoMETA::TYPE,
        next: std::ptr::null(),
        space: space.as_raw_openxr_space(),
        display_time: frame.predicted_display_time,
    };
    let result = unsafe { (fns.acquire_environment_depth_image)(provider, &info, &mut image) };
    // (not available yet, or this frame: the last image stands)
    if result != sys::Result::SUCCESS {
        return;
    }
    let views = image.views.map(|v| {
        let (q, p) = (v.pose.orientation, v.pose.position);
        let tracking_from_view = Mat4::from_rotation_translation(Quat::from_xyzw(q.x, q.y, q.z, q.w), Vec3::new(p.x, p.y, p.z));
        let tangents = Vec4::new(v.fov.angle_left.tan(), v.fov.angle_right.tan(), v.fov.angle_down.tan(), v.fov.angle_up.tan());
        (tracking_from_view.inverse(), tangents)
    });
    if !depth.logged {
        depth.logged = true;
        info!("Occlusion: first depth image (near {}, far {}, image {})", image.near_z, image.far_z, image.swapchain_index);
    }
    depth.latest = Some(Latest { image: image.swapchain_index as usize, near: image.near_z, far: image.far_z, views });
}

#[derive(Resource)]
struct OcclusionPipeline {
    layouts: [BindGroupLayoutDescriptor; 2],
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
    /// The pass that filters the room's image for soft occlusion.
    filter_layout: BindGroupLayoutDescriptor,
    filter: CachedRenderPipelineId,
}

#[derive(PartialEq, Eq, Hash, Clone, Copy)]
struct PipelineKey {
    format: TextureFormat,
    multisampled: bool,
}

#[derive(Component)]
struct OcclusionPipelineId(CachedRenderPipelineId, bool);

fn init_pipeline(mut commands: Commands, fullscreen: Res<FullscreenShader>, assets: Res<AssetServer>, cache: Res<PipelineCache>) {
    let layout = |multisampled: bool| {
        let entries = (
            uniform_buffer::<ViewUniform>(true),
            if multisampled { texture_depth_2d_multisampled() } else { texture_depth_2d() },
            texture_2d_array(TextureSampleType::Depth),
            uniform_buffer::<Eye>(false),
            texture_2d_array(TextureSampleType::Float { filterable: true }),
            sampler(SamplerBindingType::Filtering),
        );
        BindGroupLayoutDescriptor::new("occlusion", &BindGroupLayoutEntries::sequential(ShaderStages::FRAGMENT, entries))
    };
    let filter_layout =
        BindGroupLayoutDescriptor::new("occlusion filter", &BindGroupLayoutEntries::single(ShaderStages::FRAGMENT, texture_depth_2d()));
    let filter = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("occlusion filter".into()),
        layout: vec![filter_layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: assets.load("embedded://skate_game/xr/occlusion_filter.wgsl"),
            targets: vec![Some(ColorTargetState { format: FILTERED, blend: None, write_mask: ColorWrites::ALL })],
            ..default()
        }),
        ..default()
    });
    commands.insert_resource(OcclusionPipeline {
        filter_layout,
        filter,
        layouts: [layout(false), layout(true)],
        fullscreen: fullscreen.clone(),
        shader: assets.load("embedded://skate_game/xr/occlusion.wgsl"),
    });
}

impl SpecializedRenderPipeline for OcclusionPipeline {
    type Key = PipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        // What is there is multiplied by what the pass says is seen.
        let keep = BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::Src, operation: BlendOperation::Add };
        let keep_alpha = BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::SrcAlpha, operation: BlendOperation::Add };
        RenderPipelineDescriptor {
            label: Some("occlusion".into()),
            layout: vec![self.layouts[key.multisampled as usize].clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: if key.multisampled { vec!["MULTISAMPLED".into()] } else { vec![] },
                targets: vec![Some(ColorTargetState {
                    format: key.format,
                    blend: Some(BlendState { color: keep, alpha: keep_alpha }),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}

/// Each eye view's pipeline, and each eye's uniform for this frame.
fn prepare(
    mut commands: Commands,
    wanted: Option<Res<Wanted>>,
    cache: Res<PipelineCache>,
    pipeline: Res<OcclusionPipeline>,
    mut pipelines: ResMut<SpecializedRenderPipelines<OcclusionPipeline>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut depth: ResMut<RoomDepth>,
    problem: Res<OcclusionProblem>,
    views: Query<(Entity, &ExtractedView, &Msaa), With<XrCamera>>,
) {
    let (Some(wanted), Some(latest)) = (wanted.filter(|w| w.mode != Occlusion::Off), depth.latest) else { return };
    for (entity, view, msaa) in &views {
        let multisampled = msaa.samples() > 1;
        let format = if view.hdr { ViewTarget::TEXTURE_FORMAT_HDR } else { TextureFormat::bevy_default() };
        let id = pipelines.specialize(&cache, &pipeline, PipelineKey { format, multisampled });
        // (a pass that can't be made is otherwise only a line in the log)
        if let bevy::render::render_resource::CachedPipelineState::Err(error) = cache.get_render_pipeline_state(id) {
            if !depth.broken {
                depth.broken = true;
                error!("Occlusion: the pass can't be made: {error}");
                *problem.0.lock().unwrap() = Some("Occlusion's drawing pass failed to start (see the log).");
            }
        }
        commands.entity(entity).insert(OcclusionPipelineId(id, multisampled));
    }
    let tracking_from_world = wanted.world_from_tracking.inverse();
    for layer in 0..2 {
        let (view_from_tracking, tangents) = latest.views[layer];
        let eye = Eye {
            view_from_world: view_from_tracking * tracking_from_world,
            tangents,
            near: latest.near,
            far: if latest.far.is_finite() { latest.far } else { 0.0 },
            layer: layer as u32,
            hole_count: wanted.hole_count,
            mode: match wanted.mode {
                Occlusion::Hard => 1,
                Occlusion::Soft => 2,
                _ => 0,
            },
            below: BELOW_M * wanted.world_from_tracking.x_axis.truncate().length(),
            holes: wanted.holes,
        };
        let buffer = depth.eyes[layer].get_or_insert_with(UniformBuffer::default);
        buffer.set(eye);
        buffer.write_buffer(&device, &queue);
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct OcclusionLabel;

#[derive(Default)]
struct OcclusionNode;

impl ViewNode for OcclusionNode {
    type ViewQuery = (&'static ViewTarget, &'static ViewDepthTexture, &'static ViewUniformOffset, &'static XrCamera, &'static OcclusionPipelineId);

    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        context: &mut RenderContext,
        (target, scene_depth, view_offset, camera, pipeline_id): QueryItem<Self::ViewQuery>,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let depth = world.resource::<RoomDepth>();
        let mode = world.get_resource::<Wanted>().map_or(Occlusion::Off, |w| w.mode);
        let on = mode != Occlusion::Off;
        let (true, Some(latest), Some(made)) = (on, depth.latest, depth.made.as_ref()) else { return Ok(()) };
        let cache = world.resource::<PipelineCache>();
        let pipeline = world.resource::<OcclusionPipeline>();
        let (Some(render_pipeline), Some(view_uniforms), Some((_, room)), Some(eye)) = (
            cache.get_render_pipeline(pipeline_id.0),
            world.resource::<ViewUniforms>().uniforms.binding(),
            made.views.get(latest.image),
            depth.eyes.get(camera.0 as usize).and_then(|e| e.as_ref()).and_then(|e| e.binding()),
        ) else {
            return Ok(());
        };
        let bind_group = context.render_device().create_bind_group(
            "occlusion",
            &cache.get_bind_group_layout(&pipeline.layouts[pipeline_id.1 as usize]),
            &BindGroupEntries::sequential((view_uniforms, scene_depth.view(), room, eye, &made.filtered.1, &made.sampler)),
        );
        // Soft: this eye's half of the room's image is filtered first. (Each
        // frame, not each new image: it is a small image, and the headset
        // doesn't say which are new.)
        let layer = (camera.0 as usize).min(1);
        if let (Occlusion::Soft, Some(filter), Some(layers)) = (mode, cache.get_render_pipeline(pipeline.filter), made.layers.get(latest.image)) {
            let bind_group = context.render_device().create_bind_group(
                "occlusion filter",
                &cache.get_bind_group_layout(&pipeline.filter_layout),
                &BindGroupEntries::single(&layers[layer]),
            );
            let mut pass = context.command_encoder().begin_render_pass(&RenderPassDescriptor {
                label: Some("occlusion filter"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &made.filtered.2[layer],
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(filter);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        let mut pass = context.command_encoder().begin_render_pass(&RenderPassDescriptor {
            label: Some("occlusion"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: target.main_texture_view(),
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(render_pipeline);
        pass.set_bind_group(0, &bind_group, &[view_offset.offset]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_goes_round_and_reads_old_files() {
        assert_eq!(Occlusion::Off.next(), Occlusion::Hard, "the quick one comes first");
        assert_eq!(Occlusion::Hard.next(), Occlusion::Soft);
        assert_eq!(Occlusion::Soft.next(), Occlusion::SoftLegacy, "the old soft is still there to compare");
        assert_eq!(Occlusion::SoftLegacy.next(), Occlusion::Off);
        assert_eq!(Occlusion::default(), Occlusion::Off);
        for (old, mode) in [("\"Soft\"", Occlusion::Soft), ("\"Hard\"", Occlusion::Hard), ("\"Off\"", Occlusion::Off)] {
            assert_eq!(serde_json::from_str::<Occlusion>(old).unwrap(), mode);
        }
    }

    /// The pass's shader is valid WGSL, with and without MSAA. (A shader
    /// that isn't only shows as a log line on the headset, and no occlusion.)
    #[test]
    fn the_filter_shader_is_valid() {
        let text = include_str!("occlusion_filter.wgsl");
        let module = wgpu::naga::front::wgsl::parse_str(text).unwrap_or_else(|e| panic!("{}", e.emit_to_string(text)));
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(text)));
    }

    #[test]
    fn the_shader_is_valid() {
        let source = include_str!("occlusion.wgsl");
        for multisampled in [false, true] {
            // What Bevy's preprocessor would do: the import, and the #ifdef.
            let mut text = String::from("struct View { world_from_clip: mat4x4<f32>, viewport: vec4<f32>, world_position: vec3<f32> }\n");
            let mut keep = true;
            for line in source.lines() {
                match line.trim() {
                    l if l.starts_with("#import") => {}
                    "#ifdef MULTISAMPLED" => keep = multisampled,
                    "#else" => keep = !multisampled,
                    "#endif" => keep = true,
                    _ if keep => {
                        text.push_str(line);
                        text.push('\n');
                    }
                    _ => {}
                }
            }
            let module = wgpu::naga::front::wgsl::parse_str(&text).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&text)));
            wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&text)));
        }
    }
}
