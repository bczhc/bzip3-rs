use std::io;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("Invalid block size: must be between 65kiB and 511MiB")]
    BlockSize,
    #[error("{0}")]
    ProcessBlock(String),
}
