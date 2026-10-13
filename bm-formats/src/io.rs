//! Tiny little-endian cursor/writer helpers and the shared error type.

use std::fmt;

#[derive(Debug)]
pub enum FormatError {
    /// Ran out of bytes while reading `what`.
    Eof(&'static str),
    /// Structure is inconsistent / does not match the verified layout.
    Invalid(String),
    /// Valid-looking but not implemented (unknown opcode, unseen variant ...).
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, FormatError>;

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::Eof(w) => write!(f, "unexpected end of data while reading {w}"),
            FormatError::Invalid(m) => write!(f, "invalid data: {m}"),
            FormatError::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for FormatError {}

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(FormatError::Invalid(msg.into()))
}

/// Read cursor over a byte slice (all integers little-endian).
#[derive(Clone)]
pub struct Cur<'a> {
    pub d: &'a [u8],
    pub p: usize,
}

impl<'a> Cur<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Cur { d, p: 0 }
    }
    pub fn at(d: &'a [u8], p: usize) -> Self {
        Cur { d, p }
    }
    pub fn remaining(&self) -> usize {
        self.d.len().saturating_sub(self.p)
    }
    pub fn eof(&self) -> bool {
        self.p >= self.d.len()
    }
    pub fn bytes(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
        let end = self.p.checked_add(n).ok_or(FormatError::Eof(what))?;
        let s = self.d.get(self.p..end).ok_or(FormatError::Eof(what))?;
        self.p = end;
        Ok(s)
    }
    pub fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.bytes(1, what)?[0])
    }
    pub fn u16(&mut self, what: &'static str) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2, what)?.try_into().unwrap()))
    }
    pub fn u32(&mut self, what: &'static str) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4, what)?.try_into().unwrap()))
    }
    pub fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(self.u32(what)? as i32)
    }
    pub fn f32(&mut self, what: &'static str) -> Result<f32> {
        Ok(f32::from_bits(self.u32(what)?))
    }
    pub fn f64(&mut self, what: &'static str) -> Result<f64> {
        Ok(f64::from_le_bytes(self.bytes(8, what)?.try_into().unwrap()))
    }
    /// NUL terminated string (Latin-1 decoded, terminator consumed).
    pub fn cstr(&mut self, what: &'static str) -> Result<String> {
        let rest = self.d.get(self.p..).ok_or(FormatError::Eof(what))?;
        let n = rest.iter().position(|&b| b == 0).ok_or(FormatError::Eof(what))?;
        let s = latin1(&rest[..n]);
        self.p += n + 1;
        Ok(s)
    }
}

pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

pub fn from_latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

pub fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// Append helpers for little-endian output.
pub trait PutLe {
    fn put_u8(&mut self, v: u8);
    fn put_u16(&mut self, v: u16);
    fn put_u32(&mut self, v: u32);
    fn put_f32(&mut self, v: f32);
    fn put_cstr_latin1(&mut self, s: &str);
}

impl PutLe for Vec<u8> {
    fn put_u8(&mut self, v: u8) {
        self.push(v);
    }
    fn put_u16(&mut self, v: u16) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_u32(&mut self, v: u32) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_f32(&mut self, v: f32) {
        self.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    fn put_cstr_latin1(&mut self, s: &str) {
        self.extend_from_slice(&from_latin1(s));
        self.push(0);
    }
}
