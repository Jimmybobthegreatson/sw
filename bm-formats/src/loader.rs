//! Convenience loader: reads and parses every file of a scene through a [`bm_vfs::Vfs`]
//! (so loose-file mods override archive contents) and joins GMS/BUF/PRP into a [`SceneGraph`].

use std::fmt;

use bm_vfs::{Vfs, VfsError};

use crate::anm::Anm;
use crate::buf::Buf;
use crate::gms::Gms;
use crate::io::FormatError;
use crate::loc::Loc;
use crate::mat::Mat;
use crate::misc::RoomMap;
use crate::oct::Oct;
use crate::prm::Prm;
use crate::prp::Prp;
use crate::scene::SceneGraph;
use crate::sgp::Sgp;
use crate::snd::Snd;
use crate::tex::Tex;

#[derive(Debug)]
pub enum LoadError {
    Vfs(VfsError),
    Format { file: String, error: FormatError },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Vfs(e) => write!(f, "{e}"),
            LoadError::Format { file, error } => write!(f, "{file}: {error}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<VfsError> for LoadError {
    fn from(e: VfsError) -> Self {
        LoadError::Vfs(e)
    }
}

/// All parsed files of one scene (`Scenes/<dir>/<name>.ZIP`).
pub struct SceneFiles {
    /// Scene path without extension, e.g. `Scenes/M01/M01_main`.
    pub base: String,
    pub gms: Gms,
    pub buf: Buf,
    pub prp: Prp,
    pub prm: Prm,
    pub tex: Tex,
    pub mat: Mat,
    /// Absent in loader scenes.
    pub loc: Option<Loc>,
    pub anm: Anm,
    pub oct: Oct,
    pub sgp: Sgp,
    pub snd: Snd,
    pub rmc: RoomMap,
    pub rmi: RoomMap,
}

fn fmt_err(file: &str) -> impl Fn(FormatError) -> LoadError + '_ {
    move |error| LoadError::Format { file: file.to_string(), error }
}

impl SceneFiles {
    pub fn load(vfs: &Vfs, base: &str) -> Result<SceneFiles, LoadError> {
        let get = |ext: &str| -> Result<(String, Vec<u8>), LoadError> {
            let path = format!("{base}.{ext}");
            let bytes = vfs.open(&path)?;
            Ok((path, bytes))
        };
        let (p, b) = get("GMS")?;
        let gms = Gms::parse(&b).map_err(fmt_err(&p))?;
        let (p, b) = get("BUF")?;
        let buf = Buf::parse(&b).map_err(fmt_err(&p))?;
        let (p, b) = get("PRP")?;
        let prp = Prp::parse(&b).map_err(fmt_err(&p))?;
        let (p, b) = get("PRM")?;
        let prm = Prm::parse(&b).map_err(fmt_err(&p))?;
        let (p, b) = get("TEX")?;
        let tex = Tex::parse(&b).map_err(fmt_err(&p))?;
        let (p, b) = get("MAT")?;
        let mat = Mat::parse(&b).map_err(fmt_err(&p))?;
        let loc = if vfs.exists(&format!("{base}.LOC")) {
            let (p, b) = get("LOC")?;
            Some(Loc::parse(&b).map_err(fmt_err(&p))?)
        } else {
            None
        };
        let (p, b) = get("ANM")?;
        let anm = Anm::parse(&b).map_err(fmt_err(&p))?;
        let (p, b) = get("OCT")?;
        let oct = Oct::parse(&b).map_err(fmt_err(&p))?;
        let (p, b) = get("SGP")?;
        let sgp = Sgp::parse(&b).map_err(fmt_err(&p))?;
        let (p, b) = get("SND")?;
        let snd = Snd::parse(&b).map_err(fmt_err(&p))?;
        let (p, b) = get("RMC")?;
        let rmc = RoomMap::parse(&b).map_err(fmt_err(&p))?;
        let (p, b) = get("RMI")?;
        let rmi = RoomMap::parse(&b).map_err(fmt_err(&p))?;
        Ok(SceneFiles { base: base.to_string(), gms, buf, prp, prm, tex, mat, loc, anm, oct, sgp, snd, rmc, rmi })
    }

    /// Joins GMS + BUF + PRP into the scene tree.
    pub fn graph(&self) -> Result<SceneGraph, FormatError> {
        SceneGraph::build(&self.gms, &self.buf, &self.prp)
    }
}
