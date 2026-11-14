// One eye's half of the room's depth image as it came, drawn to a texture
// that can be read back (the developer's capture: occlusion_capture.rs).
@group(0) @binding(0) var room_depth: texture_depth_2d;

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4(textureLoad(room_depth, vec2<i32>(position.xy), 0), 0.0, 0.0, 1.0);
}
