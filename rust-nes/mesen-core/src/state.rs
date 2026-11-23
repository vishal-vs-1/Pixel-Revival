//! Minimal, stable binary save-state reader/writer.
//!
//! Components serialize with an explicit field order. We avoid `serde` so the
//! core stays dependency-free and save states remain easy to inspect/debug.

use std::fmt;

#[derive(Clone, Debug)]
pub struct StateError(pub String);

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "save-state error: {}", self.0)
    }
}

impl std::error::Error for StateError {}

impl From<&str> for StateError {
    fn from(value: &str) -> Self {
        StateError(value.to_string())
    }
}

impl From<String> for StateError {
    fn from(value: String) -> Self {
        StateError(value)
    }
}

pub type StateResult<T> = Result<T, StateError>;

#[derive(Default, Debug)]
pub struct StateWriter {
    data: Vec<u8>,
}

impl StateWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.data
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn u8(&mut self, value: u8) {
        self.data.push(value);
    }

    pub fn bool(&mut self, value: bool) {
        self.u8(value as u8);
    }

    pub fn u16(&mut self, value: u16) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub fn i16(&mut self, value: i16) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub fn i32(&mut self, value: i32) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub fn f32(&mut self, value: f32) {
        self.data.extend_from_slice(&value.to_bits().to_le_bytes());
    }

    pub fn f64(&mut self, value: f64) {
        self.data.extend_from_slice(&value.to_bits().to_le_bytes());
    }

    pub fn bytes(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.data.extend_from_slice(value);
    }

    pub fn fixed_bytes(&mut self, value: &[u8]) {
        self.data.extend_from_slice(value);
    }
}

#[derive(Clone, Debug)]
pub struct StateReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> StateReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> StateResult<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(StateError(format!(
                "unexpected end of save state (needed {} bytes at {}, have {})",
                n,
                self.pos,
                self.remaining()
            )));
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn u8(&mut self) -> StateResult<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> StateResult<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn u16(&mut self) -> StateResult<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> StateResult<i16> {
        let b = self.take(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> StateResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> StateResult<i32> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> StateResult<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn f32(&mut self) -> StateResult<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub fn f64(&mut self) -> StateResult<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    pub fn bytes(&mut self) -> StateResult<Vec<u8>> {
        let len = self.u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    pub fn fixed_bytes(&mut self, len: usize) -> StateResult<Vec<u8>> {
        Ok(self.take(len)?.to_vec())
    }

    pub fn skip(&mut self, len: usize) -> StateResult<()> {
        self.take(len)?;
        Ok(())
    }
}
