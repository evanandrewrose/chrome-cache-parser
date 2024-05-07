use std::fmt::{self, Debug, Formatter};
use zerocopy::{FromBytes, FromZeroes};

extern crate static_assertions as sa;

const FILE_TYPE_MASK: u32 = 0x70000000;
const FILE_TYPE_OFFSET: u32 = 28;
const FILE_NAME_MASK: u32 = 0x0FFFFFFF;
const FILE_SELECTOR_MASK: u32 = 0x00ff0000;
const FILE_SELECTOR_OFFSET: u32 = 16;
const START_BLOCK_MASK: u32 = 0x0000FFFF;
const NUM_BLOCKS_MASK: u32 = 0x03000000;
const NUM_BLOCKS_OFFSET: u32 = 24;

#[derive(Debug)]
pub enum FileType {
    External = 0,
    Rankings = 1,
    Block256 = 2,
    Block1k = 3,
    Block4k = 4,
    BlockFiles = 5,
    BlockEntries = 6,
    BlockEvicted = 7,
}

// See: https://chromium.googlesource.com/chromium/src/net/+/ddbc6c5954c4bee29902082eb9052405e83abc02/disk_cache/disk_format_base.h#28
#[derive(Copy, Clone, FromZeroes, FromBytes)]
#[repr(C)]
pub struct CacheAddr {
    pub value: u32,
}

pub const CACHE_ADDRESS_SIZE: usize = std::mem::size_of::<CacheAddr>();

sa::const_assert_eq!(CACHE_ADDRESS_SIZE, 4);

// See: https://chromium.googlesource.com/chromium/src/net/+/ddbc6c5954c4bee29902082eb9052405e83abc02/disk_cache/addr.h
impl CacheAddr {
    pub fn from(value: u32) -> CacheAddr {
        CacheAddr { value }
    }

    pub fn is_initialized(&self) -> bool {
        self.value != 0
    }

    pub fn file_type(&self) -> FileType {
        match (self.value & FILE_TYPE_MASK) >> FILE_TYPE_OFFSET {
            0 => FileType::External,
            1 => FileType::Rankings,
            2 => FileType::Block256,
            3 => FileType::Block1k,
            4 => FileType::Block4k,
            5 => FileType::BlockFiles,
            6 => FileType::BlockEntries,
            7 => FileType::BlockEvicted,
            _ => panic!("u32 shifted 28 bits isn't in (0..8)?"),
        }
    }

    pub fn file_number(&self) -> u32 {
        match self.file_type() {
            FileType::External => self.value & FILE_NAME_MASK,
            _ => (self.value & FILE_SELECTOR_MASK) >> FILE_SELECTOR_OFFSET,
        }
    }

    pub fn start_block(&self) -> u32 {
        self.value & START_BLOCK_MASK
    }

    pub fn num_blocks(&self) -> u32 {
        ((self.value & NUM_BLOCKS_MASK) >> NUM_BLOCKS_OFFSET) + 1
    }
}

impl Debug for CacheAddr {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "0x{:X?} (initialized: {:?}, file_type: {:?} file_number: {:?}, start_block: {:?}, num_blocks: {:?})",
               self.value, self.is_initialized(), self.file_type(), self.file_number(), self.start_block(), self.num_blocks())
    }
}
