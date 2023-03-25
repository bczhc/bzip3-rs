//! BZip3 compressor and decompressor
//! that do a direct stream-to-stream process
use std::io;
use std::io::{Read, Write};

use crate::errors::*;

/// Compress `reader` to `writer`.
///
/// The block size must be between 65kiB and 511MiB.
pub fn compress<R, W>(mut reader: R, mut writer: W, block_size: usize) -> Result<()>
where
    R: Read,
    W: Write,
{
    let mut encoder = crate::read::Bz3Encoder::new(&mut reader, block_size)?;
    io::copy(&mut encoder, &mut writer)?;
    Ok(())
}

/// Decompress `reader` to `writer`.
pub fn decompress<R, W>(mut reader: R, mut writer: W) -> Result<()>
where
    R: Read,
    W: Write,
{
    let mut decoder = crate::read::Bz3Decoder::new(&mut reader)?;
    io::copy(&mut decoder, &mut writer)?;
    Ok(())
}
