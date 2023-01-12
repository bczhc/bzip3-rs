pub mod errors;
pub mod read;
pub mod stream;
pub mod write;

/// The signature of a bzip3 file.
pub const MAGIC_NUMBER: &[u8; 5] = b"BZ3v1";
