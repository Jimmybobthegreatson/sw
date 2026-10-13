//! Room containment from the decoded `.RMI` map (`bm_formats::roommap`): which `ZROOM` is a point in?
//!
//! The `.RMI` file stores one box per room volume (the `InsideBound*` `ZBOUND` nodes below each `ZROOM`, plus a few
//! helper boxes). Boxes are matched to the scene graph by world centre and volume, which gives every box its `ZROOM`
//! (name, location group, inside / outside flag). A point's room is the smallest box that contains it.
//!
//! Consumers: `bm-sound` reverb (`SndDb::room_effects` are named after the rooms), `bm-music` zones
//! (`MusicLocationDef` lists room names), AI / trespassing (`ZoneControl` lists room groups per security zone).

use crate::math::Vec3;
use bm_formats::roommap::RoomMap;
use bm_formats::scene::TYPE_ROOM;
use bm_models::Scene;
use std::collections::HashMap;

/// One `ZROOM` of the scene with at least one volume in the room map.
#[derive(Clone, Debug)]
pub struct RoomDef {
    /// `ZROOM` node index.
    pub node: usize,
    /// Last path segment, e.g. `M01_Hacienda_Room_101`.
    pub name: String,
    /// Name of the group the room lives in (`M01_LOC_Ins_Hacienda_1stFloor`, `M01_LOC_Outside`).
    pub location: String,
    /// `eINSIDE` rooms (interiors) as opposed to open areas.
    pub inside: bool,
    /// Indices of the `.RMI` records that belong to this room.
    pub volumes: Vec<usize>,
}

/// A door opening between two rooms (a plane of the decoded `.SGP`).
#[derive(Clone, Debug)]
pub struct RoomPortal {
    /// Room indices (into [`RoomIndex::rooms`]).
    pub a: usize,
    pub b: usize,
    pub center: Vec3,
    pub normal: Vec3,
    pub corners: [Vec3; 4],
}

/// Room lookup over the `.RMI` boxes of a level.
#[derive(Clone, Debug, Default)]
pub struct RoomIndex {
    pub rooms: Vec<RoomDef>,
    /// Record index -> room index (`None` for helper boxes that match no `InsideBound` node).
    owner: Vec<Option<u32>>,
    map: Option<RoomMap>,
    by_name: HashMap<String, usize>,
    /// Number of `.RMI` records / how many of them were matched to a room (diagnostics).
    pub records: usize,
    pub matched: usize,
    /// Diagnostics: `ZROOM`s flagged inside / outside, distinct record ids, and whether the id-order rule was used.
    pub room_nodes: usize,
    pub distinct_ids: usize,
    pub by_order: bool,
    /// Portals between rooms (`.SGP`), see [`RoomIndex::neighbors`].
    pub portals: Vec<RoomPortal>,
}

fn last_segment(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

impl RoomIndex {
    /// Builds the index from the scene and its parsed `.RMI`.
    ///
    /// Box -> room assignment (see `docs/formats.md`): every record carries a room `id`; the distinct ids in ascending
    /// order correspond to the `ZROOM` nodes with a positive volume count (property 11) in node order (checked against the boxes
    /// that can be matched to `InsideBound*` nodes by world centre and volume). When the counts do not fit, only the
    /// matched boxes (and boxes sharing their id) are used.
    pub fn build(scene: &Scene, rmi: Option<RoomMap>) -> RoomIndex {
        let Some(map) = rmi else { return RoomIndex::default() };
        let token_of = |node: usize| -> Option<&str> {
            scene.graph.nodes[node].props.iter().find_map(|v| match v {
                bm_formats::prp::Value::Token(t) if t == "eINSIDE" || t == "eOUTSIDE" => Some(t.as_str()),
                _ => None,
            })
        };
        // real rooms in node order
        let room_nodes: Vec<usize> = scene.nodes.iter().filter(|n| n.type_id == TYPE_ROOM && scene.graph.nodes[n.index].props.get(11).and_then(|v| v.as_i32()).unwrap_or(0) > 0).map(|n| n.index).collect();

        // anchors: ZBOUND volumes matched to records
        let mut cands: Vec<([f32; 3], f32, usize)> = Vec::new();
        for n in &scene.nodes {
            if n.type_id != 0x20001c {
                continue;
            }
            let Some(m) = n.prim_id.and_then(|p| scene.prm.model(p as usize)) else { continue };
            let mut room = None;
            let mut cur = n.parent;
            while let Some(p) = cur {
                if scene.nodes[p].type_id == TYPE_ROOM {
                    room = Some(p);
                    break;
                }
                cur = scene.nodes[p].parent;
            }
            let Some(room) = room else { continue };
            let c = [(m.bbox_min[0] + m.bbox_max[0]) * 0.5, (m.bbox_min[1] + m.bbox_max[1]) * 0.5, (m.bbox_min[2] + m.bbox_max[2]) * 0.5];
            let vol = (0..3).map(|k| (m.bbox_max[k] - m.bbox_min[k]).abs()).product::<f32>();
            cands.push((n.world.transform_point(c), vol, room));
        }
        let mut anchor: Vec<Option<usize>> = vec![None; map.records.len()];
        for (ri, r) in map.records.iter().enumerate() {
            let vol = r.volume();
            let d2 = |c: &[f32; 3]| (0..3).map(|k| (c[k] - r.center[k]).powi(2)).sum::<f32>();
            anchor[ri] = cands
                .iter()
                .filter(|c| d2(&c.0).sqrt() < 2.0 && (c.1 - vol).abs() <= 0.02 * vol.max(1.0) + 1.0)
                .min_by(|a, b| d2(&a.0).total_cmp(&d2(&b.0)))
                .map(|c| c.2);
        }

        // ids -> rooms by order
        let mut ids: Vec<u32> = map.records.iter().filter(|r| r.volume() > 0.0).map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        let mut id_room: HashMap<u32, usize> = HashMap::new();
        let mut by_order = false;
        // anchored position (index into `room_nodes`) of every id
        let pos_of_room: HashMap<usize, usize> = room_nodes.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let mut anchored: Vec<Option<usize>> = vec![None; ids.len()];
        for (ri, r) in map.records.iter().enumerate() {
            if let (Some(a), Ok(i)) = (anchor[ri], ids.binary_search(&r.id)) {
                anchored[i] = anchored[i].or(pos_of_room.get(&a).copied());
            }
        }
        let known: Vec<(usize, usize)> = anchored.iter().enumerate().filter_map(|(i, p)| p.map(|p| (i, p))).collect();
        if known.windows(2).all(|w| w[0].1 < w[1].1) {
            // fill the gaps between anchors where the number of ids equals the number of rooms
            let mut bounds: Vec<(isize, isize)> = vec![(-1, -1)];
            bounds.extend(known.iter().map(|&(i, p)| (i as isize, p as isize)));
            bounds.push((ids.len() as isize, room_nodes.len() as isize));
            let mut ok = true;
            for w in bounds.windows(2) {
                let (i0, p0) = w[0];
                let (i1, p1) = w[1];
                let (n, m) = (i1 - i0 - 1, p1 - p0 - 1);
                if n == m {
                    for k in 1..=n {
                        id_room.insert(ids[(i0 + k) as usize], room_nodes[(p0 + k) as usize]);
                    }
                } else {
                    ok = false;
                }
                if i1 < ids.len() as isize {
                    id_room.insert(ids[i1 as usize], room_nodes[p1 as usize]);
                }
            }
            by_order = ok;
        }
        if id_room.is_empty() || !by_order {
            for (ri, r) in map.records.iter().enumerate() {
                if let Some(a) = anchor[ri] {
                    id_room.entry(r.id).or_insert(a);
                }
            }
        }
        // final consistency: matched boxes must agree with the assignment
        if !map.records.iter().enumerate().all(|(ri, r)| anchor[ri].is_none_or(|a| id_room.get(&r.id) == Some(&a))) {
            log::warn!("rooms: id order disagrees with matched InsideBound volumes; using matched boxes only");
            id_room.clear();
            by_order = false;
            for (ri, r) in map.records.iter().enumerate() {
                if let Some(a) = anchor[ri] {
                    id_room.entry(r.id).or_insert(a);
                }
            }
        }

        let mut room_idx: HashMap<usize, usize> = HashMap::new();
        let mut rooms: Vec<RoomDef> = Vec::new();
        let mut owner: Vec<Option<u32>> = vec![None; map.records.len()];
        let mut matched = 0;
        for (ri, r) in map.records.iter().enumerate() {
            let Some(&node) = id_room.get(&r.id) else { continue };
            let idx = *room_idx.entry(node).or_insert_with(|| {
                let n = &scene.nodes[node];
                let loc = n.parent.map(|p| last_segment(&scene.path(p)).to_string()).unwrap_or_default();
                rooms.push(RoomDef { node, name: n.name.clone(), location: loc, inside: token_of(node) == Some("eINSIDE"), volumes: Vec::new() });
                rooms.len() - 1
            });
            rooms[idx].volumes.push(ri);
            owner[ri] = Some(idx as u32);
            matched += 1;
        }
        let by_name = rooms.iter().enumerate().map(|(i, r)| (r.name.to_ascii_lowercase(), i)).collect();
        RoomIndex { rooms, owner, records: map.records.len(), matched, map: Some(map), by_name, room_nodes: room_nodes.len(), distinct_ids: ids.len(), by_order, portals: Vec::new() }
    }

    /// Attaches the portal graph of the level's `.SGP`: paths are matched to rooms by their last segment.
    pub fn with_portals(mut self, sgp: Option<&bm_formats::sgp::Sgp>) -> RoomIndex {
        let Some(sgp) = sgp else { return self };
        let room_of_path: Vec<Option<usize>> = sgp.paths.iter().map(|(p, _)| self.find(p)).collect();
        self.portals = sgp
            .portals()
            .into_iter()
            .filter_map(|p| {
                let (a, b) = (room_of_path[p.from]?, room_of_path[p.to]?);
                let v = |c: [f32; 3]| Vec3::from_array(c);
                Some(RoomPortal { a, b, center: v(p.center), normal: v(p.normal), corners: [v(p.corners[0]), v(p.corners[1]), v(p.corners[2]), v(p.corners[3])] })
            })
            .collect();
        self
    }

    /// Rooms connected to `room` by a portal (sorted, without duplicates).
    pub fn neighbors(&self, room: usize) -> Vec<usize> {
        let mut v: Vec<usize> = self.portals.iter().filter_map(|p| if p.a == room { Some(p.b) } else if p.b == room { Some(p.a) } else { None }).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Portals between two rooms.
    pub fn portals_between(&self, a: usize, b: usize) -> impl Iterator<Item = &RoomPortal> {
        self.portals.iter().filter(move |p| (p.a == a && p.b == b) || (p.a == b && p.b == a))
    }

    /// Shortest chain of rooms (fewest portals) from `from` to `to`, both included.
    pub fn route(&self, from: usize, to: usize) -> Option<Vec<usize>> {
        if from == to {
            return Some(vec![from]);
        }
        let mut prev: HashMap<usize, usize> = HashMap::new();
        let mut queue = std::collections::VecDeque::from([from]);
        while let Some(r) = queue.pop_front() {
            for n in self.neighbors(r) {
                if n == from || prev.contains_key(&n) {
                    continue;
                }
                prev.insert(n, r);
                if n == to {
                    let mut path = vec![to];
                    let mut cur = to;
                    while let Some(&p) = prev.get(&cur) {
                        path.push(p);
                        cur = p;
                    }
                    path.reverse();
                    return Some(path);
                }
                queue.push_back(n);
            }
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    /// Room index (into [`RoomIndex::rooms`]) at a world position: the smallest owned box that contains it.
    pub fn room_at(&self, p: Vec3) -> Option<usize> {
        let map = self.map.as_ref()?;
        map.records_at(p.to_array())
            .into_iter()
            .filter(|&ri| self.owner[ri].is_some())
            .min_by(|&a, &b| map.records[a].volume().total_cmp(&map.records[b].volume()))
            .and_then(|ri| self.owner[ri].map(|o| o as usize))
    }

    pub fn room(&self, idx: usize) -> Option<&RoomDef> {
        self.rooms.get(idx)
    }

    /// Name of the room at `p` (`M01_Hacienda_Room_101`).
    pub fn name_at(&self, p: Vec3) -> Option<&str> {
        self.room_at(p).map(|i| self.rooms[i].name.as_str())
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.by_name.get(&last_segment(name).to_ascii_lowercase()).copied()
    }

    /// Room map the index was built from.
    pub fn map(&self) -> Option<&RoomMap> {
        self.map.as_ref()
    }

    /// True if `p` is inside a room flagged `eINSIDE`.
    pub fn is_inside(&self, p: Vec3) -> bool {
        self.room_at(p).is_some_and(|i| self.rooms[i].inside)
    }
}
