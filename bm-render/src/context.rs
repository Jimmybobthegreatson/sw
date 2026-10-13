//! Headless wgpu bootstrap (tests, tools, offscreen rendering). Windowed apps create their own
//! device/surface and hand them to [`crate::Renderer::new`].

/// Instance + adapter + device + queue.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// True when no real GPU was found and the software (fallback) adapter is used.
    pub is_fallback: bool,
}

impl GpuContext {
    /// Requests a high performance adapter, falling back to the software adapter.
    pub fn new_headless() -> Result<GpuContext, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut is_fallback = false;
        let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        })) {
            Ok(a) => a,
            Err(_) => {
                is_fallback = true;
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: true,
                    compatible_surface: None,
                    ..Default::default()
                }))
                .map_err(|e| format!("no wgpu adapter available: {e}"))?
            }
        };
        let (device, queue) = Self::request_device(&adapter)?;
        Ok(GpuContext { instance, adapter, device, queue, is_fallback })
    }

    /// Device request with the limits the renderer needs (also usable for windowed apps).
    pub fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), String> {
        let mut limits = wgpu::Limits::default();
        let a = adapter.limits();
        limits.max_buffer_size = a.max_buffer_size.min(1 << 30);
        limits.max_texture_dimension_2d = a.max_texture_dimension_2d.min(8192);
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("bm-render device"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            ..Default::default()
        }))
        .map_err(|e| format!("request_device failed: {e}"))
    }

    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }
}
