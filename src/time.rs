use chrono::{DateTime, Local, Utc};
use zerocopy::{FromBytes, FromZeroes};

use crate::{CCPError, CCPResult};

const MICROSEC_PER_SEC: u64 = 1_000_000;
const NANOSEC_PER_MICROSEC: u64 = 1_000;
const WIN_TO_UNIX_EPOCH_DELTA_SEC: u64 = 11_644_473_600;
const WIN_TO_UNIX_EPOCH_DIFF_MICROSEC: u64 = WIN_TO_UNIX_EPOCH_DELTA_SEC * MICROSEC_PER_SEC;

/// Represents a time in microseconds since the Windows epoch (1601-01-01 00:00:00 UTC) (used in
/// the chrome cache format).
#[derive(Debug, FromZeroes, FromBytes, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WindowsEpochMicroseconds(u64);

impl WindowsEpochMicroseconds {
    pub fn into_datetime_utc(self) -> CCPResult<DateTime<Utc>> {
        let windows_micro_seconds: u64 = self.0;

        let unix_micro_seconds = windows_micro_seconds
            .checked_sub(WIN_TO_UNIX_EPOCH_DIFF_MICROSEC)
            .ok_or(CCPError::InvalidTimestamp(windows_micro_seconds))?;
        let unix_seconds = unix_micro_seconds / MICROSEC_PER_SEC;
        let unix_nanoseconds = (unix_micro_seconds % MICROSEC_PER_SEC) * NANOSEC_PER_MICROSEC;

        DateTime::from_timestamp(unix_seconds as i64, unix_nanoseconds as u32)
            .ok_or(CCPError::InvalidTimestamp(windows_micro_seconds))
    }

    pub fn into_datetime_local(self) -> CCPResult<DateTime<Local>> {
        let utc: CCPResult<DateTime<Utc>> = self.into_datetime_utc();
        Ok(utc?.with_timezone(&Local))
    }
}

#[cfg(test)]
#[test]
fn test_windows_epoch_microseconds() {
    use chrono::{Datelike, Timelike};
    let timestamp = WindowsEpochMicroseconds(13_360_111_021_811_283);
    let date: CCPResult<DateTime<Utc>> = timestamp.into_datetime_utc();
    let date = date.unwrap();

    assert_eq!(date.year(), 2024);
    assert_eq!(date.month(), 5);
    assert_eq!(date.day(), 13);
    assert_eq!(date.hour(), 21);
    assert_eq!(date.second(), 1);
}

#[test]
fn test_windows_epoch_from_0_input_returns_error() {
    let timestamp = WindowsEpochMicroseconds(0);
    let date: CCPResult<DateTime<Utc>> = timestamp.into_datetime_utc();
    assert!(date.is_err());
}
