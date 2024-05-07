use std::mem;
use zerocopy::{FromBytes, FromZeroes};

use crate::{time::WindowsEpochMicroseconds, CacheAddr};
use static_assertions as sa;

pub const INDEX_MAGIC: u32 = 0xc103cac3;

pub const INDEX_HEADER_SIZE: usize = mem::size_of::<IndexHeader>();
sa::const_assert_eq!(INDEX_HEADER_SIZE, 368);

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum CacheVersion {
    Version2_0,
    Version2_1,
    Version3_0,
    Unknown(u32),
}

impl From<CacheVersionId> for CacheVersion {
    fn from(version: CacheVersionId) -> Self {
        match version.0 {
            0x20000 => CacheVersion::Version2_0,
            0x20001 => CacheVersion::Version2_1,
            0x30000 => CacheVersion::Version3_0,
            _ => CacheVersion::Unknown(version.0),
        }
    }
}

#[derive(Debug, FromZeroes, FromBytes, Copy, Clone)]
pub struct CacheVersionId(u32);

// See: https://chromium.googlesource.com/chromium/src/net/+/refs/heads/main/disk_cache/blockfile/disk_format.h#77
#[derive(Debug, FromZeroes, FromBytes)]
#[repr(C)]
pub struct IndexHeader {
    pub magic: u32,
    pub version: CacheVersionId,
    pub num_entries: i32,
    pub num_bytes: i32,
    pub last_file: i32,
    pub this_id: i32,
    pub stats: CacheAddr,
    pub table_len: i32,
    pub crash: i32,
    pub experiment: i32,
    pub create_time: WindowsEpochMicroseconds,
    pad: [u32; 52],
    pub lru: LruData,
}

// See: https://chromium.googlesource.com/chromium/src/net/+/refs/heads/main/disk_cache/blockfile/disk_format.h#64
#[derive(Debug, FromZeroes, FromBytes)]
#[repr(C)]
pub struct LruData {
    pad1: [u32; 2],
    pub filled: i32,
    pub sizes: [i32; 5],
    pub heads: [CacheAddr; 5],
    pub tails: [CacheAddr; 5],
    pub transaction: CacheAddr,
    pub operation: i32,
    pub operation_list: i32,
    pad2: [u32; 7],
}
