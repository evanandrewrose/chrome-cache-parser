use std::{cell::RefCell, collections::HashMap, fmt, fs, io::Read, mem, path::PathBuf, rc::Rc};

use zerocopy::{FromBytes, FromZeroes};

use crate::{
    cache_address::CacheAddr,
    error::{self, CCPResult},
    time::WindowsEpochMicroseconds,
};
use static_assertions as sa;

const BLOCK_MAGIC: u32 = 0xc104cac3;
const BLOCK_HEADER_SIZE: u32 = 8192;
const MAX_BLOCKS: u32 = (BLOCK_HEADER_SIZE - 80) * 8;
const INLINE_KEY_SIZE: usize = 160;

#[derive(Debug, FromZeroes, FromBytes)]
#[repr(C)]
struct AllocBitmap {
    data: [u32; MAX_BLOCKS as usize / 32],
}

// See: https://chromium.googlesource.com/chromium/src/net/+/ddbc6c5954c4bee29902082eb9052405e83abc02/disk_cache/disk_format_base.h
#[derive(Debug, FromZeroes, FromBytes)]
#[repr(C)]
struct BlockFileHeader {
    pub magic: u32,
    pub version: u32,
    pub this_file: i16,
    pub next_file: i16,
    pub entry_size: i32,
    pub num_entries: i32,
    pub max_entries: i32,
    pub empty: [i32; 4],
    pub hints: [i32; 4],
    pub updating: i32,
    pub user: [i32; 5],
    pub allocation_map: AllocBitmap,
}

#[derive(FromZeroes, FromBytes, Clone)]
pub struct InlineCacheKey {
    key: [u8; INLINE_KEY_SIZE],
}

impl fmt::Debug for InlineCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", std::str::from_utf8(&self.key).unwrap())
    }
}

impl fmt::Display for InlineCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = std::str::from_utf8(&self.key)
            .map_err(|_| fmt::Error)?
            .trim_end_matches(char::from(0));
        write!(f, "{}", key)?;
        Ok(())
    }
}

// See: https://chromium.googlesource.com/chromium/src/net/+/ddbc6c5954c4bee29902082eb9052405e83abc02/disk_cache/disk_format.h#101
#[derive(Debug, FromZeroes, FromBytes, Clone)]
#[repr(C)]
pub struct BlockFileCacheEntry {
    pub hash: u32,
    pub next: CacheAddr,
    pub rankings_node: CacheAddr,
    pub reuse_count: i32,
    pub refetch_count: i32,
    pub state: i32,
    pub creation_time: WindowsEpochMicroseconds,
    pub key_len: i32,
    pub long_key: CacheAddr,
    pub data_size: [i32; 4],
    pub data_addr: [CacheAddr; 4],
    pub flags: u32,
    pad: [u32; 4],
    pub self_hash: u32,
    pub key: InlineCacheKey,
}

const BLOCK_FILE_ENTRY_SIZE: usize = mem::size_of::<BlockFileCacheEntry>();
sa::const_assert_eq!(BLOCK_FILE_ENTRY_SIZE, 256);

/// An iterator over the logical entries in a map of block files. Data files are lazily loaded and
/// cached. An entry in the chrome cache is a node in a linked list of entries in the block files.
/// The index file is a hash table that maps keys to the first entry in the linked list.
///
/// The next node in a given linked list is not guaranteed to be in the same block file, so each
/// entry needs needs a reference to all of the data files.
///
/// By storing the reference to the data files, we can lazily evaluate the actual entries without
/// copying the underlying buffer. The iterator yields a parser with a shared reference to the
/// underlying data required for transmutation.
///
/// `LazyBlockFileCacheEntryIterator`` is to be instantiated with the cache address of the first
/// entry and yields any subsequent entries in the linked list.
pub struct LazyBlockFileCacheEntryIterator {
    current: Option<CacheAddr>,
    cache_path: PathBuf,
    data_files: Rc<RefCell<HashMap<u32, LazyBlockFile>>>,
}

impl LazyBlockFileCacheEntryIterator {
    pub fn new(
        cache_path: PathBuf,
        data_files: Rc<RefCell<HashMap<u32, LazyBlockFile>>>,
        start: CacheAddr,
    ) -> LazyBlockFileCacheEntryIterator {
        LazyBlockFileCacheEntryIterator {
            current: Some(start),
            cache_path,
            data_files,
        }
    }
}

impl Iterator for LazyBlockFileCacheEntryIterator {
    type Item = LazyBlockFileCacheEntry;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.take()?;

        let mut data_files = (*self.data_files).borrow_mut();

        let data_file = data_files.entry(current.file_number()).or_insert_with(|| {
            let file_path = self
                .cache_path
                .join(format!("data_{}", current.file_number()));

            let mut file = fs::File::open(file_path).unwrap();
            let mut buf: Vec<u8> = Vec::new();
            file.read_to_end(&mut buf).unwrap();
            LazyBlockFile::new(Rc::new(buf))
        });

        let current = data_file.entry(&current).ok()?;

        if let Ok(current) = current.get() {
            let next = current.next;
            if next.is_initialized() {
                self.current = Some(next);
            }
        }

        Some(current)
    }
}

pub struct LazyBlockFileCacheEntry {
    buffer: Rc<Vec<u8>>,
    address: usize,
    size: usize,
}

impl LazyBlockFileCacheEntry {
    pub fn new(buffer: Rc<Vec<u8>>, address: usize, size: usize) -> LazyBlockFileCacheEntry {
        LazyBlockFileCacheEntry {
            buffer,
            address,
            size,
        }
    }

    pub fn from_block_file(
        block_file: &LazyBlockFile,
        addr: &CacheAddr,
    ) -> CCPResult<LazyBlockFileCacheEntry> {
        block_file.entry(addr)
    }

    /// Parse the entry from the buffer and return a reference to it.
    pub fn get(&self) -> CCPResult<&BlockFileCacheEntry> {
        let entry_location = BLOCK_HEADER_SIZE as usize + self.address * self.size;
        BlockFileCacheEntry::ref_from(
            &self.buffer[entry_location..entry_location + BLOCK_FILE_ENTRY_SIZE],
        )
        .ok_or(error::CCPError::DataMisalignment(format!(
            "entry at {}",
            entry_location
        )))
    }
}

pub struct LazyBlockFile {
    buffer: Rc<Vec<u8>>,
}

/// Represents a block file in the chrome cache. It has a header, providing some metadata about the
/// file, followed by a series of contiguous blocks of a fixed size, defined by a field within the
/// header.
impl LazyBlockFile {
    pub fn new(buffer: Rc<Vec<u8>>) -> LazyBlockFile {
        LazyBlockFile { buffer }
    }

    fn header(&self) -> CCPResult<&BlockFileHeader> {
        let header = BlockFileHeader::ref_from(&self.buffer[0..mem::size_of::<BlockFileHeader>()])
            .ok_or(error::CCPError::DataMisalignment(
                "block file header".to_string(),
            ))?;
        if header.magic != BLOCK_MAGIC {
            return Err(error::CCPError::InvalidData(format!(
                "expected block magic {:x}, got {:x}",
                BLOCK_MAGIC, header.magic
            )));
        }
        Ok(header)
    }

    /// Returns a lazily evaluated cache entry at the given address.
    pub fn entry(&self, addr: &CacheAddr) -> CCPResult<LazyBlockFileCacheEntry> {
        let header = self.header()?;
        Ok(LazyBlockFileCacheEntry::new(
            Rc::clone(&self.buffer),
            addr.start_block() as usize,
            header.entry_size as usize,
        ))
    }
}
