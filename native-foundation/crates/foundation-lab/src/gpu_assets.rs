//! Real texture uploads, sampling, eviction and retirement against the cache budget.

use crate::gpu::{Error, Gpu, write_png};
use residency::{AssetId, Budget, Cache, Handle, Priority, Published, Request, Spec};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Instant};

const SIDE: u32 = 256;
const BYTES: u64 = (SIDE * SIDE * 4) as u64;
const TARGET: u32 = SIDE * 2;

struct TextureAsset {
    texture: wgpu::Texture,
    group: wgpu::BindGroup,
    pixels: Vec<u8>,
}

fn color(id: u64) -> [u8; 4] {
    let value = id.wrapping_mul(0x9e3779).wrapping_add(0x552211);
    [value as u8, (value >> 8) as u8, (value >> 16) as u8, 255]
}
fn pixels(id: u64) -> Vec<u8> {
    let main = color(id);
    let mut pixels = vec![0; BYTES as usize];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let dark = x < 8 || y < 8 || x >= SIDE - 8 || y >= SIDE - 8;
            let value = if dark {
                [main[0] / 2, main[1] / 2, main[2] / 2, 255]
            } else {
                main
            };
            let offset = ((y * SIDE + x) * 4) as usize;
            pixels[offset..offset + 4].copy_from_slice(&value);
        }
    }
    pixels
}

fn upload(
    gpu: &Gpu,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    id: AssetId,
) -> Result<TextureAsset, Error> {
    let pixels = pixels(id.0);
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("budgeted asset texture"),
        size: wgpu::Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        texture.as_image_copy(),
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(SIDE * 4),
            rows_per_image: Some(SIDE),
        },
        texture.size(),
    );
    let submitted = gpu.queue.submit([]);
    gpu.wait(Some(submitted))?;
    let view = texture.create_view(&Default::default());
    let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("budgeted asset binding"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    Ok(TextureAsset {
        texture,
        group,
        pixels,
    })
}

fn acquire(
    cache: &mut Cache,
    assets: &mut BTreeMap<Handle, TextureAsset>,
    gpu: &Gpu,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    id: AssetId,
    fences: &mut u64,
) -> Result<Handle, Error> {
    for _ in 0..8 {
        match cache.request(id, Priority::Visible)? {
            Request::Ready(handle) => return Ok(handle),
            Request::Load(token) => {
                let asset = upload(gpu, layout, sampler, id)?;
                let Published::Ready(handle) =
                    cache.publish(token, asset.pixels.len() as u64, BYTES)?
                else {
                    return Err("uncanceled load retired unexpectedly".into());
                };
                assert!(assets.insert(handle, asset).is_none());
                return Ok(handle);
            }
            Request::Deferred { retire, .. } => {
                if retire.is_empty() {
                    return Err("GPU experiment made no admission progress".into());
                }
                let charged = cache.usage().gpu();
                assert!(cache.usage().retiring_gpu >= BYTES);
                gpu.wait(None)?;
                *fences += 1;
                for token in retire {
                    let asset = assets
                        .remove(&token.0)
                        .ok_or("missing retired GPU resource")?;
                    asset.texture.destroy();
                    drop(asset);
                    assert!(cache.acknowledge_retired(token));
                }
                assert!(cache.usage().gpu() < charged);
            }
        }
    }
    Err("GPU asset admission did not converge".into())
}

pub fn run(output: &Path) -> Result<Value, Error> {
    let gpu = Gpu::open()?;
    let shader = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("foundation gallery shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gallery.wgsl").into()),
        });
    let pipeline = gpu
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("foundation gallery"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
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
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
    let layout = pipeline.get_bind_group_layout(0);
    let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("foundation nearest sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("foundation gallery target"),
        size: wgpu::Extent3d {
            width: TARGET,
            height: TARGET,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let budget = Budget {
        cpu_bytes: 8 * BYTES,
        gpu_bytes: 4 * BYTES,
        max_entries: 8,
        max_loading: 2,
    };
    let mut cache = Cache::new(budget)?;
    for id in 0..4096 {
        cache.register(Spec {
            id: AssetId(id),
            cpu_bytes: BYTES,
            gpu_bytes: BYTES,
            transient_cpu_bytes: BYTES,
        })?;
    }
    let mut assets = BTreeMap::new();
    let mut fences = 0;
    let mut pixel_assertions = 0;
    let mut frame_ms = Vec::new();
    let mut final_pixels = Vec::new();
    for frame in 0..256u64 {
        let started = Instant::now();
        let mut handles = Vec::new();
        for tile in 0..4 {
            let handle = acquire(
                &mut cache,
                &mut assets,
                &gpu,
                &layout,
                &sampler,
                AssetId((frame * 4 + tile) % 4096),
                &mut fences,
            )?;
            cache.pin(handle)?;
            handles.push(handle);
        }
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("foundation gallery pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1.,
                            g: 0.,
                            b: 1.,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            for (tile, handle) in handles.iter().enumerate() {
                let asset = &assets[handle];
                pass.set_viewport(
                    (tile % 2) as f32 * SIDE as f32,
                    (tile / 2) as f32 * SIDE as f32,
                    SIDE as f32,
                    SIDE as f32,
                    0.,
                    1.,
                );
                pass.set_bind_group(0, &asset.group, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        gpu.queue.submit([encoder.finish()]);
        for handle in &handles {
            cache.unpin(*handle)?;
        }
        if frame % 32 == 0 || frame == 255 {
            let read = gpu.read_texture(&target)?;
            for (tile, handle) in handles.iter().enumerate() {
                let (x, y) = (
                    (tile as u32 % 2) * SIDE + SIDE / 2,
                    (tile as u32 / 2) * SIDE + SIDE / 2,
                );
                let offset = ((y * TARGET + x) * 4) as usize;
                assert_eq!(
                    &read[offset..offset + 4],
                    &color(handle.id.0),
                    "sampled pixels belong to wrong asset generation"
                );
                pixel_assertions += 1;
            }
            final_pixels = read;
        }
        let usage = cache.usage();
        let actual_cpu: usize = assets.values().map(|a| a.pixels.capacity()).sum();
        assert!(
            actual_cpu as u64 <= usage.cpu()
                && usage.cpu() <= budget.cpu_bytes
                && usage.gpu() <= budget.gpu_bytes
        );
        frame_ms.push(started.elapsed().as_secs_f64() * 1000.);
    }
    gpu.wait(None)?;
    let handles: Vec<_> = cache.ready_handles().collect();
    for handle in handles {
        let token = cache.retire(handle)?;
        let asset = assets.remove(&handle).unwrap();
        asset.texture.destroy();
        drop(asset);
        assert!(cache.acknowledge_retired(token));
    }
    assert!(assets.is_empty() && cache.usage().entries() == 0);
    let image = output.with_extension("png");
    write_png(&image, TARGET, TARGET, &final_pixels)?;
    frame_ms.sort_by(f64::total_cmp);
    let stats = cache.stats();
    Ok(
        json!({"status":"PASS","adapter":gpu.info.name,"backend":format!("{:?}",gpu.info.backend),"device_type":format!("{:?}",gpu.info.device_type),"frames":256,"catalog_entries":4096,"actual_texture_uploads":stats.admitted,"evictions":stats.evicted,"retirement_fences":fences,"readback_pixel_assertions":pixel_assertions,"asset_cpu_budget":budget.cpu_bytes,"asset_gpu_budget":budget.gpu_bytes,"peak_charged_cpu":stats.peak_cpu,"peak_charged_gpu":stats.peak_gpu,"texture_bytes_each":BYTES,"fixed_render_target_bytes":u64::from(TARGET)*u64::from(TARGET)*4,"fixed_readback_peak_bytes":u64::from(TARGET)*u64::from(TARGET)*4,"sample_median_ms":frame_ms[128],"sample_p99_ms":frame_ms[253],"image":image,"ending_asset_allocations":0,"scope":"synchronous texture-admission correctness experiment, not game frame-time performance; logical payload budgets exclude driver overhead"}),
    )
}
