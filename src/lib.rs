use bytesize::ByteSize;

pub mod errors;
pub mod read;
pub mod stream;
pub mod write;

/// The signature of a bzip3 file.
pub const MAGIC_NUMBER: &[u8; 5] = b"BZ3v1";

pub(crate) fn check_block_size(block_size: usize) -> Result<(), ()> {
    if block_size < ByteSize::kib(65).0 as usize || block_size > ByteSize::mib(511).0 as usize {
        Err(())
    } else {
        Ok(())
    }
}
