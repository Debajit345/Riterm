struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) background: vec4<f32>,
    @location(3) rect_size: vec2<f32>,
    @location(4) image_size: vec2<f32>,
    // mode, blend, opacity, unused
    @location(5) params: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) background: vec4<f32>,
    @location(2) rect_size: vec2<f32>,
    @location(3) image_size: vec2<f32>,
    @location(4) params: vec4<f32>,
};

@group(0) @binding(0) var image_texture: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(input.position, 0.0, 1.0);
    out.uv = input.uv;
    out.background = input.background;
    out.rect_size = input.rect_size;
    out.image_size = input.image_size;
    out.params = input.params;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let mode = input.params.x;
    let blend = clamp(input.params.y, 0.0, 1.0);
    let opacity = clamp(input.params.z, 0.0, 1.0);
    let local = input.uv * input.rect_size;
    let safe_image = max(input.image_size, vec2<f32>(1.0, 1.0));

    var image_uv = input.uv;
    var inside = true;
    if (mode == 1.0) {
        // Tile at the image's natural pixel size.
        image_uv = local / safe_image;
    } else if (mode == 2.0 || mode == 3.0) {
        var scale = max(input.rect_size.x / safe_image.x, input.rect_size.y / safe_image.y);
        if (mode == 3.0) {
            scale = min(input.rect_size.x / safe_image.x, input.rect_size.y / safe_image.y);
        }
        let displayed = safe_image * scale;
        let origin = (input.rect_size - displayed) * 0.5;
        let offset = local - origin;
        if (mode == 3.0 &&
            (offset.x < 0.0 || offset.y < 0.0 ||
             offset.x > displayed.x || offset.y > displayed.y)) {
            inside = false;
        }
        image_uv = offset / displayed;
    }

    var color = input.background.rgb;
    if (inside) {
        let sample = textureSample(image_texture, image_sampler, image_uv);
        color = mix(input.background.rgb, sample.rgb, blend * sample.a);
    }
    return vec4<f32>(color, opacity);
}
