//! Writes textures of a scene whose name contains the filter as PNGs (scaled up, on a checkerboard-free dark background):
//! `texdump M01 OSD out_dir`.
use std::io::BufWriter;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", a[1])).expect("scene");
    let filter = a[2].to_ascii_lowercase();
    let out = std::path::PathBuf::from(&a[3]);
    std::fs::create_dir_all(&out).unwrap();
    for id in 0..4096u32 {
        let Some(t) = scene.materials.tex().image(id) else { continue };
        if !t.name.to_ascii_lowercase().contains(&filter) {
            continue;
        }
        let Some(img) = scene.materials.image(id) else { continue };
        let s = 4usize;
        let (w, h) = (img.width as usize * s, img.height as usize * s);
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let p = &img.rgba[((y / s) * img.width as usize + x / s) * 4..][..4];
                let a = p[3] as f32 / 255.0;
                let bg = 40.0;
                let o = (y * w + x) * 4;
                for c in 0..3 {
                    buf[o + c] = (p[c] as f32 * a + bg * (1.0 - a)) as u8;
                }
                buf[o + 3] = 255;
            }
        }
        let f = BufWriter::new(std::fs::File::create(out.join(format!("{id}.png"))).unwrap());
        let mut enc = png::Encoder::new(f, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&buf).unwrap();
        println!("{id} {} {}x{}", t.name, img.width, img.height);
    }
}
