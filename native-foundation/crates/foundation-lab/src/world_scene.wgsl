struct Transform { camera: mat4x4<f32>, offset: vec4<f32> }
@group(0) @binding(0) var<uniform> transform: Transform;
struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) color: vec3<f32> }
@vertex fn vertex(@location(0) local: vec3<f32>, @location(1) color: vec3<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = transform.camera * vec4<f32>(local + transform.offset.xyz, 1.0);
    out.color = color;
    return out;
}
@fragment fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(input.color, 1.0);
}
