//! `cargo run -p bm-formats --example matdump -- Scenes/M01/M01_main 286` - prints a material's property tree.
use bm_formats::mat::{Mat, MatProp, MatValue};
use bm_vfs::Vfs;

fn show(p: &MatProp, ind: usize) {
    match &p.value {
        MatValue::Struct(c) => {
            println!("{:ind$}{} {{", "", p.tag, ind = ind);
            c.iter().for_each(|k| show(k, ind + 2));
            println!("{:ind$}}}", "", ind = ind);
        }
        v => println!("{:ind$}{} = {:?}", "", p.tag, v, ind = ind),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let mat = Mat::parse(&vfs.open(&format!("{}.MAT", a[0])).unwrap()).unwrap();
    let id: u32 = a[1].parse().unwrap();
    let m = mat.instance(id).expect("no such material");
    println!("material {id} \"{}\" at {:#x} ({} bytes), shader {:?}", m.name, m.offset, m.len, m.shader(&mat));
    for p in m.props(&mat) {
        show(&p, 2);
    }
    println!("textures: {:#?}", m.textures(&mat));
}
