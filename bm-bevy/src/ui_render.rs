//! Draws a `bm-ui` [`DrawList`] with Bevy: one pooled mesh entity per draw command, unlit, seen by an orthographic
//! overlay camera on its own render layer (screen pixels, y down, like the wgpu back end it replaces).
//!
//! Not supported (yet): per-command scissor rectangles.

use bevy::asset::RenderAssetUsages;
use bevy::camera::ScalingMode;
use bevy::camera::visibility::RenderLayers;
use bevy::image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bm_ui::{Blend, DrawList, TextureId, TextureStore};
use std::collections::HashMap;

pub const UI_LAYER: usize = 1;

#[derive(Component)]
pub struct UiCamera;

pub fn spawn_camera(commands: &mut Commands) {
    commands.spawn((
        Camera3d::default(),
        Camera { order: 1, clear_color: ClearColorConfig::None, ..default() },
        Projection::Orthographic(OrthographicProjection {
            near: -1000.0,
            far: 1000.0,
            scaling_mode: ScalingMode::WindowSize,
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 0.0, 10.0),
        Msaa::Sample4,
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        RenderLayers::layer(UI_LAYER),
        UiCamera,
    ));
}

struct Slot {
    entity: Entity,
    mesh: Handle<Mesh>,
}

#[derive(Default)]
pub struct UiRenderer {
    slots: Vec<Slot>,
    images: HashMap<(TextureId, u32), Handle<Image>>,
    materials: HashMap<(TextureId, Blend, u32), Handle<StandardMaterial>>,
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

impl UiRenderer {
    fn material(&mut self, store: &TextureStore, tex: TextureId, blend: Blend, images: &mut Assets<Image>, mats: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
        let ver = store.version(tex);
        if let Some(m) = self.materials.get(&(tex, blend, ver)) {
            return m.clone();
        }
        let image = self
            .images
            .entry((tex, ver))
            .or_insert_with(|| {
                let src = store.get(tex).expect("texture in store");
                let filter = if src.nearest { ImageFilterMode::Nearest } else { ImageFilterMode::Linear };
                let mut img = Image::new(
                    Extent3d { width: src.width, height: src.height, depth_or_array_layers: 1 },
                    TextureDimension::D2,
                    src.rgba.clone(),
                    TextureFormat::Rgba8UnormSrgb,
                    RenderAssetUsages::RENDER_WORLD,
                );
                img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor { mag_filter: filter, min_filter: filter, ..default() });
                images.add(img)
            })
            .clone();
        let m = mats.add(StandardMaterial {
            base_color_texture: Some(image),
            unlit: true,
            double_sided: true,
            cull_mode: None,
            fog_enabled: false,
            alpha_mode: match blend {
                Blend::Alpha => AlphaMode::Blend,
                Blend::Additive => AlphaMode::Add,
            },
            ..default()
        });
        self.materials.insert((tex, blend, ver), m.clone());
        m
    }

    /// Updates the pooled entities to show `dl` on a `screen`-sized (logical pixels) window.
    pub fn draw(
        &mut self,
        dl: &DrawList,
        store: &TextureStore,
        screen: Vec2,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        images: &mut Assets<Image>,
        mats: &mut Assets<StandardMaterial>,
        slots_q: &mut Query<(&mut Mesh3d, &mut MeshMaterial3d<StandardMaterial>, &mut Transform, &mut Visibility), With<UiQuad>>,
    ) {
        let half = screen * 0.5;
        for (i, cmd) in dl.cmds.iter().enumerate() {
            while self.slots.len() <= i {
                let mesh = meshes.add(Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default()));
                let entity = commands
                    .spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d::<StandardMaterial>::default(),
                        Transform::default(),
                        Visibility::Hidden,
                        RenderLayers::layer(UI_LAYER),
                        bevy::camera::visibility::NoFrustumCulling,
                        UiQuad,
                    ))
                    .id();
                self.slots.push(Slot { entity, mesh });
            }
            let tex = if store.get(cmd.texture).is_some() { cmd.texture } else { TextureId::WHITE };
            let material = self.material(store, tex, cmd.blend, images, mats);
            let slot = &self.slots[i];
            // the vertices of one command are contiguous in `indices`; re-index them into a compact buffer
            let r = cmd.indices.start as usize..cmd.indices.end as usize;
            let (mut pos, mut uv, mut col, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let mut remap: HashMap<u32, u32> = HashMap::new();
            for &ix in &dl.indices[r] {
                let n = *remap.entry(ix).or_insert_with(|| {
                    let v = &dl.vertices[ix as usize];
                    pos.push([v.pos[0] - half.x, half.y - v.pos[1], 0.0]);
                    uv.push(v.uv);
                    col.push([srgb_to_linear(v.color[0]), srgb_to_linear(v.color[1]), srgb_to_linear(v.color[2]), v.color[3]]);
                    pos.len() as u32 - 1
                });
                idx.push(n);
            }
            if let Some(mut m) = meshes.get_mut(&slot.mesh) {
                m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
                m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
                m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
                m.insert_indices(Indices::U32(idx));
            }
            if let Ok((_, mut mm, mut tr, mut vis)) = slots_q.get_mut(slot.entity) {
                mm.0 = material;
                // later commands are nearer to the camera so that blended quads sort back to front
                tr.translation.z = i as f32 * 0.01;
                *vis = Visibility::Inherited;
            }
        }
        for slot in self.slots.iter().skip(dl.cmds.len()) {
            if let Ok((_, _, _, mut vis)) = slots_q.get_mut(slot.entity) {
                *vis = Visibility::Hidden;
            }
        }
    }
}

#[derive(Component)]
pub struct UiQuad;
