use std::{
    cell::RefCell,
    cmp::min,
    collections::{hash_map::Entry, HashMap},
    fmt,
    fs::{self, File},
    io::{self, BufReader, Read},
    mem,
    path::PathBuf,
    rc::Rc,
};

use zerocopy::{FromBytes, FromZeroes};

use crate::{
    cache_address::{CacheAddr, FileType},
    error::{self, CCPResult},
    time::WindowsEpochMicroseconds,
    CCPError,
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

#[derive(Debug, Clone, PartialEq)]
#[repr(u32)]
pub enum BlockCacheEntryState {
    Normal = 0,
    Evicted = 1,
    Doomed = 2,
    Unknown,
}

impl From<i32> for BlockCacheEntryState {
    fn from(value: i32) -> Self {
        match value {
            0 => BlockCacheEntryState::Normal,
            1 => BlockCacheEntryState::Evicted,
            2 => BlockCacheEntryState::Doomed,
            _ => BlockCacheEntryState::Unknown,
        }
    }
}

#[derive(Debug, FromZeroes, FromBytes, Clone, Copy)]
pub struct BlockCacheEntryStateField(i32);

impl BlockCacheEntryStateField {
    // zerocopy lib doesn't provide a mechanism for decoding enums that don't represent all
    // states, see: https://github.com/google/zerocopy/issues/1429
    pub fn kind(&self) -> BlockCacheEntryState {
        BlockCacheEntryState::from(self.0)
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
    pub state: BlockCacheEntryStateField,
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

struct BlockFileStreamReader {
    addr: CacheAddr,
    size: usize,
    data_files: Rc<RefCell<DataFiles>>,
    read_offset: usize,
}

impl BlockFileStreamReader {
    pub fn new(
        addr: CacheAddr,
        size: usize,
        data_files: Rc<RefCell<DataFiles>>,
    ) -> BlockFileStreamReader {
        BlockFileStreamReader {
            addr,
            size,
            data_files,
            read_offset: 0,
        }
    }
}

impl Read for BlockFileStreamReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.read_offset >= self.size {
            return Ok(0);
        }

        let mut data_files = self.data_files.borrow_mut();
        let data_file = match data_files.get(self.addr.file_number()) {
            Ok(file) => file,
            Err(CCPError::Io { source }) => return Err(source),
            Err(err) => return Err(io::Error::new(io::ErrorKind::Other, err)),
        };

        let block_size = self
            .addr
            .file_type()
            .block_size()
            .or(Err(io::ErrorKind::InvalidData))?;
        let start_addr =
            BLOCK_HEADER_SIZE + self.addr.start_block() as usize * block_size + self.read_offset;
        let to_be_read = min(buf.len(), self.size - self.read_offset);
        let end_addr = start_addr + to_be_read;

        buf[0..to_be_read].copy_from_slice(&data_file.buffer[start_addr..end_addr]);

        self.read_offset += to_be_read;

        Ok(to_be_read)
    }
}

struct ExternalFileReader {
    addr: CacheAddr,
    file: Option<BufReader<File>>,
    cache_path: PathBuf,
}

impl ExternalFileReader {
    pub fn new(addr: CacheAddr, cache_path: PathBuf) -> ExternalFileReader {
        ExternalFileReader {
            addr,
            file: None,
            cache_path,
        }
    }
}

impl Read for ExternalFileReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(file) = &mut self.file {
            file.read(buf)
        } else {
            let file_name = format!("f_{:0>6x}", self.addr.file_number());
            let reader = File::open(self.cache_path.join(file_name))?;
            self.file.replace(BufReader::new(reader));
            self.read(buf)
        }
    }
}

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
    cache_path: PathBuf,
}

impl LazyBlockFileCacheEntryIterator {
    pub fn new(
        data_files: Rc<RefCell<DataFiles>>,
        start: CacheAddr,
        cache_path: PathBuf,
    ) -> LazyBlockFileCacheEntryIterator {
        LazyBlockFileCacheEntryIterator {
            current: Some(start),
            data_files,
            cache_path,
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

    fn get(&mut self, file_number: u32) -> CCPResult<&LazyBlockFile> {
        Ok(match self.data_files.entry(file_number) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let file_path = self.path.join(format!("data_{}", file_number));
                let mut file = fs::File::open(&file_path)?;
                let mut buf: Vec<u8> = Vec::new();
                file.read_to_end(&mut buf)?;
                entry.insert(LazyBlockFile::new(Rc::new(buf)))
            }
        })
    }

    pub fn get_entry(&mut self, addr: &CacheAddr) -> CCPResult<BufferSlice> {
        let data_file = self.get(addr.file_number())?;
        data_file.get_buffer(addr)
    }
}

impl Iterator for LazyBlockFileCacheEntryIterator {
    type Item = LazyBlockFileCacheEntry;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.take()?;

        let mut data_files = (*self.data_files).borrow_mut();

        let current = data_files.get_entry(&current).ok()?;
        let current = LazyBlockFileCacheEntry::new(
            current,
            Rc::clone(&self.data_files),
            self.cache_path.clone(),
        );

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
    cache_path: PathBuf,
}

impl LazyBlockFileCacheEntry {
    pub fn new(
        buffer: BufferSlice,
        block_files: Rc<RefCell<DataFiles>>,
        cache_path: PathBuf,
    ) -> LazyBlockFileCacheEntry {
        LazyBlockFileCacheEntry {
            buffer,
            data_files: block_files,
            cache_path,
        }
    }

    /// Parse the entry from the buffer and return a reference to it.
    pub fn get(&self) -> CCPResult<&BlockFileCacheEntry> {
        BlockFileCacheEntry::ref_from(self.buffer.get()).ok_or(error::CCPError::DataMisalignment(
            format!("block file cache entry at {}", self.buffer.start),
        ))
    }

    /// Return readers for the actual cache data. Typically, this is a header stream followed by
    /// a content stream.
    pub fn stream_readers(self) -> CCPResult<Vec<CCPResult<Box<dyn Read>>>> {
        let entry = self.get().or(Err(CCPError::InvalidState(
            "Unable to read entry".to_string(),
        )))?;

        Ok(entry
            .data_addr
            .iter()
            .zip(entry.data_size.iter())
            .map(|(addr, size)| match addr.file_type() {
                FileType::External => Ok(Box::new(ExternalFileReader::new(
                    *addr,
                    self.cache_path.clone(),
                )) as Box<dyn Read>),
                FileType::Block1k | FileType::Block256 | FileType::Block4k => Ok(Box::new(
                    BlockFileStreamReader::new(*addr, *size as usize, self.data_files.clone()),
                )
                    as Box<dyn Read>),
                _ => Err(CCPError::InvalidState(
                    format!(
                        "Requested stream reader of nonsense address type {:?}",
                        addr.file_type()
                    )
                    .to_string(),
                )),
            })
            .collect())
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

    pub fn get_buffer(&self, addr: &CacheAddr) -> CCPResult<BufferSlice> {
        let header = self.header()?;
        Ok(BufferSlice::new(
            Rc::clone(&self.buffer),
            BLOCK_HEADER_SIZE + addr.start_block() as usize * header.entry_size as usize,
            header.entry_size as usize,
        ))
    }
}
