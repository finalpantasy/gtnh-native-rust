//! Real headless GPU utilities. Missing hardware or failed readback is a failed scenario.

use std::{fs::File, path::Path, sync::mpsc, time::Duration};

pub type Error = Box<dyn std::error::Error>;

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
}

impl Gpu {
    pub fn open() -> Result<Self, Error> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::VULKAN | wgpu::Backends::DX12;
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))?;
        Self::from_adapter(&adapter)
    }
    pub fn from_adapter(adapter: &wgpu::Adapter) -> Result<Self, Error> {
        let info = adapter.get_info();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("GTNH foundation GPU"),
                ..Default::default()
            }))?;
        Ok(Self {
            device,
            queue,
            info,
        })
    }
    pub fn wait(&self, index: Option<wgpu::SubmissionIndex>) -> Result<(), Error> {
        self.device.poll(wgpu::PollType::Wait {
            submission_index: index,
            timeout: Some(Duration::from_secs(10)),
        })?;
        Ok(())
    }
    pub fn read_texture(&self, texture: &wgpu::Texture) -> Result<Vec<u8>, Error> {
        let size = texture.size();
        let row = size.width * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("foundation readback"),
            size: u64::from(padded) * u64::from(size.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(size.height),
                },
            },
            size,
        );
        let index = self.queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::sync_channel(1);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.wait(Some(index))?;
        receiver.recv_timeout(Duration::from_secs(10))??;
        let mapped = buffer.slice(..).get_mapped_range()?;
        let mut pixels = Vec::with_capacity((row * size.height) as usize);
        for line in mapped.chunks(padded as usize).take(size.height as usize) {
            pixels.extend_from_slice(&line[..row as usize]);
        }
        drop(mapped);
        buffer.unmap();
        buffer.destroy();
        Ok(pixels)
    }
}

pub fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<(), Error> {
    let mut encoder = png::Encoder::new(File::create(path)?, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)?;
    Ok(())
}
