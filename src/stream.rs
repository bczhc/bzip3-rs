//! BZip3 compressor and decompressor
//! that do a direct stream-to-stream process
//!
//! TODO: handle panics

use crate::{check_block_size, TryReadExact, MAGIC_NUMBER};
use byteorder::{ReadBytesExt, WriteBytesExt, LE};
use bytesize::ByteSize;
use libbzip3_sys::*;
use std::ffi::CStr;
use std::io::{ErrorKind, Read, Write};
use std::mem::MaybeUninit;
use std::{io, slice};

/// Compress `reader` to `writer`.
///
/// The block size must be between 65kiB and 511MiB.
pub fn compress<R, W>(reader: &mut R, writer: &mut W, block_size: usize) -> io::Result<()>
where
    R: Read,
    W: Write,
{
    let buffer_size = block_size + block_size / 50 + 32;

    writer.write_all(MAGIC_NUMBER)?;
    writer.write_u32::<LE>(block_size as u32)?;
    unsafe {
        let bz3 = bz3_new(block_size as i32);

        let mut buffer = vec![MaybeUninit::<u8>::uninit(); buffer_size];
        let buffer = slice::from_raw_parts_mut(buffer.as_mut_ptr() as *mut u8, buffer_size);
        loop {
            let read_len = reader.try_read_exact(&mut buffer[..block_size])?;
            if read_len == 0 {
                break;
            }

            let new_size = bz3_encode_block(bz3, buffer.as_mut_ptr(), read_len as i32);
            if new_size == -1 {
                let err_msg = CStr::from_ptr(bz3_strerror(bz3)).to_string_lossy();
                panic!("Failed to encode a block: {}", err_msg);
            }

            writer.write_u32::<LE>(new_size as u32)?;
            writer.write_u32::<LE>(read_len as u32)?;
            writer.write_all(&buffer[..(new_size as usize)])?;
        }
        writer.flush()?;
        bz3_free(bz3);
    }
    Ok(())
}

/// Decompress `reader` to `writer`.
pub fn decompress<R, W>(reader: &mut R, writer: &mut W) -> io::Result<()>
where
    R: Read,
    W: Write,
{
    let mut magic_num = [0_u8; 5];
    reader.read_exact(&mut magic_num)?;
    if &magic_num != MAGIC_NUMBER {
        panic!("Invalid signature");
    }
    let block_size = reader.read_u32::<LE>()? as usize;
    if check_block_size(block_size).is_err() {
        panic!(
            "Invalid file. Reason: invalid block size: {}",
            ByteSize(block_size as u64).to_string_as(true)
        );
    }

    let buffer_size = block_size + block_size / 50 + 32;
    unsafe {
        let bz3 = bz3_new(block_size as i32);

        let mut buffer = vec![MaybeUninit::<u8>::uninit(); buffer_size];
        let buffer = slice::from_raw_parts_mut(buffer.as_mut_ptr() as *mut u8, buffer_size);
        loop {
            let new_size = match reader.read_u32::<LE>() {
                Ok(s) => s,
                Err(e) => {
                    if e.kind() == ErrorKind::UnexpectedEof {
                        break;
                    } else {
                        return Err(e);
                    }
                }
            };

            let old_size = reader.read_u32::<LE>()?;
            reader.read_exact(&mut buffer[..(new_size as usize)])?;

            let result =
                bz3_decode_block(bz3, buffer.as_mut_ptr(), new_size as i32, old_size as i32);
            if result == -1 {
                panic!(
                    "Failed to decode a block: {}",
                    CStr::from_ptr(bz3_strerror(bz3)).to_string_lossy()
                );
            }

            writer.write_all(&buffer[..(old_size as usize)])?;
        }

        writer.flush()?;
        bz3_free(bz3);
    }

    Ok(())
}
