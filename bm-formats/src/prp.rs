//! `.PRP` - "IOPacked" property stream: the serialized property values of every scene object.
//!
//! # File layout (confidence: high; round-trips byte-exactly on all shipped scenes)
//!
//! ```text
//! 0x00  "IOPacked v0.1\0"      14 bytes
//! 0x0e  u8  is_raw             0 in every file seen (meaning of 1 unknown)
//! 0x0f  u32 flags              13 = 0b1101 in every file seen
//!                               bit 2: 0x0e/0x0f ops carry string tokens, bit 3: strings are
//!                               stored as u32 indices into the string table (instead of inline)
//! 0x13  u32 unk                0
//! 0x17  u32 key_count          string table holds key_count + 1 strings
//! 0x1b  u32 table_bytes        byte length of the string table
//! 0x1f  string table           NUL separated strings (identifiers, enum names, resource paths)
//! ....  u32 object_count       number of BeginObject ops in the stream
//! ....  op stream              until EndOfStream (0x7f)
//! ```
//!
//! # Op stream (high)
//!
//! One opcode byte, then a payload. `op | 0x80` is the "named" twin of ops 1..=0x10 with the same
//! payload (never seen in shipped files).
//!
//! | op | name | payload |
//! |---|---|---|
//! | 0x01 | Array | u32 length; elements follow until `0x7c` |
//! | 0x02 | BeginObject | - (closed by `0x7e`) |
//! | 0x03 | Reference | u32 (**unverified**) |
//! | 0x04 | Container | u32 count (items follow, no terminator) |
//! | 0x05 | Char | u8 |
//! | 0x06 | Bool | u8 |
//! | 0x07 | Int8 | i8 |
//! | 0x08 | Int16 | i16 |
//! | 0x09 | Int32 | i32 |
//! | 0x0a | Float32 | f32 |
//! | 0x0b | Float64 | f64 |
//! | 0x0c | String | u32 string-table index (flag bit 3) or u32 length + bytes |
//! | 0x0d | RawData | u32 length + bytes |
//! | 0x0e | StringToken | like String when flag bit 2, else raw u32 |
//! | 0x0f | StringArray | with flag bits 2+3: u32 count + count x u32 index; else u32 |
//! | 0x10 | Bitfield | u32 (**unverified**) |
//! | 0x7c | EndArray | - |
//! | 0x7d | SkipMark | - |
//! | 0x7e | EndObject | - |
//! | 0x7f | EndOfStream | - |
//!
//! # Meaning
//!
//! The stream starts with a *definitions* `Container` (scene-wide defines such as `CameraFOV`,
//! each: String name, Int32 kind, payload: kind 2 = int array, 3 = float array, 12/14/16 = string,
//! 17 = string table), followed by the scene tree in depth-first order. Every tree node is
//! `Object(props) Container(controllers) Container(children)` where each controller is
//! `String(name) Object(props)` and the children are inlined nodes (see [`Prp::scene`]).
//!
//! Property objects carry **no field names**; their meaning depends on the entity type (taken
//! from the GMS type id). For every geometry type the first fields are the ZGEOM base:
//! `StringToken(bounding mode)`, `Array[9] f32` (rotation matrix), `Array[3] f32` (position),
//! then type-specific values. For `ZSTDOBJ` (0x200002) the tail `Bool, Int32 primitive, Bool`
//! gives the index of the PRM model chunk (see `scene.rs`).

use crate::io::{from_latin1, invalid, latin1, Cur, FormatError, PutLe, Result};

pub const MAGIC: &[u8; 14] = b"IOPacked v0.1\0";

pub const OP_ARRAY: u8 = 0x01;
pub const OP_BEGIN_OBJECT: u8 = 0x02;
pub const OP_REFERENCE: u8 = 0x03;
pub const OP_CONTAINER: u8 = 0x04;
pub const OP_CHAR: u8 = 0x05;
pub const OP_BOOL: u8 = 0x06;
pub const OP_INT8: u8 = 0x07;
pub const OP_INT16: u8 = 0x08;
pub const OP_INT32: u8 = 0x09;
pub const OP_FLOAT32: u8 = 0x0a;
pub const OP_FLOAT64: u8 = 0x0b;
pub const OP_STRING: u8 = 0x0c;
pub const OP_RAW: u8 = 0x0d;
pub const OP_STRING_TOKEN: u8 = 0x0e;
pub const OP_STRING_ARRAY: u8 = 0x0f;
pub const OP_BITFIELD: u8 = 0x10;
pub const OP_END_ARRAY: u8 = 0x7c;
pub const OP_SKIP_MARK: u8 = 0x7d;
pub const OP_END_OBJECT: u8 = 0x7e;
pub const OP_END_OF_STREAM: u8 = 0x7f;

const FLAG_TOKENS: u32 = 1 << 2;
const FLAG_BY_INDEX: u32 = 1 << 3;

/// Payload of one instruction, kept in its encoded form so the stream can be re-serialised
/// byte-exactly.
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    None,
    /// Array / Container length.
    Count(u32),
    Byte(u8),
    I16(i16),
    I32(i32),
    U32(u32),
    F32(f32),
    F64(f64),
    /// Index into [`Prp::strings`].
    StrIdx(u32),
    /// Inline string bytes (flag bit 3 clear).
    StrLit(Vec<u8>),
    Raw(Vec<u8>),
    StrIdxList(Vec<u32>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Instr {
    /// Raw opcode byte (including the 0x80 "named" bit).
    pub opcode: u8,
    pub data: Data,
}

impl Instr {
    pub fn base(&self) -> u8 {
        if self.opcode == OP_END_OF_STREAM { self.opcode } else { self.opcode & 0x7f }
    }
}

#[derive(Clone, Debug)]
pub struct Prp {
    pub is_raw: u8,
    pub flags: u32,
    pub unk: u32,
    /// `key_count + 1` strings, Latin-1 decoded.
    pub strings: Vec<String>,
    pub object_count: u32,
    pub instrs: Vec<Instr>,
}

/// Nested view of a property stream (see [`Prp::values`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Char(u8),
    I8(i8),
    I16(i16),
    I32(i32),
    F32(f32),
    F64(f64),
    Str(String),
    /// Op 0x0e: a string-table token (enum value / resource key).
    Token(String),
    Raw(Vec<u8>),
    Bitfield(u32),
    Reference(u32),
    StrArray(Vec<String>),
    Array(Vec<Value>),
    /// `Container(n)` followed by its `n` items.
    Container(Vec<Value>),
    Object(Vec<Value>),
    SkipMark,
}

impl Value {
    pub fn as_i32(&self) -> Option<i32> {
        if let Value::I32(v) = self { Some(*v) } else { None }
    }
    pub fn as_f32(&self) -> Option<f32> {
        if let Value::F32(v) = self { Some(*v) } else { None }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        if let Value::Array(v) = self { Some(v) } else { None }
    }
}

/// One scene-wide definition from the leading container.
#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    pub name: String,
    /// Kind code (2 int array, 3 float array, 12/14/16 string, 17 string table).
    pub kind: i32,
    pub value: DefValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DefValue {
    Ints(Vec<i32>),
    Floats(Vec<f32>),
    Str(String),
    StrTab(Vec<String>),
}

/// A node of the scene tree: ROOT is node 0, then depth-first order.
#[derive(Clone, Debug)]
pub struct PrpNode {
    /// Range into [`Prp::instrs`] covering the property object *contents* (without Begin/End).
    pub props: std::ops::Range<usize>,
    pub controllers: Vec<PrpController>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct PrpController {
    pub name: String,
    pub props: std::ops::Range<usize>,
}

#[derive(Clone, Debug)]
pub struct PrpScene {
    pub definitions: Vec<Definition>,
    pub nodes: Vec<PrpNode>,
    /// Index of the first instruction after the last node (the stream trailer: `Bool(0)`, EndOfStream).
    pub trailer_start: usize,
}

impl Prp {
    pub fn parse(d: &[u8]) -> Result<Prp> {
        let mut c = Cur::new(d);
        if c.bytes(14, "PRP magic")? != MAGIC {
            return invalid("not an IOPacked v0.1 file");
        }
        let is_raw = c.u8("is_raw")?;
        let flags = c.u32("flags")?;
        let unk = c.u32("unk")?;
        let key_count = c.u32("key_count")? as usize;
        let table_bytes = c.u32("table_bytes")? as usize;
        let table_start = c.p;
        let mut strings = Vec::with_capacity(key_count + 1);
        for _ in 0..=key_count {
            strings.push(c.cstr("PRP string")?);
        }
        if c.p - table_start != table_bytes {
            return invalid(format!("PRP string table is {} bytes, header says {table_bytes}", c.p - table_start));
        }
        let object_count = c.u32("object_count")?;
        let mut instrs = Vec::new();
        loop {
            let op = c.u8("opcode")?;
            let base = if op == OP_END_OF_STREAM { op } else { op & 0x7f };
            let data = match base {
                OP_ARRAY | OP_CONTAINER => Data::Count(c.u32("count")?),
                OP_BEGIN_OBJECT | OP_END_ARRAY | OP_SKIP_MARK | OP_END_OBJECT | OP_END_OF_STREAM => Data::None,
                OP_REFERENCE | OP_BITFIELD => Data::U32(c.u32("u32")?),
                OP_CHAR | OP_BOOL | OP_INT8 => Data::Byte(c.u8("byte")?),
                OP_INT16 => Data::I16(c.u16("i16")? as i16),
                OP_INT32 => Data::I32(c.i32("i32")?),
                OP_FLOAT32 => Data::F32(c.f32("f32")?),
                OP_FLOAT64 => Data::F64(c.f64("f64")?),
                OP_STRING => exchange_string(&mut c, flags, strings.len())?,
                OP_RAW => {
                    let n = c.u32("raw length")? as usize;
                    Data::Raw(c.bytes(n, "raw data")?.to_vec())
                }
                OP_STRING_TOKEN => {
                    if flags & FLAG_TOKENS != 0 {
                        exchange_string(&mut c, flags, strings.len())?
                    } else {
                        Data::U32(c.u32("token")?)
                    }
                }
                OP_STRING_ARRAY => {
                    if flags & FLAG_TOKENS != 0 {
                        if flags & FLAG_BY_INDEX == 0 {
                            return Err(FormatError::Unsupported("PRP StringArray with inline strings".into()));
                        }
                        let n = c.u32("string array length")? as usize;
                        let mut v = Vec::with_capacity(n);
                        for _ in 0..n {
                            let i = c.u32("string index")?;
                            if i as usize >= strings.len() {
                                return invalid("PRP string index out of range");
                            }
                            v.push(i);
                        }
                        Data::StrIdxList(v)
                    } else {
                        Data::U32(c.u32("string array")?)
                    }
                }
                _ => return Err(FormatError::Unsupported(format!("PRP opcode {op:#04x} at {:#x}", c.p - 1))),
            };
            instrs.push(Instr { opcode: op, data });
            if base == OP_END_OF_STREAM {
                break;
            }
        }
        if !c.eof() {
            return invalid(format!("{} trailing bytes after EndOfStream", c.remaining()));
        }
        Ok(Prp { is_raw, flags, unk, strings, object_count, instrs })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.put_u8(self.is_raw);
        out.put_u32(self.flags);
        out.put_u32(self.unk);
        out.put_u32(self.strings.len().saturating_sub(1) as u32);
        let table: Vec<u8> = self.strings.iter().flat_map(|s| from_latin1(s).into_iter().chain([0u8])).collect();
        out.put_u32(table.len() as u32);
        out.extend_from_slice(&table);
        out.put_u32(self.object_count);
        for i in &self.instrs {
            out.push(i.opcode);
            match &i.data {
                Data::None => {}
                Data::Count(v) | Data::U32(v) | Data::StrIdx(v) => out.put_u32(*v),
                Data::Byte(v) => out.put_u8(*v),
                Data::I16(v) => out.extend_from_slice(&v.to_le_bytes()),
                Data::I32(v) => out.extend_from_slice(&v.to_le_bytes()),
                Data::F32(v) => out.put_f32(*v),
                Data::F64(v) => out.extend_from_slice(&v.to_le_bytes()),
                Data::StrLit(b) => {
                    out.put_u32(b.len() as u32);
                    out.extend_from_slice(b);
                }
                Data::Raw(b) => {
                    out.put_u32(b.len() as u32);
                    out.extend_from_slice(b);
                }
                Data::StrIdxList(l) => {
                    out.put_u32(l.len() as u32);
                    for v in l {
                        out.put_u32(*v);
                    }
                }
            }
        }
        out
    }

    fn string_of(&self, d: &Data) -> String {
        match d {
            Data::StrIdx(i) => self.strings.get(*i as usize).cloned().unwrap_or_default(),
            Data::StrLit(b) => latin1(b),
            Data::U32(v) => format!("#{v}"),
            _ => String::new(),
        }
    }

    /// Converts a range of instructions into nested [`Value`]s.
    pub fn values(&self, range: std::ops::Range<usize>) -> Vec<Value> {
        let mut pos = range.start;
        let mut out = Vec::new();
        while pos < range.end {
            out.push(self.value_at(&mut pos, range.end));
        }
        out
    }

    fn value_at(&self, pos: &mut usize, end: usize) -> Value {
        let i = &self.instrs[*pos];
        *pos += 1;
        match i.base() {
            OP_ARRAY => {
                let mut items = Vec::new();
                while *pos < end && self.instrs[*pos].base() != OP_END_ARRAY {
                    items.push(self.value_at(pos, end));
                }
                *pos += 1; // EndArray
                Value::Array(items)
            }
            OP_BEGIN_OBJECT => {
                let mut items = Vec::new();
                while *pos < end && self.instrs[*pos].base() != OP_END_OBJECT {
                    items.push(self.value_at(pos, end));
                }
                *pos += 1;
                Value::Object(items)
            }
            OP_CONTAINER => {
                let n = if let Data::Count(n) = i.data { n } else { 0 };
                let mut items = Vec::new();
                for _ in 0..n {
                    if *pos >= end {
                        break;
                    }
                    items.push(self.value_at(pos, end));
                }
                Value::Container(items)
            }
            OP_REFERENCE => Value::Reference(if let Data::U32(v) = i.data { v } else { 0 }),
            OP_CHAR => Value::Char(if let Data::Byte(v) = i.data { v } else { 0 }),
            OP_BOOL => Value::Bool(matches!(i.data, Data::Byte(v) if v != 0)),
            OP_INT8 => Value::I8(if let Data::Byte(v) = i.data { v as i8 } else { 0 }),
            OP_INT16 => Value::I16(if let Data::I16(v) = i.data { v } else { 0 }),
            OP_INT32 => Value::I32(if let Data::I32(v) = i.data { v } else { 0 }),
            OP_FLOAT32 => Value::F32(if let Data::F32(v) = i.data { v } else { 0.0 }),
            OP_FLOAT64 => Value::F64(if let Data::F64(v) = i.data { v } else { 0.0 }),
            OP_STRING => Value::Str(self.string_of(&i.data)),
            OP_RAW => Value::Raw(if let Data::Raw(b) = &i.data { b.clone() } else { Vec::new() }),
            OP_STRING_TOKEN => Value::Token(self.string_of(&i.data)),
            OP_STRING_ARRAY => match &i.data {
                Data::StrIdxList(l) => {
                    Value::StrArray(l.iter().map(|&k| self.strings.get(k as usize).cloned().unwrap_or_default()).collect())
                }
                _ => Value::StrArray(Vec::new()),
            },
            OP_BITFIELD => Value::Bitfield(if let Data::U32(v) = i.data { v } else { 0 }),
            _ => Value::SkipMark,
        }
    }

    /// Parses the definitions container and the scene node tree.
    pub fn scene(&self) -> Result<PrpScene> {
        let mut pos = 0usize;
        let ins = &self.instrs;
        let defs = self.parse_definitions(&mut pos)?;
        let mut nodes: Vec<PrpNode> = Vec::new();
        self.parse_node(&mut pos, None, &mut nodes)?;
        let _ = ins;
        Ok(PrpScene { definitions: defs, nodes, trailer_start: pos })
    }

    fn expect(&self, pos: usize, op: u8) -> Result<&Instr> {
        match self.instrs.get(pos) {
            Some(i) if i.base() == op => Ok(i),
            Some(i) => invalid(format!("expected op {op:#x} at instr {pos}, found {:#x}", i.opcode)),
            None => Err(FormatError::Eof("PRP instruction")),
        }
    }

    fn parse_definitions(&self, pos: &mut usize) -> Result<Vec<Definition>> {
        let n = match self.expect(*pos, OP_CONTAINER)?.data {
            Data::Count(n) => n,
            _ => 0,
        };
        *pos += 1;
        let mut defs = Vec::new();
        for _ in 0..n {
            let name = self.string_of(&self.expect(*pos, OP_STRING)?.data);
            *pos += 1;
            let kind = match self.expect(*pos, OP_INT32)?.data {
                Data::I32(k) => k,
                _ => 0,
            };
            *pos += 1;
            let value = match kind {
                2 | 3 => {
                    let cap = match self.expect(*pos, OP_INT32)?.data {
                        Data::I32(c) => c as u32,
                        _ => 0,
                    };
                    *pos += 1;
                    match self.expect(*pos, OP_ARRAY)?.data {
                        Data::Count(c) if c == cap => {}
                        _ => return invalid("definition array length mismatch"),
                    }
                    *pos += 1;
                    let mut ints = Vec::new();
                    let mut floats = Vec::new();
                    for _ in 0..cap {
                        match &self.instrs[*pos].data {
                            Data::I32(v) if kind == 2 => ints.push(*v),
                            Data::F32(v) if kind == 3 => floats.push(*v),
                            _ => return invalid("definition array element type"),
                        }
                        *pos += 1;
                    }
                    self.expect(*pos, OP_END_ARRAY)?;
                    *pos += 1;
                    if kind == 2 { DefValue::Ints(ints) } else { DefValue::Floats(floats) }
                }
                12 | 14 | 16 => {
                    let s = self.string_of(&self.expect(*pos, OP_STRING)?.data);
                    *pos += 1;
                    DefValue::Str(s)
                }
                17 => {
                    let cnt = match self.expect(*pos, OP_CONTAINER)?.data {
                        Data::Count(c) => c,
                        _ => 0,
                    };
                    *pos += 1;
                    let mut v = Vec::new();
                    for _ in 0..cnt {
                        v.push(self.string_of(&self.expect(*pos, OP_STRING)?.data));
                        *pos += 1;
                    }
                    DefValue::StrTab(v)
                }
                k => return Err(FormatError::Unsupported(format!("definition kind {k}"))),
            };
            defs.push(Definition { name, kind, value });
        }
        Ok(defs)
    }

    /// Returns the range of the object's contents and advances `pos` past EndObject.
    fn skip_object(&self, pos: &mut usize) -> Result<std::ops::Range<usize>> {
        self.expect(*pos, OP_BEGIN_OBJECT)?;
        *pos += 1;
        let start = *pos;
        let mut depth = 1;
        while depth > 0 {
            let i = self.instrs.get(*pos).ok_or(FormatError::Eof("PRP object"))?;
            match i.base() {
                OP_BEGIN_OBJECT => depth += 1,
                OP_END_OBJECT => depth -= 1,
                OP_END_OF_STREAM => return invalid("EndOfStream inside object"),
                _ => {}
            }
            *pos += 1;
        }
        Ok(start..*pos - 1)
    }

    fn parse_node(&self, pos: &mut usize, parent: Option<usize>, nodes: &mut Vec<PrpNode>) -> Result<usize> {
        let props = self.skip_object(pos)?;
        let idx = nodes.len();
        nodes.push(PrpNode { props, controllers: Vec::new(), parent, children: Vec::new() });
        let nctrl = match self.expect(*pos, OP_CONTAINER)?.data {
            Data::Count(n) => n,
            _ => 0,
        };
        *pos += 1;
        for _ in 0..nctrl {
            let name = self.string_of(&self.expect(*pos, OP_STRING)?.data);
            *pos += 1;
            let props = self.skip_object(pos)?;
            nodes[idx].controllers.push(PrpController { name, props });
        }
        let nchild = match self.expect(*pos, OP_CONTAINER)?.data {
            Data::Count(n) => n,
            _ => 0,
        };
        *pos += 1;
        for _ in 0..nchild {
            let c = self.parse_node(pos, Some(idx), nodes)?;
            nodes[idx].children.push(c);
        }
        Ok(idx)
    }
}

fn exchange_string(c: &mut Cur, flags: u32, nstrings: usize) -> Result<Data> {
    if flags & FLAG_BY_INDEX != 0 {
        let i = c.u32("string index")?;
        if i as usize >= nstrings {
            return invalid(format!("PRP string index {i} out of range ({nstrings})"));
        }
        Ok(Data::StrIdx(i))
    } else {
        let n = c.u32("string length")? as usize;
        Ok(Data::StrLit(c.bytes(n, "string bytes")?.to_vec()))
    }
}
