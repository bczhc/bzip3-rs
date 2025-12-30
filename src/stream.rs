//! BZip3 compressor and decompressor
//! that do a direct stream-to-stream process.
use byteorder::{ReadBytesExt, WriteBytesExt, LE};
use rayon::prelude::*;
use std::io;
use std::io::{Read, Write};

use crate::errors::*;
use crate::{bound, Bz3State, MAGIC_NUMBER};

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

pub fn parallel_compress<R: Read, W: Write>(
    mut reader: R,
    mut writer: W,
    block_size: usize,
) -> Result<()> {
    writer.write_all(MAGIC_NUMBER)?;
    writer.write_i32::<LE>(block_size as i32)?;

    let num_cpus = rayon::current_num_threads();

    loop {
        let mut chunks = Vec::with_capacity(num_cpus);

        for _ in 0..num_cpus {
            let mut buf = vec![0u8; block_size];
            let mut read_size = 0;
            while read_size < block_size {
                let n = reader.read(&mut buf[read_size..])?;
                if n == 0 {
                    break;
                }
                read_size += n;
            }
            if read_size > 0 {
                chunks.push((buf, read_size));
            } else {
                break;
            }
        }

        if chunks.is_empty() {
            break;
        }

        let results: Vec<Result<(Vec<u8>, usize, usize)>> = chunks
            .into_par_iter()
            .map(|(mut buf, original_size)| {
                let mut state = Bz3State::new(block_size)?;
                let mut out_buf = vec![0u8; bound(original_size)];
                out_buf[..original_size].copy_from_slice(&buf[..original_size]);

                let new_size = state.encode_block(&mut out_buf, original_size)?;
                Ok((out_buf, new_size, original_size))
            })
            .collect();

        for res in results {
            let (compressed_data, new_size, original_size) = res?;
            writer.write_i32::<LE>(new_size as i32)?;
            writer.write_i32::<LE>(original_size as i32)?;
            writer.write_all(&compressed_data[..new_size])?;
        }
    }

    Ok(())
}

pub fn parallel_decompress<R: Read, W: Write>(mut reader: R, mut writer: W) -> Result<()> {
    let mut sig = [0u8; 5];
    reader
        .read_exact(&mut sig)
        .map_err(|_| Error::InvalidSignature)?;
    if &sig != MAGIC_NUMBER {
        return Err(Error::InvalidSignature);
    }

    let block_size = reader.read_i32::<LE>()? as usize;
    let num_cpus = rayon::current_num_threads();

    loop {
        let mut blocks = Vec::with_capacity(num_cpus);

        for _ in 0..num_cpus {
            let new_size = match reader.read_i32::<LE>() {
                Ok(s) => s as usize,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            };
            let original_size = reader.read_i32::<LE>()? as usize;

            let mut compressed_buf = vec![0u8; std::cmp::max(bound(original_size), new_size)];
            reader.read_exact(&mut compressed_buf[..new_size])?;

            blocks.push((compressed_buf, new_size, original_size));
        }

        if blocks.is_empty() {
            break;
        }

        let results: Vec<Result<(Vec<u8>, usize)>> = blocks
            .into_par_iter()
            .map(|(mut buf, new_size, original_size)| {
                let mut state = Bz3State::new(block_size)?;
                state.decode_block(&mut buf, new_size, original_size)?;
                Ok((buf, original_size))
            })
            .collect();

        for res in results {
            let (decompressed_data, original_size) = res?;
            writer.write_all(&decompressed_data[..original_size])?;
        }
    }

    Ok(())
}
