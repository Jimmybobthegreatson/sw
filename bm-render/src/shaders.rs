//! WGSL module registry with VFS loose-file override and hot reload.

use std::collections::HashMap;
use std::sync::Arc;

use bm_vfs::Vfs;

/// VFS path of the builtin uber shader. A mod overrides it by shipping `Shaders/WGSL/main.wgsl`.
pub const BUILTIN_PATH: &str = "Shaders/WGSL/main.wgsl";
/// Embedded source of the builtin shader (also the template for modders).
pub const BUILTIN_SOURCE: &str = include_str!("shaders/main.wgsl");
/// VFS path / source of the present pass (display space -> target format, FXAA). Overridable like the main shader.
pub const POST_PATH: &str = "Shaders/WGSL/post.wgsl";
pub const POST_SOURCE: &str = include_str!("shaders/post.wgsl");

pub struct ShaderEntry {
    pub path: String,
    pub source: String,
    pub module: wgpu::ShaderModule,
}

/// Index of the builtin shader in [`ShaderLib::entries`].
pub const BUILTIN: usize = 0;
/// Index of the present-pass shader.
pub const POST: usize = 1;

pub struct ShaderLib {
    pub entries: Vec<ShaderEntry>,
    map: HashMap<String, usize>,
}

/// Compiles WGSL and returns validation errors instead of panicking.
pub fn compile(device: &wgpu::Device, label: &str, src: &str) -> Result<wgpu::ShaderModule, String> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    match pollster::block_on(scope.pop()) {
        Some(e) => Err(format!("{label}: {e}")),
        None => Ok(module),
    }
}

impl ShaderLib {
    pub fn new(device: &wgpu::Device, vfs: Option<&Arc<Vfs>>) -> ShaderLib {
        let mut lib = ShaderLib { entries: Vec::new(), map: HashMap::new() };
        let mut source = BUILTIN_SOURCE.to_string();
        if let Some(v) = vfs {
            if let Some(s) = read_text(v, BUILTIN_PATH) {
                source = s;
            }
        }
        let module = match compile(device, BUILTIN_PATH, &source) {
            Ok(m) => m,
            Err(e) => {
                log::error!("override of builtin shader rejected: {e}");
                source = BUILTIN_SOURCE.to_string();
                compile(device, BUILTIN_PATH, &source).expect("builtin shader must compile")
            }
        };
        lib.entries.push(ShaderEntry { path: BUILTIN_PATH.into(), source, module });
        lib.map.insert(BUILTIN_PATH.to_ascii_lowercase(), BUILTIN);

        let mut psource = POST_SOURCE.to_string();
        if let Some(v) = vfs {
            if let Some(s) = read_text(v, POST_PATH) {
                psource = s;
            }
        }
        let pmodule = match compile(device, POST_PATH, &psource) {
            Ok(m) => m,
            Err(e) => {
                log::error!("override of the present shader rejected: {e}");
                psource = POST_SOURCE.to_string();
                compile(device, POST_PATH, &psource).expect("builtin post shader must compile")
            }
        };
        lib.entries.push(ShaderEntry { path: POST_PATH.into(), source: psource, module: pmodule });
        lib.map.insert(POST_PATH.to_ascii_lowercase(), POST);
        lib
    }

    /// Loads (or returns the cached) shader module at a VFS path; `None` if missing / invalid.
    pub fn ensure(&mut self, device: &wgpu::Device, vfs: Option<&Arc<Vfs>>, path: &str) -> Option<usize> {
        let key = path.to_ascii_lowercase();
        if let Some(&i) = self.map.get(&key) {
            return Some(i);
        }
        let src = read_text(vfs?, path)?;
        match compile(device, path, &src) {
            Ok(module) => {
                self.entries.push(ShaderEntry { path: path.to_string(), source: src, module });
                self.map.insert(key, self.entries.len() - 1);
                Some(self.entries.len() - 1)
            }
            Err(e) => {
                log::error!("custom shader rejected: {e}");
                None
            }
        }
    }

    /// Re-reads every loaded shader from the VFS; returns `(any_changed, errors)`. Modules that fail to
    /// compile keep their previous version.
    pub fn reload(&mut self, device: &wgpu::Device, vfs: &Arc<Vfs>) -> (bool, Vec<String>) {
        let mut changed = false;
        let mut errors = Vec::new();
        for e in &mut self.entries {
            let new_src = match read_text(vfs, &e.path) {
                Some(s) => s,
                None if e.path == BUILTIN_PATH => BUILTIN_SOURCE.to_string(),
                None if e.path == POST_PATH => POST_SOURCE.to_string(),
                None => continue,
            };
            if new_src == e.source {
                continue;
            }
            match compile(device, &e.path, &new_src) {
                Ok(m) => {
                    e.module = m;
                    e.source = new_src;
                    changed = true;
                }
                Err(err) => {
                    // remember the broken source so we do not re-report it every poll
                    e.source = new_src;
                    errors.push(err);
                }
            }
        }
        (changed, errors)
    }
}

fn read_text(vfs: &Vfs, path: &str) -> Option<String> {
    String::from_utf8(vfs.open(path).ok()?).ok()
}
