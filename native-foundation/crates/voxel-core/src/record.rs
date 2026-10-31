//! Versioned cube data with an accidental-corruption checksum (not authentication).

use crate::coords::{CubePos, VOLUME};
use std::fmt;

pub const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
pub const MIN_ENCODED: usize = 44 + VOLUME * 4 + 8;
pub const MAX_ENCODED: usize = MIN_ENCODED + MAX_PAYLOAD;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordError {
    Invalid(&'static str),
    UnsupportedVersion(u16),
    ChecksumMismatch,
    TooLarge,
    RevisionExhausted,
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for RecordError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CubeRecord {
    pos: CubePos,
    revision: u64,
    blocks: Box<[u32; VOLUME]>,
    payload: Vec<u8>,
}

impl CubeRecord {
    pub fn new(pos: CubePos) -> Result<Self, RecordError> {
        if pos.block_at(0).is_none() {
            return Err(RecordError::Invalid("cube origin"));
        }
        Ok(Self {
            pos,
            revision: 0,
            blocks: Box::new([0; VOLUME]),
            payload: Vec::new(),
        })
    }
    pub fn pos(&self) -> CubePos {
        self.pos
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn blocks(&self) -> &[u32; VOLUME] {
        &self.blocks
    }
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    pub fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + VOLUME * 4 + self.payload.capacity()
    }
    pub fn set(&mut self, index: usize, value: u32) -> Result<bool, RecordError> {
        if index >= VOLUME {
            return Err(RecordError::Invalid("local index"));
        }
        if self.blocks[index] == value {
            return Ok(false);
        }
        let next = self
            .revision
            .checked_add(1)
            .ok_or(RecordError::RevisionExhausted)?;
        self.blocks[index] = value;
        self.revision = next;
        Ok(true)
    }
    pub fn replace_payload(&mut self, mut bytes: Vec<u8>) -> Result<bool, RecordError> {
        if bytes.len() > MAX_PAYLOAD {
            return Err(RecordError::TooLarge);
        }
        if bytes == self.payload {
            return Ok(false);
        }
        let next = self
            .revision
            .checked_add(1)
            .ok_or(RecordError::RevisionExhausted)?;
        bytes.shrink_to_fit();
        self.payload = bytes;
        self.revision = next;
        Ok(true)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(MIN_ENCODED + self.payload.len());
        bytes.extend_from_slice(b"NCUB");
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        for coordinate in [self.pos.x, self.pos.y, self.pos.z] {
            bytes.extend_from_slice(&coordinate.to_le_bytes());
        }
        bytes.extend_from_slice(&self.revision.to_le_bytes());
        bytes.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        for value in self.blocks.iter() {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&self.payload);
        let checksum = checksum(&bytes);
        bytes.extend_from_slice(&checksum.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, RecordError> {
        if bytes.len() > MAX_ENCODED {
            return Err(RecordError::TooLarge);
        }
        if bytes.len() < MIN_ENCODED {
            return Err(RecordError::Invalid("truncated record"));
        }
        if &bytes[..4] != b"NCUB" {
            return Err(RecordError::Invalid("magic"));
        }
        let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if version != 1 {
            return Err(RecordError::UnsupportedVersion(version));
        }
        if bytes[6..8] != [0, 0] {
            return Err(RecordError::Invalid("flags"));
        }
        let body_end = bytes.len() - 8;
        let stored = u64::from_le_bytes(bytes[body_end..].try_into().unwrap());
        if checksum(&bytes[..body_end]) != stored {
            return Err(RecordError::ChecksumMismatch);
        }
        let coordinate = |offset| i64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let pos = CubePos::new(coordinate(8), coordinate(16), coordinate(24));
        let mut record = Self::new(pos)?;
        let length = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        if length > MAX_PAYLOAD {
            return Err(RecordError::TooLarge);
        }
        if MIN_ENCODED + length != bytes.len() {
            return Err(RecordError::Invalid("payload length"));
        }
        record.revision = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
        for (value, encoded) in record
            .blocks
            .iter_mut()
            .zip(bytes[44..44 + VOLUME * 4].chunks_exact(4))
        {
            *value = u32::from_le_bytes(encoded.try_into().unwrap());
        }
        record.payload = bytes[44 + VOLUME * 4..body_end].to_vec();
        Ok(record)
    }
}

pub(crate) fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> CubeRecord {
        let mut cube = CubeRecord::new(CubePos::new(-2, 1_000_000, 3)).unwrap();
        cube.set(0, 42).unwrap();
        cube.set(4095, u32::MAX).unwrap();
        cube.replace_payload(b"machine-state:items=12;energy=9000".to_vec())
            .unwrap();
        cube
    }

    #[test]
    fn codec_preserves_every_block_position_revision_and_payload() {
        let cube = fixture();
        let bytes = cube.encode();
        assert_eq!(bytes.len(), MIN_ENCODED + cube.payload().len());
        assert_eq!(CubeRecord::decode(&bytes), Ok(cube));
    }

    #[test]
    fn truncated_corrupt_and_trailing_records_are_rejected() {
        let bytes = fixture().encode();
        for length in [0, 4, 43, MIN_ENCODED - 1, bytes.len().saturating_sub(1)] {
            assert!(CubeRecord::decode(&bytes[..length.min(bytes.len())]).is_err());
        }
        let mut corrupt = bytes.clone();
        assert!(corrupt.len() > 100);
        corrupt[100] ^= 1;
        assert_eq!(
            CubeRecord::decode(&corrupt),
            Err(RecordError::ChecksumMismatch)
        );
        let mut trailing = bytes;
        trailing.push(0);
        assert!(CubeRecord::decode(&trailing).is_err());
    }

    #[test]
    fn updates_are_bounded_and_revision_only_changes_on_edits() {
        let mut cube = fixture();
        let before = cube.clone();
        assert!(!cube.set(0, 42).unwrap());
        assert_eq!(cube, before);
        assert!(cube.set(VOLUME, 1).is_err());
        assert!(cube.replace_payload(vec![0; MAX_PAYLOAD + 1]).is_err());
        assert_eq!(cube, before);
        cube.revision = u64::MAX;
        assert_eq!(cube.set(0, 1), Err(RecordError::RevisionExhausted));
        assert_eq!(cube.blocks()[0], 42);
    }

    #[test]
    fn future_versions_and_mismatched_lengths_fail_explicitly() {
        let mut bytes = fixture().encode();
        bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            CubeRecord::decode(&bytes),
            Err(RecordError::UnsupportedVersion(2))
        );
        bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
        bytes[40..44].copy_from_slice(&0u32.to_le_bytes());
        let body = bytes.len() - 8;
        let sum = checksum(&bytes[..body]);
        bytes[body..].copy_from_slice(&sum.to_le_bytes());
        assert_eq!(
            CubeRecord::decode(&bytes),
            Err(RecordError::Invalid("payload length"))
        );
    }
}
