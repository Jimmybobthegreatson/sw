//! Render to a texture, read back, save PNG. Used for headless verification and screenshots.

use crate::camera::Camera;
use crate::gpu_scene::SceneGpu;
use crate::params::FrameParams;
use crate::renderer::{DepthTexture, RenderTarget, Renderer};

pub struct OffscreenTarget {
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth: DepthTexture,
}

impl OffscreenTarget {
    /// `format` must match the format the `Renderer` was created with (`Rgba8UnormSrgb` recommended).
    pub fn new(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> OffscreenTarget {
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bm offscreen color"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        OffscreenTarget { width, height, format, color, color_view, depth: DepthTexture::new(device, width, height) }
    }

    pub fn target(&self) -> RenderTarget<'_> {
        RenderTarget { color: &self.color_view, depth: &self.depth.view, width: self.width, height: self.height }
    }

    /// Renders one frame and returns tightly packed RGBA8 (BGRA targets are swizzled).
    pub fn render_rgba(
        &self,
        renderer: &mut Renderer,
        scene: Option<&SceneGpu>,
        camera: &Camera,
        params: &FrameParams,
    ) -> Vec<u8> {
        let device = renderer.device().clone();
        let queue = renderer.queue().clone();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bm offscreen") });
        renderer.render(&mut encoder, &self.target(), scene, camera, params);
        let bpr = (self.width * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bm readback"),
            size: (bpr * self.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(self.height) },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().expect("map callback").expect("map failed");
        let data = slice.get_mapped_range().expect("mapped range");
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height as usize {
            out.extend_from_slice(&data[y * bpr as usize..y * bpr as usize + self.width as usize * 4]);
        }
        drop(data);
        buf.unmap();
        if matches!(self.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
            for p in out.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
        }
        out
    }

    /// Renders and writes a PNG file.
    pub fn render_png(
        &self,
        renderer: &mut Renderer,
        scene: Option<&SceneGpu>,
        camera: &Camera,
        params: &FrameParams,
        path: &std::path::Path,
    ) -> std::io::Result<()> {
        let rgba = self.render_rgba(renderer, scene, camera, params);
        write_png(path, self.width, self.height, &rgba)
    }
}

pub fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(std::io::Error::other)?;
    wr.write_image_data(rgba).map_err(std::io::Error::other)?;
    Ok(())
}
