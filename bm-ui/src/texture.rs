//! CPU-side texture registry shared by fonts, sprites and the GPU renderer.
//!
//! The UI logic only ever deals in [`TextureId`] handles; the wgpu renderer uploads the images lazily (and
//! re-uploads them when [`TextureStore::replace`] bumps the version). That keeps layout and drawing
//! renderer-agnostic and unit-testable without a GPU.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureId(pub u32);

impl TextureId {
    /// 1x1 opaque white texture (solid rectangles).
    pub const WHITE: TextureId = TextureId(0);
}

/// RGBA8 image, straight alpha, non-premultiplied, sRGB-encoded values as authored.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub label: String,
    /// Use nearest sampling (pixel-exact UI art / bitmap fonts at 1:1).
    pub nearest: bool,
}

impl Image {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>, label: impl Into<String>) -> Image {
        assert_eq!(rgba.len(), (width * height * 4) as usize, "rgba size mismatch");
        Image { width, height, rgba, label: label.into(), nearest: false }
    }

    pub fn solid(width: u32, height: u32, c: [u8; 4], label: impl Into<String>) -> Image {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            rgba.extend_from_slice(&c);
        }
        Image::new(width, height, rgba, label)
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

#[derive(Debug)]
pub struct TextureStore {
    images: Vec<Image>,
    versions: Vec<u32>,
    by_label: HashMap<String, TextureId>,
}

impl Default for TextureStore {
    fn default() -> Self {
        TextureStore::new()
    }
}

impl TextureStore {
    pub fn new() -> TextureStore {
        let mut s = TextureStore { images: Vec::new(), versions: Vec::new(), by_label: HashMap::new() };
        s.images.push(Image::solid(1, 1, [255; 4], "white"));
        s.versions.push(1);
        s
    }

    pub fn add(&mut self, img: Image) -> TextureId {
        let id = TextureId(self.images.len() as u32);
        self.by_label.insert(img.label.to_ascii_lowercase(), id);
        self.images.push(img);
        self.versions.push(1);
        id
    }

    /// Adds or, when a texture with the same label exists, replaces it in place (hot reload).
    pub fn add_or_replace(&mut self, img: Image) -> TextureId {
        if let Some(&id) = self.by_label.get(&img.label.to_ascii_lowercase()) {
            self.replace(id, img);
            id
        } else {
            self.add(img)
        }
    }

    pub fn replace(&mut self, id: TextureId, img: Image) {
        let i = id.0 as usize;
        if i < self.images.len() {
            self.images[i] = img;
            self.versions[i] += 1;
        }
    }

    pub fn get(&self, id: TextureId) -> Option<&Image> {
        self.images.get(id.0 as usize)
    }

    pub fn version(&self, id: TextureId) -> u32 {
        self.versions.get(id.0 as usize).copied().unwrap_or(0)
    }

    pub fn find(&self, label: &str) -> Option<TextureId> {
        self.by_label.get(&label.to_ascii_lowercase()).copied()
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = TextureId> + '_ {
        (0..self.images.len() as u32).map(TextureId)
    }
}
