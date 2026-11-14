// Real things in front of the skater and the park hide them: after the
// scene is drawn, each of its pixels is found in the headset's depth image
// of the room (XR_META_environment_depth) and, where the room is nearer,
// taken out of the picture (colour and alpha to 0: passthrough shows).
#import bevy_render::view::View

struct Eye {
    // World to the depth image's view (metres; -z ahead).
    view_from_world: mat4x4<f32>,
    // The image's left, right, down and up edges, as tangents.
    tangents: vec4<f32>,
    near: f32,
    far: f32,
    layer: u32,
    hole_count: u32,
    // How far under a mouth's height counts as in the bowl (world units).
    below: f32,
    // Not 0: one sample of the room's image a pixel (hard edges, quick).
    hard: u32,
    // The mouths of the bowls sunk into real surfaces: the centre (world), and the radius.
    holes: array<vec4<f32>, 4>,
}

@group(0) @binding(0) var<uniform> view: View;
#ifdef MULTISAMPLED
@group(0) @binding(1) var scene_depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(1) var scene_depth: texture_depth_2d;
#endif
@group(0) @binding(2) var room_depth: texture_depth_2d_array;
@group(0) @binding(3) var<uniform> eye: Eye;

// How far along the room's view the real surface at a texel is, metres (0: unknown).
fn room_distance(texel: vec2<i32>, size: vec2<i32>) -> f32 {
    let d = textureLoad(room_depth, clamp(texel, vec2(0), size - 1), i32(eye.layer), 0);
    if d <= 0.0 || d >= 1.0 {
        return 0.0;
    }
    if eye.far <= 0.0 || eye.far > 1e6 {
        return eye.near / (1.0 - d);
    }
    return eye.near * eye.far / (eye.far - d * (eye.far - eye.near));
}

// 1: seen; 0: behind the real surface. A little past it still counts as on it.
fn seen(real: f32, drawn: f32) -> f32 {
    if real <= 0.0 {
        return 1.0;
    }
    return 1.0 - smoothstep(real * 1.02, real * 1.08 + 0.01, drawn);
}

// The cubic B-spline's weights for the four texels about a point `f` of
// the way from the second to the third (they add up to 1).
fn spline(f: f32) -> vec4<f32> {
    let g = 1.0 - f;
    return vec4(g * g * g, 3.0 * f * f * f - 6.0 * f * f + 4.0, 3.0 * g * g * g - 6.0 * g * g + 4.0, f * f * f) / 6.0;
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let depth = textureLoad(scene_depth, vec2<i32>(position.xy), 0);
    // Nothing solid here (the menu and other see-through things are left alone).
    if depth <= 0.0 {
        return vec4(1.0);
    }
    let uv = (position.xy - view.viewport.xy) / view.viewport.zw;
    let ndc = vec3(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let clip = view.world_from_clip * vec4(ndc, 1.0);
    var world = clip.xyz / clip.w;
    // In a bowl, under the real surface it is sunk into (which would hide
    // all of it): the point that counts is where the line of sight goes in
    // through the bowl's mouth, and only what is really in front of that,
    // by more than the depth image's error on the surface itself, hides it.
    var slack = 0.0;
    for (var i = 0u; i < eye.hole_count; i++) {
        let hole = eye.holes[i];
        let eye_height = view.world_position.y - hole.y;
        if world.y < hole.y - eye.below && eye_height > 0.0 && distance(world.xz, hole.xz) < hole.w {
            world = mix(view.world_position, world, eye_height / (view.world_position.y - world.y));
            slack = 1.0;
            break;
        }
    }
    let at = eye.view_from_world * vec4(world, 1.0);
    var ahead = -at.z;
    if ahead <= eye.near {
        return vec4(1.0);
    }
    let t = vec2(at.x, at.y) / ahead;
    ahead = ahead * (1.0 - 0.08 * slack) - 0.03 * slack;
    // Where in the room's image (its rows from the bottom).
    let image = vec2((t.x - eye.tangents.x) / (eye.tangents.y - eye.tangents.x), (t.y - eye.tangents.z) / (eye.tangents.w - eye.tangents.z));
    if any(image < vec2(0.0)) || any(image > vec2(1.0)) {
        return vec4(1.0);
    }
    let size = vec2<i32>(textureDimensions(room_depth).xy);
    if eye.hard != 0u {
        // The texel the pixel is in, and no other: edges step along the room's texels.
        return vec4(seen(room_distance(vec2<i32>(floor(image * vec2<f32>(size))), size), ahead));
    }
    let texel = image * vec2<f32>(size) - 0.5;
    let base = vec2<i32>(floor(texel));
    // The room's image is far coarser than the picture (a texel is some
    // seven pixels across): each texel's answer is blended with a cubic
    // B-spline over the sixteen about the pixel, which has no steps where
    // the pixel passes from one texel to the next, so edges are feathered
    // over about two texels, not jagged along them.
    let f = texel - vec2<f32>(base);
    let wx = spline(f.x);
    let wy = spline(f.y);
    var shown = 0.0;
    for (var y = 0; y < 4; y++) {
        for (var x = 0; x < 4; x++) {
            shown += wx[x] * wy[y] * seen(room_distance(base + vec2(x - 1, y - 1), size), ahead);
        }
    }
    return vec4(shown);
}
