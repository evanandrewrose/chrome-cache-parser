use clap::{CommandFactory, Parser};
use std::{fmt::Debug, path::PathBuf};

use chrome_cache_parser::{CCPError, CCPResult, ChromeCache};
use chrono::{DateTime, Local};

/// A simple command line tool to display the contents of a Chrome cache directory.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the cache directory (containing an index file)
    #[arg(short, long)]
    path: Option<String>,

    /// Whether to be silent
    #[arg(short, long)]
    silent: bool,
}

fn default_cache_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let windows_path = home.join("AppData/Local/Google/Chrome/User Data/Default/Cache/Cache_Data");
    let linux_path = home.join(".cache/google-chrome/Default/Cache");

    if windows_path.exists() {
        Some(windows_path)
    } else if linux_path.exists() {
        Some(linux_path)
    } else {
        None
    }
}

fn main() {
    let args = Args::parse();
    if let Err(e) = display_cache(args) {
        eprintln!("Error: {}\n", e);
        Args::command().print_help().unwrap();
    }
}

fn display_cache(args: Args) -> CCPResult<()> {
    let path = args
        .path
        .map(PathBuf::from)
        .or(default_cache_path())
        .ok_or(CCPError::CacheLocationCouldNotBeDetermined())?;
    let cache = ChromeCache::from_path(path).unwrap();

    let entries = cache.entries().unwrap();

    if !args.silent {
        entries.for_each(|mut e| {
            let cache_entry = &e.get().unwrap();
            println!(
                "[{:?}\t=>\t{:?}]: {:?}",
                cache_entry.hash,
                cache_entry.key,
                DateTime::<Local>::from(cache_entry.creation_time)
            );
            let ranking = e.get_rankings_node().unwrap();
            println!(
                "\tlast used\t{:?}",
                DateTime::<Local>::from(ranking.get().unwrap().last_used)
            );
        });
    }

    Ok(())
}
