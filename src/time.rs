use chrono::{DateTime, Local, Utc};
use zerocopy::{FromBytes, FromZeroes};

const MICROSEC_PER_SEC: u64 = 1_000_000;
const NANOSEC_PER_MICROSEC: u64 = 1_000;
const WIN_TO_UNIX_EPOCH_DELTA_SEC: u64 = 11_644_473_600;
const WIN_TO_UNIX_EPOCH_DIFF_MICROSEC: u64 = WIN_TO_UNIX_EPOCH_DELTA_SEC * MICROSEC_PER_SEC;

/// Represents a time in microseconds since the Windows epoch (1601-01-01 00:00:00 UTC) (used in
/// the chrome cache format).
#[derive(Debug, FromZeroes, FromBytes, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WindowsEpochMicroseconds(u64);

impl From<WindowsEpochMicroseconds> for DateTime<Utc> {
    fn from(micro_seconds: WindowsEpochMicroseconds) -> Self {
        let windows_micro_seconds: u64 = micro_seconds.0;

        let unix_micro_seconds = windows_micro_seconds - WIN_TO_UNIX_EPOCH_DIFF_MICROSEC;
        let unix_seconds = unix_micro_seconds / MICROSEC_PER_SEC;
        let unix_nanoseconds = (unix_micro_seconds % MICROSEC_PER_SEC) * NANOSEC_PER_MICROSEC;

        DateTime::from_timestamp(unix_seconds as i64, unix_nanoseconds as u32).unwrap()
    }
}

impl From<WindowsEpochMicroseconds> for DateTime<Local> {
    fn from(micro_seconds: WindowsEpochMicroseconds) -> Self {
        let utc = DateTime::<Utc>::from(micro_seconds);
        utc.with_timezone(&Local)
    }
}
