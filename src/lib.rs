//! A safe, zero-copy, rust-based chrome cache entry parser, supporting chrome cache versions 2.0,
//! 2.1, and 3.0.
pub mod block_file;
pub mod cache_address;
pub mod cache_index;
pub mod error;
pub mod time;

pub use crate::cache_address::CacheAddr;
use crate::cache_index::CacheVersion;
pub use crate::cache_index::IndexHeader;
pub use crate::error::{CCPError, CCPResult};

use block_file::{LazyBlockFile, LazyBlockFileCacheEntry, LazyBlockFileCacheEntryIterator};
use cache_address::CACHE_ADDRESS_SIZE;
use cache_index::INDEX_HEADER_SIZE;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use zerocopy::{FromBytes, Ref};

/// A Chrome cache parser. Internally, it only stores the path to the cache and
/// the cache's index as a buffer.
pub struct ChromeCache {
    path: PathBuf,
    buffer: Vec<u8>,
}

impl ChromeCache {
    pub fn from_path(path: PathBuf) -> CCPResult<ChromeCache> {
        let index = Self::path_to_index(&path);
        if !index.exists() {
            return Err(CCPError::IndexDoesNotExist(
                index.to_string_lossy().to_string(),
            ));
        }

        let mut index_buffer = Vec::new();
        let mut f = fs::File::open(index)?;
        f.read_to_end(&mut index_buffer)?;

        let chrome_cache = ChromeCache {
            path,
            buffer: index_buffer,
        };

        let header = ChromeCache::header(&chrome_cache)?;
        let version = CacheVersion::from(header.version);

        if header.magic != cache_index::INDEX_MAGIC {
            return Err(CCPError::InvalidData("invalid index magic".to_string()));
        }

        if let CacheVersion::Unknown(version) = version {
            return Err(CCPError::UnsupportedVersion(format!(
                "unsupported version ({:x})",
                version
            )));
        }

        Ok(chrome_cache)
    }

    pub fn header(&self) -> CCPResult<&IndexHeader> {
        IndexHeader::ref_from(&self.buffer[0..INDEX_HEADER_SIZE]).ok_or(CCPError::DataMisalignment(
            "index header misalignment".to_string(),
        ))
    }

    pub fn addresses(&self) -> CCPResult<&[CacheAddr]> {
        let table_len = self.header()?.table_len as usize;
        let begin = INDEX_HEADER_SIZE;
        let end = begin + table_len * CACHE_ADDRESS_SIZE;
        let addresses = Ref::<_, [CacheAddr]>::new_slice(&self.buffer[begin..end]).ok_or(
            CCPError::DataMisalignment("cache address table misalignment".to_string()),
        )?;

        Ok(addresses.into_slice())
    }

    fn path_to_index(cache_dir: &Path) -> PathBuf {
        cache_dir.join("index")
    }

    pub fn entries(&self) -> CCPResult<impl Iterator<Item = LazyBlockFileCacheEntry> + '_> {
        // A map from the data file number to the data file.
        let data_files: Rc<RefCell<HashMap<u32, LazyBlockFile>>> =
            Rc::new(RefCell::new(HashMap::new()));

        let entries = self
            .addresses()?
            .iter()
            .filter(|addr| addr.is_initialized())
            .zip(std::iter::repeat(data_files))
            .flat_map(|(addr, data_files)| {
                LazyBlockFileCacheEntryIterator::new(self.path.to_path_buf(), data_files, *addr)
            });

        Ok(entries)
    }
}
