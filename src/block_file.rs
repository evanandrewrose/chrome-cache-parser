use std::{cell::RefCell, collections::HashMap, fmt, fs, io::Read, mem, path::PathBuf, rc::Rc};

use zerocopy::{FromBytes, FromZeroes};

use crate::{
    cache_address::CacheAddr,
    error::{self, CCPResult},
    time::WindowsEpochMicroseconds,
};
use static_assertions as sa;

const BLOCK_MAGIC: u32 = 0xc104cac3;
const BLOCK_HEADER_SIZE: usize = 8192;
const MAX_BLOCKS: usize = (BLOCK_HEADER_SIZE - 80) * 8;
const INLINE_KEY_SIZE: usize = 160;

#[derive(Debug, FromZeroes, FromBytes)]
#[repr(C)]
struct AllocBitmap {
    data: [u32; MAX_BLOCKS / 32],
}

#[derive(Debug, FromZeroes, FromBytes, Clone)]
#[repr(C, packed(4))]
pub struct RankingsNode {
    pub last_used: WindowsEpochMicroseconds,
    pub last_modified: WindowsEpochMicroseconds,
    pub next: CacheAddr,
    pub prev: CacheAddr,
    pub contents: CacheAddr,
    pub dirty: i32,
    pub self_hash: u32,
}

sa::const_assert_eq!(mem::size_of::<RankingsNode>(), 36);

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

sa::const_assert_eq!(mem::size_of::<BlockFileCacheEntry>(), 256);

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
    data_files: Rc<RefCell<DataFiles>>,
}

impl LazyBlockFileCacheEntryIterator {
    pub fn new(
        data_files: Rc<RefCell<DataFiles>>,
        start: CacheAddr,
    ) -> LazyBlockFileCacheEntryIterator {
        LazyBlockFileCacheEntryIterator {
            current: Some(start),
            data_files,
        }
    }
}

/// A map of data files, lazily loaded and cached. Provides a method to get a cache entry from a
/// cache address, selecting the approapriate data file by the file number in the cache address.
pub struct DataFiles {
    data_files: HashMap<u32, LazyBlockFile>,
    path: PathBuf,
}

impl DataFiles {
    pub fn new(data_files: HashMap<u32, LazyBlockFile>, path: PathBuf) -> DataFiles {
        DataFiles { data_files, path }
    }

    fn get(&mut self, file_number: u32) -> &LazyBlockFile {
        self.data_files.entry(file_number).or_insert_with(|| {
            let file_path = self.path.join(format!("data_{}", file_number));

            let mut file = fs::File::open(file_path).unwrap();
            let mut buf: Vec<u8> = Vec::new();
            file.read_to_end(&mut buf).unwrap();
            LazyBlockFile::new(Rc::new(buf))
        })
    }

    pub fn get_entry(&mut self, addr: &CacheAddr) -> CCPResult<BufferSlice> {
        let data_file = self.get(addr.file_number());
        data_file.entry(addr)
    }
}

impl Iterator for LazyBlockFileCacheEntryIterator {
    type Item = LazyBlockFileCacheEntry;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.take()?;

        let mut data_files = (*self.data_files).borrow_mut();

        let current = data_files.get_entry(&current).ok()?;
        let current = LazyBlockFileCacheEntry::new(current, Rc::clone(&self.data_files));

        if let Ok(current) = current.get() {
            let next = current.next;
            if next.is_initialized() {
                self.current = Some(next);
            }
        }

        Some(current)
    }
}

pub struct LazyRankingsNode {
    buffer: BufferSlice,
}

/// A slice to a shared buffer. Enables us to pass a reference to the buffer to all of the
/// transmuters.
pub struct BufferSlice {
    buffer: Rc<Vec<u8>>,
    start: usize,
    size: usize,
}

impl BufferSlice {
    pub fn new(buffer: Rc<Vec<u8>>, start: usize, size: usize) -> BufferSlice {
        BufferSlice {
            buffer,
            start,
            size,
        }
    }

    pub fn get(&self) -> &[u8] {
        &self.buffer[self.start..self.start + self.size]
    }
}

impl LazyRankingsNode {
    pub fn get(&self) -> CCPResult<&RankingsNode> {
        RankingsNode::ref_from(self.buffer.get()).ok_or(error::CCPError::DataMisalignment(format!(
            "rankings node at {}",
            self.buffer.start
        )))
    }
}

pub struct LazyBlockFileCacheEntry {
    buffer: BufferSlice,
    data_files: Rc<RefCell<DataFiles>>,
}

impl LazyBlockFileCacheEntry {
    pub fn new(
        buffer: BufferSlice,
        block_files: Rc<RefCell<DataFiles>>,
    ) -> LazyBlockFileCacheEntry {
        LazyBlockFileCacheEntry {
            buffer,
            data_files: block_files,
        }
    }

    /// Parse the entry from the buffer and return a reference to it.
    pub fn get(&self) -> CCPResult<&BlockFileCacheEntry> {
        BlockFileCacheEntry::ref_from(self.buffer.get()).ok_or(error::CCPError::DataMisalignment(
            format!("block file cache entry at {}", self.buffer.start),
        ))
    }

    pub fn get_rankings_node(&mut self) -> CCPResult<LazyRankingsNode> {
        let cache_entry = self.get()?;

        if !cache_entry.rankings_node.is_initialized() {
            return Err(error::CCPError::InvalidData(
                "rankings node not initialized".to_string(),
            ));
        }

        let mut data_files = self.data_files.borrow_mut();
        let ranking_entry = data_files.get_entry(&cache_entry.rankings_node)?;

        Ok(LazyRankingsNode {
            buffer: ranking_entry,
        })
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
    pub fn entry(&self, addr: &CacheAddr) -> CCPResult<BufferSlice> {
        let header = self.header()?;
        Ok(BufferSlice::new(
            Rc::clone(&self.buffer),
            BLOCK_HEADER_SIZE + addr.start_block() as usize * header.entry_size as usize,
            header.entry_size as usize,
        ))
    }
}
