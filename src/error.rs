use thiserror::Error;

pub type CCPResult<T> = Result<T, CCPError>;

#[derive(Error, Debug)]
pub enum CCPError {
    #[error("io error")]
    Io {
        #[from]
        source: std::io::Error,
    },
    #[error("unsupported version ({0})")]
    UnsupportedVersion(String),
    #[error("invalid data ({0})")]
    InvalidData(String),
    #[error("data misalignment ({0})")]
    DataMisalignment(String),
    #[error("chrome cache index does not exist at {0}")]
    IndexDoesNotExist(String),
    #[error("chrome cache location could not be determined")]
    CacheLocationCouldNotBeDetermined(),
    #[error("invalid timestamp ({0})")]
    InvalidTimestamp(u64),
}
