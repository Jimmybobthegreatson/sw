//! Scene graph assembled from `.GMS` + `.BUF` + `.PRP` (the "scene loader" view).
//!
//! Join rule (verified on M01, hideout and the front-end scene): PRP node 0 is the implicit `ROOT`
//! (a `ZROOM`, type 0x100021); PRP node `i + 1` is GMS table entry `i`, both lists being in
//! depth-first order. The PRP supplies the hierarchy and the property values, the GMS supplies the
//! type id, the BUF supplies the name.
//!
//! Base properties common to every geometry (medium confidence, cross-checked against the object
//! positions stored in the GMS for the `ZSTDOBJ` entries tested):
//!
//! ```text
//! Token  bounding mode  ("BOUNDING_Static" / "BOUNDING_Dynamic" / ...)
//! Array[9] f32          rotation matrix, row-major as stored
//! Array[3] f32          position
//! ... type specific ...
//! ```

use crate::buf::Buf;
use crate::gms::Gms;
use crate::io::{invalid, Result};
use crate::prp::{Definition, Prp, Value};

pub const TYPE_ROOM: u32 = 0x100021;
pub const TYPE_GROUP: u32 = 0x100001;
pub const TYPE_STDOBJ: u32 = 0x200002;

/// Class names of the most common type ids. The id -> name pairs come from the community type
/// registry (ReGlacier/HBM_GMSTool) and were cross-checked against the node names / structure
/// found in the shipped scenes (e.g. every `0x10044e` node carries a `TimeOutDelete` controller,
/// every `0x800024` node is called `Omni..`). Unlisted ids return `None`.
pub fn type_name(type_id: u32) -> Option<&'static str> {
    Some(match type_id {
        0x100021 => "ZROOM",
        0x100001 => "ZGROUP",
        0x200002 => "ZSTDOBJ",
        0x10003f => "ZBackdrop",
        0x2000ff => "ZParticleEmitter",
        0x2000e1 => "ZBoxPrimitive",
        0x2000ce => "ZScatterObj",
        0x800024 => "ZOMNILIGHT",
        0x800023 => "ZSPOTLIGHT",
        0x800020 => "ZSPOTLIGHTSQUARE",
        0x80000d => "ZENVIRONMENT",
        0x200046 => "ZWINPIC",
        0x200110 => "ZSHADOWMESHOBJ",
        0x20001c => "ZBOUND",
        0x200006 => "ZLNKOBJ",
        0x2003e9 => "ZHM3Actor",
        0x2003e8 => "ZHitman3",
        0x200012 => "ZSNDOBJ",
        0x400003 => "ZCAMERA",
        0x10044e => "ZHM3ClipParticleControl",
        0x1000ca => "ZAllocMany",
        0x200435 => "ZHM3HitmanMovementGuide",
        0x200447 => "ZHM3ClothBundle",
        0x100446 => "ZHM3HmAs",
        0x800001a => "ZLIST",
        _ => return None,
    })
}

#[derive(Clone, Debug)]
pub struct Controller {
    pub name: String,
    pub props: Vec<Value>,
}

#[derive(Clone, Debug)]
pub struct SceneNode {
    /// 0 = ROOT, otherwise GMS entry index + 1.
    pub index: usize,
    pub name: String,
    pub type_id: u32,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub props: Vec<Value>,
    pub controllers: Vec<Controller>,
    /// Rotation matrix (row-major as stored) from the base properties.
    pub rotation: Option<[f32; 9]>,
    /// Position from the base properties.
    pub position: Option<[f32; 3]>,
}

impl SceneNode {
    /// PRM chunk index of the model for `ZSTDOBJ` nodes (props tail `Bool, Int32 prim, Bool`).
    /// Class name of `type_id` if known (see [`type_name`]).
    pub fn type_name(&self) -> Option<&'static str> {
        type_name(self.type_id)
    }

    pub fn stdobj_prim_id(&self) -> Option<u32> {
        if self.type_id != TYPE_STDOBJ || self.props.len() < 6 {
            return None;
        }
        match (&self.props[3], &self.props[4], &self.props[5]) {
            (Value::Bool(_), Value::I32(p), Value::Bool(_)) if *p >= 0 => Some(*p as u32),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SceneGraph {
    pub definitions: Vec<Definition>,
    pub nodes: Vec<SceneNode>,
}

impl SceneGraph {
    pub fn build(gms: &Gms, buf: &Buf, prp: &Prp) -> Result<SceneGraph> {
        let ps = prp.scene()?;
        if ps.nodes.len() != gms.entries.len() + 1 {
            return invalid(format!(
                "PRP has {} nodes but GMS has {} entries (+ROOT expected)",
                ps.nodes.len(),
                gms.entries.len()
            ));
        }
        let mut nodes = Vec::with_capacity(ps.nodes.len());
        for (i, n) in ps.nodes.iter().enumerate() {
            let (name, type_id) = if i == 0 {
                ("ROOT".to_string(), TYPE_ROOM)
            } else {
                let g = gms.geom(i - 1).ok_or_else(|| crate::io::FormatError::Invalid("bad GMS entry".into()))?;
                (buf.name_at(g.name_offset).unwrap_or_default(), g.type_id)
            };
            let props = prp.values(n.props.clone());
            let (rotation, position) = base_transform(&props);
            nodes.push(SceneNode {
                index: i,
                name,
                type_id,
                parent: n.parent,
                children: n.children.clone(),
                props,
                controllers: n
                    .controllers
                    .iter()
                    .map(|c| Controller { name: c.name.clone(), props: prp.values(c.props.clone()) })
                    .collect(),
                rotation,
                position,
            });
        }
        Ok(SceneGraph { definitions: ps.definitions, nodes })
    }

    pub fn root(&self) -> &SceneNode {
        &self.nodes[0]
    }

    /// Slash separated path of node names from the root's children down to `idx`.
    pub fn path(&self, idx: usize) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(idx);
        while let Some(i) = cur {
            if i != 0 {
                parts.push(self.nodes[i].name.as_str());
            }
            cur = self.nodes[i].parent;
        }
        parts.reverse();
        parts.join("/")
    }

    pub fn find_by_name(&self, name: &str) -> Vec<usize> {
        self.nodes.iter().filter(|n| n.name.eq_ignore_ascii_case(name)).map(|n| n.index).collect()
    }

    /// Depth-first iterator over node indices (already the storage order).
    pub fn depth(&self, idx: usize) -> usize {
        let mut d = 0;
        let mut cur = self.nodes[idx].parent;
        while let Some(p) = cur {
            d += 1;
            cur = self.nodes[p].parent;
        }
        d
    }
}

fn base_transform(props: &[Value]) -> (Option<[f32; 9]>, Option<[f32; 3]>) {
    let floats = |v: &Value, n: usize| -> Option<Vec<f32>> {
        let a = v.as_array()?;
        if a.len() != n {
            return None;
        }
        a.iter().map(|x| x.as_f32()).collect()
    };
    if props.len() < 3 {
        return (None, None);
    }
    let rot = floats(&props[1], 9).map(|v| {
        let mut m = [0f32; 9];
        m.copy_from_slice(&v);
        m
    });
    let pos = floats(&props[2], 3).map(|v| [v[0], v[1], v[2]]);
    (rot, pos)
}
