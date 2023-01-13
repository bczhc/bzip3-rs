//! Read-based BZip3 compressor and decompressor.

use std::ffi::CStr;
use std::io::{Cursor, ErrorKind, Read, Write};
use std::mem::MaybeUninit;
use std::{io, slice};

use byteorder::{WriteBytesExt, LE};

use libbzip3_sys::{bz3_encode_block, bz3_free, bz3_new, bz3_state, bz3_strerror};

use crate::errors::*;
use crate::{check_block_size, TryReadExact, MAGIC_NUMBER};

pub struct Bz3Encoder<'a, R>
where
    R: Read,
{
    state: *mut bz3_state,
    reader: &'a mut R,
    /// The temporary buffer for [`Read::read`]
    buffer: Vec<MaybeUninit<u8>>,
    buffer_pos: usize,
    buffer_len: usize,
}

impl<'a, R> Bz3Encoder<'a, R>
where
    R: Read,
{
    /// The block size must be between 65kiB and 511MiB.
    ///
    /// # Errors
    ///
    /// This returns [`Error::BlockSize`] if the block size is invalid.
    pub fn new(reader: &'a mut R, block_size: usize) -> Result<Self> {
        if check_block_size(block_size).is_err() {
            return Err(Error::BlockSize);
        }
        let state = unsafe {
            let state = bz3_new(block_size as i32);
            if state.is_null() {
                panic!("Allocation fails");
            }
            state
        };

        let buffer_size = block_size + block_size / 50 + 32 + MAGIC_NUMBER.len() + 4;
        let mut buffer = Vec::<MaybeUninit<u8>>::with_capacity(buffer_size);
        unsafe {
            buffer.set_len(buffer_size);
        }

        let mut header = Cursor::new(Vec::new());
        header.write_all(MAGIC_NUMBER).unwrap();
        header.write_i32::<LE>(block_size as i32).unwrap();
        for x in header.get_ref().iter().enumerate() {
            buffer[x.0] = MaybeUninit::new(*x.1);
        }

        Ok(Self {
            state,
            reader,
            buffer,
            buffer_pos: 0,
            buffer_len: header.get_ref().len(), /* default buffer holds the header */
        })
    }

    /// Compress and fill the buffer.
    /// Return the size read from `self.reader`; zero indicates EOF.
    fn compress_block(&mut self) -> Result<usize> {
        unsafe {
            let buffer =
                slice::from_raw_parts_mut(self.buffer.as_mut_ptr() as *mut u8, self.buffer.len());

            // structure of a block: [ new_size (i32) | read_size (i32) | compressed data ]
            // skip 8 bytes to write the buffer first
            let data_buffer = &mut buffer[8..];

            let read_size = self.reader.try_read_exact(data_buffer)?;

            let new_size = bz3_encode_block(self.state, data_buffer.as_mut_ptr(), read_size as i32);
            if new_size == -1 {
                return Err(Error::ProcessBlock(
                    CStr::from_ptr(bz3_strerror(self.state))
                        .to_string_lossy()
                        .into(),
                ));
            }

            // go back and fill new_size and read_size
            let mut cursor = Cursor::new(buffer);
            cursor.write_i32::<LE>(new_size)?;
            cursor.write_i32::<LE>(read_size as i32)?;

            self.buffer_len = 4 + 4 + new_size as usize;
            Ok(read_size)
        }
    }
}

impl<'a, R> Read for Bz3Encoder<'a, R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.buffer_pos == self.buffer_len {
            // reset buffer position, and re-fill the buffer
            self.buffer_pos = 0;
            match self.compress_block() {
                Ok(read_size) if read_size == 0 => {
                    // EOF
                    return Ok(0);
                }
                Err(Error::ProcessBlock(msg)) => {
                    return Err(io::Error::new(ErrorKind::Other, msg));
                }
                Err(Error::Io(e)) => {
                    return Err(e);
                }
                Err(_) => {
                    unreachable!();
                }
                _ => {}
            }
        }

        assert!(self.buffer_pos < self.buffer_len);
        // have data from buffer to read
        let remaining_size = self.buffer_len - self.buffer_pos;

        let mut required_length = buf.len();
        if required_length > remaining_size {
            required_length = remaining_size;
        }

        unsafe {
            buf.as_mut_ptr().copy_from(
                self.buffer[self.buffer_pos..].as_ptr() as *const u8,
                required_length,
            );
        }
        self.buffer_pos += required_length;
        Ok(required_length)
    }
}

impl<'a, R> Drop for Bz3Encoder<'a, R>
where
    R: Read,
{
    fn drop(&mut self) {
        unsafe {
            bz3_free(self.state);
        }
    }
}

/*pub struct Bz3Decoder<'a, R>
where
    R: Read,
{
    state: *mut bz3_state,
    reader: &'a mut R,
}

impl<'a, R> Bz3Decoder<'a, R>
where
    R: Read,
{
    fn new(reader: &mut R) {
        todo!()
    }
}

impl<'a, R> Read for Bz3Decoder<'a, R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }
}
*/
