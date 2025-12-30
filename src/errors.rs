use std::io;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("Invalid block size")]
    BlockSize,
    #[error("{0}")]
    ProcessBlock(String),
    #[error("Invalid file signature")]
    InvalidSignature,
}

impl Error {
    pub(crate) fn into_io_error(self) -> io::Error {
        match self {
            Self::Io(e) => e,
            e => io::Error::other(e),
        }
    }
}
