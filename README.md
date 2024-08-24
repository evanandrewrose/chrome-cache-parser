[![ci](https://github.com/evanandrewrose/chrome-cache-parser/actions/workflows/ci.yml/badge.svg)](https://github.com/evanandrewrose/chrome-cache-parser/actions/workflows/ci.yml)

A safe, zero-copy, rust-based chrome cache parser, supporting chrome cache versions 2.0, 2.1, and 3.0.

So far, it only parses the cache entries themselves and only for the "block" file format used on Windows. You can use it to programmatically inspect the cache index and, for example, display the known cache keys (e.g., URIs) stored in the cache, along with some entry metadata (timestamp, etc.). It provides no utility for extracting the cache data itself since I currently have no purpose for that capability. It also only supports cache keys stored inline with the cache entry, not the longer, out-of-band cache keys.

## Run The Example

By default, it'll display the cache entries from a typical google chrome cache path. Provide `--path` to point it somewhere else.

```bash
cargo run --example display-chrome-cache
```

## Example Usage

```rust
use std::{path::PathBuf};

use chrome_cache_parser::{CCPError, CCPResult, ChromeCache};
use chrono::{DateTime, Local};

let cache = ChromeCache::from_path(PathBuf::from(path)).unwrap();

let entries = cache.entries().unwrap();

entries.for_each(|e| {
    let e = e.get().unwrap();
    println!("[{:?}\t=>\t{:?}]: {:?}", e.hash, e.key, DateTime::<Local>::from(e.creation_time));
});
```

## Implementation
The implementation is mostly just transmutations via the [zerocopy](https://docs.rs/zerocopy/latest/zerocopy/) library and some lazy traversing of the cache index's hash table and internal entry linked lists.

## Background
For an overview of the chrome cache implementation, see [here](https://www.chromium.org/developers/design-documents/network-stack/disk-cache/).

The Chromium sources were helpful for understanding the cache format.

Particularly:

* [disk_cache/disk_format_base.h](https://chromium.googlesource.com/chromium/src/net/+/ddbc6c5954c4bee29902082eb9052405e83abc02/disk_cache/disk_format_base.h)
* [disk_cache/disk_format.h](https://chromium.googlesource.com/chromium/src/net/+/ddbc6c5954c4bee29902082eb9052405e83abc02/disk_cache/disk_format.h)