//! Offscreen integration of independent cubic streaming, industrial state, and budgeted meshes.
use crate::world_storage::{Backend, WorldStore};
use crate::{
    factory,
    gpu::{Error, Gpu, write_png},
    voxel_mesh,
};
use glam::{Mat4, Vec3};
use residency::{AssetId, Budget, Cache, Handle, Priority, Published, Request, Spec};
use serde_json::{Value, json};
use std::{collections::BTreeMap, num::NonZeroU64, path::Path, time::Instant};
use voxel_core::{coords::CubePos, record::CubeRecord, streaming::Streamer};

const WIDTH: u32 = 1024;
const HEIGHT: u32 = 768;
const MAX_CUBES: usize = 27;
const UNIFORM_STRIDE: u64 = 256;
const ASSET_BUDGET: u64 = 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Meshing {
    Merged,
    BlockFacesReference,
}
struct MeshAsset {
    buffer: wgpu::Buffer,
    bytes: Vec<u8>,
}
pub struct Scene {
    gpu: Gpu,
    cache: Cache,
    assets: BTreeMap<Handle, MeshAsset>,
    keys: BTreeMap<(CubePos, [u8; 32]), AssetId>,
    meshing: Meshing,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    group: wgpu::BindGroup,
    color: wgpu::Texture,
    depth: wgpu::Texture,
    frames: u64,
    fences: u64,
    draws: u64,
    samples: u64,
    timings: Vec<f64>,
}
impl Scene {
    pub fn new() -> Result<Self, Error> {
        Self::with_gpu(Gpu::open()?)
    }
    pub fn with_gpu(gpu: Gpu) -> Result<Self, Error> {
        Self::with_meshing(gpu, Meshing::Merged)
    }
    /// Explicit comparison renderer; the reference mode is bounded by the same asset budget.
    pub fn with_meshing(gpu: Gpu, meshing: Meshing) -> Result<Self, Error> {
        let layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("cubic scene bindings"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(80),
                    },
                    count: None,
                }],
            });
        let uniforms = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fixed per-cube transforms"),
            size: UNIFORM_STRIDE * MAX_CUBES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cubic scene transforms"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniforms,
                    offset: 0,
                    size: NonZeroU64::new(80),
                }),
            }],
        });
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("cubic scene shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("world_scene.wgsl").into()),
            });
        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("cubic scene layout"),
                bind_group_layouts: &[Some(&layout)],
                ..Default::default()
            });
        let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3];
        let pipeline = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("cubic scene pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: voxel_mesh::VERTEX_BYTES as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fragment"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
        let make_texture = |label, format, usage| {
            gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: WIDTH,
                    height: HEIGHT,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = make_texture(
            "fixed scene color",
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = make_texture(
            "fixed scene depth",
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let mut cache = Cache::new(Budget {
            cpu_bytes: ASSET_BUDGET,
            gpu_bytes: ASSET_BUDGET,
            max_entries: MAX_CUBES,
            max_loading: 1,
        })?;
        for id in 0..4096 {
            cache.register(Spec {
                id: AssetId(id),
                cpu_bytes: 1024,
                gpu_bytes: 1024,
                transient_cpu_bytes: 0,
            })?;
        }
        Ok(Self {
            gpu,
            cache,
            assets: BTreeMap::new(),
            keys: BTreeMap::new(),
            meshing,
            pipeline,
            uniforms,
            group,
            color,
            depth,
            frames: 0,
            fences: 0,
            draws: 0,
            samples: 0,
            timings: Vec::new(),
        })
    }
    fn acquire(
        &mut self,
        cube: &CubeRecord,
        surface: &voxel_mesh::Surface<'_>,
    ) -> Result<Handle, Error> {
        let key = (cube.pos(), surface.identity());
        let id = if let Some(id) = self.keys.get(&key) {
            *id
        } else {
            let id = AssetId(4096 + self.keys.len() as u64);
            let bytes = match self.meshing {
                Meshing::Merged => surface.size(),
                Meshing::BlockFacesReference => voxel_mesh::mesh_size(cube),
            } as u64;
            if bytes == 0 {
                return Err("unexpected empty scene cube".into());
            }
            self.cache.register(Spec {
                id,
                cpu_bytes: bytes,
                gpu_bytes: bytes,
                transient_cpu_bytes: 0,
            })?;
            self.keys.insert(key, id);
            id
        };
        for _ in 0..MAX_CUBES + 2 {
            match self.cache.request(id, Priority::Visible)? {
                Request::Ready(handle) => return Ok(handle),
                Request::Load(token) => {
                    let bytes = match self.meshing {
                        Meshing::Merged => surface.emit(),
                        Meshing::BlockFacesReference => voxel_mesh::mesh(cube),
                    };
                    let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("budgeted cubic mesh"),
                        size: bytes.len() as u64,
                        usage: wgpu::BufferUsages::VERTEX,
                        mapped_at_creation: true,
                    });
                    buffer
                        .slice(..)
                        .get_mapped_range_mut()?
                        .copy_from_slice(&bytes);
                    buffer.unmap();
                    let Published::Ready(handle) =
                        self.cache
                            .publish(token, bytes.capacity() as u64, buffer.size())?
                    else {
                        return Err("unexpected canceled mesh upload".into());
                    };
                    assert!(
                        self.assets
                            .insert(handle, MeshAsset { buffer, bytes })
                            .is_none()
                    );
                    return Ok(handle);
                }
                Request::Deferred { retire, .. } => {
                    if retire.is_empty() {
                        return Err("mesh admission stalled".into());
                    }
                    let before = self.cache.usage().gpu();
                    assert!(self.cache.usage().retiring_gpu > 0);
                    self.gpu.wait(None)?;
                    self.fences += 1;
                    for token in retire {
                        let asset = self.assets.remove(&token.0).ok_or("retired mesh missing")?;
                        asset.buffer.destroy();
                        drop(asset);
                        assert!(self.cache.acknowledge_retired(token));
                    }
                    assert!(self.cache.usage().gpu() < before);
                }
            }
        }
        Err("mesh admission failed to converge".into())
    }
    fn render(
        &mut self,
        stream: &Streamer<WorldStore>,
        center: CubePos,
        frame: u64,
        output: &Path,
    ) -> Result<(), Error> {
        let camera =
            Mat4::perspective_rh(55f32.to_radians(), WIDTH as f32 / HEIGHT as f32, 1., 400.)
                * Mat4::look_at_rh(Vec3::new(62., 52., 76.), Vec3::new(8., 8., 8.), Vec3::Y);
        self.draw(stream, center, camera, Some((output, frame)))
    }
    pub fn render_camera(
        &mut self,
        stream: &Streamer<WorldStore>,
        center: CubePos,
        camera: Mat4,
    ) -> Result<(), Error> {
        self.draw(stream, center, camera, None)
    }
    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }
    pub fn color_texture(&self) -> &wgpu::Texture {
        &self.color
    }
    pub fn dimensions() -> (u32, u32) {
        (WIDTH, HEIGHT)
    }
    pub fn usage(&self) -> residency::Usage {
        self.cache.usage()
    }
    pub fn budget(&self) -> Budget {
        self.cache.budget()
    }
    fn draw(
        &mut self,
        stream: &Streamer<WorldStore>,
        center: CubePos,
        camera: Mat4,
        sample: Option<(&Path, u64)>,
    ) -> Result<(), Error> {
        let started = Instant::now();
        let positions: Vec<_> = stream
            .resident_positions()
            .filter(|p| stream.renders(*p))
            .collect();
        assert!(positions.len() <= MAX_CUBES);
        let mut handles = Vec::with_capacity(positions.len());
        let admission: Result<(), Error> = (|| {
            for pos in &positions {
                let cube = stream.record(*pos).unwrap();
                let surface = voxel_mesh::Surface::rendered(stream, *pos)?;
                let bytes = match self.meshing {
                    Meshing::Merged => surface.size(),
                    Meshing::BlockFacesReference => voxel_mesh::mesh_size(cube),
                };
                if bytes == 0 {
                    continue;
                }
                let handle = self.acquire(cube, &surface)?;
                self.cache.pin(handle)?;
                handles.push((*pos, handle));
            }
            Ok(())
        })();
        if let Err(error) = admission {
            for (_, handle) in handles {
                self.cache.unpin(handle)?;
            }
            return Err(error);
        }
        let mut transforms = vec![0u8; UNIFORM_STRIDE as usize * MAX_CUBES];
        for (slot, (pos, _)) in handles.iter().enumerate() {
            // Subtract in integer space before converting to camera-relative f32.
            let delta = [pos.x - center.x, pos.y - center.y, pos.z - center.z];
            assert!(delta.iter().all(|d| d.unsigned_abs() <= 1));
            let values = camera
                .to_cols_array()
                .into_iter()
                .chain(delta.map(|d| (d * 16) as f32))
                .chain([0.]);
            for (index, float) in values.enumerate() {
                let offset = slot * UNIFORM_STRIDE as usize + index * 4;
                transforms[offset..offset + 4].copy_from_slice(&float.to_le_bytes());
            }
        }
        self.gpu.queue.write_buffer(&self.uniforms, 0, &transforms);
        let color = self.color.create_view(&Default::default());
        let depth = self.depth.create_view(&Default::default());
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cubic factory scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.025,
                            g: 0.04,
                            b: 0.055,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            for (slot, (_, handle)) in handles.iter().enumerate() {
                let asset = &self.assets[handle];
                pass.set_bind_group(0, &self.group, &[(slot as u64 * UNIFORM_STRIDE) as u32]);
                pass.set_vertex_buffer(0, asset.buffer.slice(..));
                pass.draw(
                    0..(asset.bytes.len() / voxel_mesh::VERTEX_BYTES) as u32,
                    0..1,
                );
                self.draws += 1;
            }
        }
        self.gpu.queue.submit([encoder.finish()]);
        for (_, handle) in handles {
            self.cache.unpin(handle)?;
        }
        if let Some((output, frame)) =
            sample.filter(|(_, frame)| frame.is_multiple_of(16) || *frame == 127)
        {
            let pixels = self.gpu.read_texture(&self.color)?;
            let geometry = pixels
                .chunks_exact(4)
                .filter(|p| p[0] > 20 || p[1] > 25 || p[2] > 30)
                .count();
            assert!(geometry > 10_000, "blank or missing voxel geometry");
            self.samples += 1;
            if frame == 127 {
                write_png(&output.with_extension("png"), WIDTH, HEIGHT, &pixels)?;
            }
        }
        let usage = self.cache.usage();
        let actual_cpu = self
            .assets
            .values()
            .map(|a| a.bytes.capacity() as u64)
            .sum::<u64>();
        let actual_gpu = self.assets.values().map(|a| a.buffer.size()).sum::<u64>();
        assert_eq!(actual_cpu, usage.cpu());
        assert_eq!(actual_gpu, usage.gpu());
        assert!(
            usage.cpu() <= ASSET_BUDGET
                && usage.gpu() <= ASSET_BUDGET
                && usage.entries() <= MAX_CUBES
        );
        self.frames += 1;
        self.timings.push(started.elapsed().as_secs_f64() * 1000.);
        Ok(())
    }
    pub fn finish(mut self, output: &Path) -> Result<Value, Error> {
        self.gpu.wait(None)?;
        for handle in self.cache.ready_handles().collect::<Vec<_>>() {
            let token = self.cache.retire(handle)?;
            let asset = self.assets.remove(&handle).ok_or("missing final mesh")?;
            asset.buffer.destroy();
            drop(asset);
            assert!(self.cache.acknowledge_retired(token));
        }
        assert!(self.assets.is_empty() && self.cache.usage().entries() == 0);
        self.timings.sort_by(f64::total_cmp);
        let stats = self.cache.stats();
        Ok(
            json!({"status":"PASS","adapter":self.gpu.info.name,"backend":format!("{:?}",self.gpu.info.backend),"frames":self.frames,"draw_calls":self.draws,"catalog_entries":4096+self.keys.len(),"actual_mesh_uploads":stats.admitted,"evictions":stats.evicted,"retirement_fences":self.fences,"nonblank_readbacks":self.samples,"cpu_asset_budget":ASSET_BUDGET,"gpu_asset_budget":ASSET_BUDGET,"peak_charged_cpu":stats.peak_cpu,"peak_charged_gpu":stats.peak_gpu,"ending_asset_allocations":0,"fixed_color_bytes":WIDTH*HEIGHT*4,"fixed_depth_bytes":WIDTH*HEIGHT*4,"fixed_uniform_bytes":UNIFORM_STRIDE*MAX_CUBES as u64,"fixed_readback_peak_bytes":WIDTH*HEIGHT*4,"sample_median_ms":self.timings[self.timings.len()/2],"sample_p99_ms":self.timings[self.timings.len()*99/100],"image":output.with_extension("png"),"scope":"real offscreen integration correctness experiment; synchronous admission/fences/readbacks; excludes driver overhead and is not production game frame performance"}),
        )
    }
}
pub fn run(output: &Path, backend: Backend) -> Result<(Value, Value), Error> {
    let mut scene = Scene::new()?;
    let factory = factory::run_with_backend(128, backend, |stream, center, frame| {
        scene.render(stream, center, frame, output)
    })?;
    assert_eq!(factory.final_state.len(), 2);
    Ok((factory.report, scene.finish(output)?))
}
