// The room's depth image, made ready for soft occlusion (after Meta's
// "EnvironmentDepth/Preprocessing"): for each texel, of the sixteen about
// it, the mean of the near ones, the mean of the far ones and the mean of
// all. Occlusion then tests a pixel against these means, which move far
// less from one depth image to the next than any one texel does.
//
// Out: (1 - near mean, 1 - far mean, mean - near mean, far mean - near
// mean), in the image's own depth values (not metres). Texel `i` is of
// the texels `i - 1` to `i + 2`: its middle is half a texel on from `i`'s.
@group(0) @binding(0) var room_depth: texture_depth_2d;

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(room_depth));
    let base = vec2<i32>(position.xy) - 1;
    var depths: array<f32, 16>;
    var nearest = 1.0;
    var farthest = 0.0;
    var sum = 0.0;
    for (var i = 0; i < 16; i++) {
        var d = textureLoad(room_depth, clamp(base + vec2(i % 4, i / 4), vec2(0), size - 1), 0);
        // (nothing known there: as far as can be, which hides nothing)
        if d <= 0.0 {
            d = 1.0;
        }
        depths[i] = d;
        nearest = min(nearest, d);
        farthest = max(farthest, d);
        sum += d;
    }
    let mean = sum / 16.0;
    // Far: at 85% of the farthest's distance or more. Near: within 115% of the nearest's.
    let far_from = (1.0 - 1.0 / 0.85) + farthest / 0.85;
    let near_to = (1.0 - 1.0 / 1.15) + nearest / 1.15;
    // All of them are both (a flat patch): one mean.
    if far_from < nearest && near_to > farthest {
        return vec4(1.0 - mean, 1.0 - mean, 0.0, 0.0);
    }
    var near_sum = 0.0;
    var near_count = 0.0;
    var far_sum = 0.0;
    var far_count = 0.0;
    for (var i = 0; i < 16; i++) {
        if depths[i] <= near_to {
            near_sum += depths[i];
            near_count += 1.0;
        }
        if depths[i] >= far_from {
            far_sum += depths[i];
            far_count += 1.0;
        }
    }
    let near_mean = near_sum / near_count;
    let far_mean = far_sum / far_count;
    return vec4(1.0 - near_mean, 1.0 - far_mean, mean - near_mean, far_mean - near_mean);
}
