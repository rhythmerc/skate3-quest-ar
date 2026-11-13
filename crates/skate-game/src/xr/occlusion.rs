//! Real things hide the skater and the park ("occlusion"), with Meta's
//! depth API (`XR_META_environment_depth`): the headset gives a depth image
//! of the room for each eye, and after the scene is drawn a pass over each
//! eye view takes out the pixels whose place in the room is behind what is
//! really there (`occlusion.wgsl`).
//!
//! Off unless the AR settings turn it on (it costs GPU time, and the depth
//! provider runs only while it is on): hard (a pixel is hidden or not) or
//! soft (edges fade, nine depth samples a pixel instead of one). Only solid
//! things are hidden: the menu, the build legend and other see-through
//! draws write no depth and stay in front of everything.
use super::ar_settings::ArSettings;
use super::rig::Rig;
use bevy::core_pipeline::FullscreenShader;
use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
use bevy::ecs::query::QueryItem;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_graph::{NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner};
use bevy::render::render_resource::binding_types::{texture_2d_array, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent, BlendFactor, BlendOperation, BlendState,
    CachedRenderPipelineId, ColorTargetState, ColorWrites, FragmentState, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, ShaderStages, ShaderType, SpecializedRenderPipeline, SpecializedRenderPipelines,
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
    /// Edges fade.
    Soft,
    /// A pixel is hidden or it isn't: the cheapest.
    Hard,
}

impl Occlusion {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Off => Self::Soft,
            Self::Soft => Self::Hard,
            Self::Hard => Self::Off,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Soft => "soft",
            Self::Hard => "hard",
        }
    }
}

/// What the render world needs each frame: the setting, and the XR rig's
/// root (the game world from the room's tracking space).
#[derive(Resource, Clone, Copy, Default, ExtractResource)]
struct Wanted {
    mode: Occlusion,
    world_from_tracking: Mat4,
}

/// Why occlusion can't run here, for the settings page (none: it can, or
/// it hasn't been tried).
#[derive(Resource, Default, Clone)]
pub(crate) struct OcclusionProblem(pub std::sync::Arc<std::sync::Mutex<Option<&'static str>>>);

pub(crate) struct OcclusionPlugin;

impl Plugin for OcclusionPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "occlusion.wgsl");
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

fn want(settings: Res<ArSettings>, rig: Res<Rig>, mut wanted: ResMut<Wanted>) {
    let root = rig.root_transform();
    wanted.mode = if root.is_some() { settings.occlusion } else { Occlusion::Off };
    wanted.world_from_tracking = root.map_or(Mat4::IDENTITY, |r| r.to_matrix());
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
}

struct Made {
    provider: sys::EnvironmentDepthProviderMETA,
    /// Each swapchain image, both eyes' layers. (Kept for as long as the
    /// app runs: the runtime owns the images.)
    views: Vec<(Texture, TextureView)>,
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
    mode: u32,
    layer: u32,
}

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
            views.push((texture, view));
        }
        info!("Occlusion: room depth {}x{}, {} images", state.width, state.height, views.len());
        Ok(Made { provider, views })
    }
}

/// Starts and stops the provider with the setting, and takes the latest
/// depth image for this frame.
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
    if on != depth.running {
        let result = unsafe { if on { (fns.start_environment_depth_provider)(provider) } else { (fns.stop_environment_depth_provider)(provider) } };
        info!("Occlusion: depth sensing {} ({:?})", if on { "started" } else { "stopped" }, result);
        depth.running = on && result.into_raw() >= 0;
        depth.latest = None;
        if !on {
            depth.logged = false;
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
}

#[derive(PartialEq, Eq, Hash, Clone, Copy)]
struct PipelineKey {
    format: TextureFormat,
    multisampled: bool,
}

#[derive(Component)]
struct OcclusionPipelineId(CachedRenderPipelineId, bool);

fn init_pipeline(mut commands: Commands, fullscreen: Res<FullscreenShader>, assets: Res<AssetServer>) {
    let layout = |multisampled: bool| {
        let entries = (
            uniform_buffer::<ViewUniform>(true),
            if multisampled { texture_depth_2d_multisampled() } else { texture_depth_2d() },
            texture_2d_array(TextureSampleType::Depth),
            uniform_buffer::<Eye>(false),
        );
        BindGroupLayoutDescriptor::new("occlusion", &BindGroupLayoutEntries::sequential(ShaderStages::FRAGMENT, entries))
    };
    commands.insert_resource(OcclusionPipeline {
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
    views: Query<(Entity, &ExtractedView, &Msaa), With<XrCamera>>,
) {
    let (Some(wanted), Some(latest)) = (wanted.filter(|w| w.mode != Occlusion::Off), depth.latest) else { return };
    for (entity, view, msaa) in &views {
        let multisampled = msaa.samples() > 1;
        let format = if view.hdr { ViewTarget::TEXTURE_FORMAT_HDR } else { TextureFormat::bevy_default() };
        let id = pipelines.specialize(&cache, &pipeline, PipelineKey { format, multisampled });
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
            mode: if wanted.mode == Occlusion::Soft { 2 } else { 1 },
            layer: layer as u32,
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
        let on = world.get_resource::<Wanted>().is_some_and(|w| w.mode != Occlusion::Off);
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
            &BindGroupEntries::sequential((view_uniforms, scene_depth.view(), room, eye)),
        );
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
        assert_eq!(Occlusion::Off.next().next().next(), Occlusion::Off);
        assert_eq!(Occlusion::default(), Occlusion::Off);
        let s: Occlusion = serde_json::from_str("\"Soft\"").unwrap();
        assert_eq!((s, s.name()), (Occlusion::Soft, "soft"));
    }
}
