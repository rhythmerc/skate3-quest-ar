// The outside of something sunk into a real surface (a bowl in the floor): drawn as nothing at
// all, colour and alpha zero so passthrough shows there, but into the depth buffer. Seen from the
// side, through the floor, the bowl's outside is in front of its inside, so the inside (and a
// skater in it) is hidden exactly where the real floor would hide it.
#import bevy_pbr::forward_io::VertexOutput

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}
