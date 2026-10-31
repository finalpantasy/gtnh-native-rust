struct VertexOut { @builtin(position) position: vec4f, @location(0) uv: vec2f }
@group(0) @binding(0) var picture: texture_2d<f32>;
@group(0) @binding(1) var picture_sampler: sampler;

@vertex fn vertex(@builtin(vertex_index) index:u32)->VertexOut {
    let positions=array<vec2f,6>(vec2f(-1,-1),vec2f(1,-1),vec2f(1,1),vec2f(-1,-1),vec2f(1,1),vec2f(-1,1));
    let uv=array<vec2f,6>(vec2f(0,1),vec2f(1,1),vec2f(1,0),vec2f(0,1),vec2f(1,0),vec2f(0,0));
    return VertexOut(vec4f(positions[index],0,1),uv[index]);
}
@fragment fn fragment(input:VertexOut)->@location(0) vec4f {
    return textureSample(picture,picture_sampler,input.uv);
}
