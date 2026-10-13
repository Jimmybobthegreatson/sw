use bm_models::scene::Scene;
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let mc: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(21516);
    let vfs = Vfs::from_env().expect("game dir");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let prm = &scene.prm;
    let m = prm.model(mc).unwrap();
    let skel = bm_models::skeleton::Skeleton::from_prm(prm, m.parts_chunk).unwrap();
    for (i, b) in skel.bones.iter().enumerate() { print!("{i}:{} ", b.name); }
    println!();
    for (i, b) in skel.bones.iter().enumerate().filter(|(_, b)| b.name.starts_with("FA_") || b.name=="HEAD") {
        let w = &b.bind_world; let l = &b.bind_local;
        println!("{i} {} parent {:?} local.t {:?} | world cols x{:?} y{:?} z{:?} | world t {:?}", b.name, b.parent,
            l.translation(), w.transform_vector([1.0,0.0,0.0]), w.transform_vector([0.0,1.0,0.0]), w.transform_vector([0.0,0.0,1.0]), w.translation());
    }
    let bank = scene.anim_bank().unwrap();
    for (i, n) in bank.facial_poses.iter().enumerate() { print!("{i}:{} ", n); }
    println!();
    let parts = prm.chunk_words(m.parts_chunk as usize).unwrap();
    let lw = prm.chunk_words(parts[1] as usize).unwrap();
    let h = prm.chunk_words(lw[11] as usize).unwrap();
    let data = prm.chunk(h[0] as usize).unwrap();
    let offs = prm.chunk_words(h[1] as usize).unwrap();
    let f = |o: usize| f32::from_le_bytes(data[o..o+4].try_into().unwrap());
    let u = |o: usize| u32::from_le_bytes(data[o..o+4].try_into().unwrap());
    let np = h[4] as usize;
    let mapb = prm.chunk(h[3] as usize).unwrap();
    let map: Vec<u16> = mapb.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    
    for p in 0..np.min(offs.len()-1) {
        let (s, e) = (offs[p] as usize, offs[p+1] as usize);
        let nm = map.iter().position(|&m| m as usize == p).map(|i| bank.facial_poses.get(i).cloned().unwrap_or_default()).unwrap_or_default();
        println!("slot {p} name '{nm}' records {s}..{e}");
        for r in s..e {
            let o = r * 44;
            let id = u(o + 40) as usize;
            println!("   bone {id:3} {:<22} q({:+.3},{:+.3},{:+.3},{:+.3}) t({:+.3},{:+.3},{:+.3}) s({:.2},{:.2},{:.2})",
                skel.bones.get(id).map(|b| b.name.as_str()).unwrap_or("?"), f(o), f(o+4), f(o+8), f(o+12), f(o+16), f(o+20), f(o+24), f(o+28), f(o+32), f(o+36));
        }
    }
}

#[allow(dead_code)]
fn axes() {}
