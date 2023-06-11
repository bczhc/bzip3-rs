//! Read-based BZip3 compressor and decompressor.

use std::io::{Cursor, ErrorKind, Read, Write};
use std::mem::MaybeUninit;
use std::{io, slice};

use byteorder::{ReadBytesExt, WriteBytesExt, LE};

use libbzip3_sys::{bz3_decode_block, bz3_encode_block};

use crate::errors::*;
use crate::{init_buffer, transmute_uninitialized_buffer, Bz3State, TryReadExact, MAGIC_NUMBER};

pub struct Bz3Encoder<R>
where
    R: Read,
{
    state: Bz3State,
    reader: R,
    /// Temporary buffer for [`Read::read`]
    buffer: Vec<MaybeUninit<u8>>,
    buffer_pos: usize,
    buffer_len: usize,
    block_size: usize,
}

impl<R> Bz3Encoder<R>
where
    R: Read,
{
    /// The block size must be between 65kiB and 511MiB.
    ///
    /// # Errors
    ///
    /// This returns [`Error::BlockSize`] if the block size is invalid.
    pub fn new(reader: R, block_size: usize) -> Result<Self> {
        let state = Bz3State::new(block_size)?;

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
            block_size,
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

            let read_size = self
                .reader
                .try_read_exact(&mut data_buffer[..self.block_size])?;

            let new_size =
                bz3_encode_block(self.state.raw, data_buffer.as_mut_ptr(), read_size as i32);
            if new_size == -1 {
                return Err(Error::ProcessBlock(self.state.error().into()));
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

impl<R> Read for Bz3Encoder<R>
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

pub struct Bz3Decoder<R>
where
    R: Read,
{
    state: Bz3State,
    reader: R,
    /// Temporary buffer for [`Read::read`]
    buffer: Vec<MaybeUninit<u8>>,
    buffer_pos: usize,
    buffer_len: usize,
    block_size: usize,
}

impl<R> Bz3Decoder<R>
where
    R: Read,
{
    /// Create a BZIP3 decoder.
    ///
    /// # Errors
    ///
    /// When creating, this function reads the bzip3 header
    /// from `reader`, and checks it. Error types are
    /// [`io::Error`] and [`Error::InvalidSignature`].
    pub fn new(mut reader: R) -> Result<Self> {
        let mut signature = [0_u8; MAGIC_NUMBER.len()];
        let result = reader.read_exact(&mut signature);
        if let Err(e) = result {
            if e.kind() != ErrorKind::UnexpectedEof {
                return Err(e.into());
            }
        }
        if &signature != MAGIC_NUMBER {
            return Err(Error::InvalidSignature);
        }

        let block_size = reader.read_i32::<LE>()? as usize;
        let state = Bz3State::new(block_size)?;

        let buffer_size = block_size + block_size / 50 + 32;
        let buffer = init_buffer(buffer_size);

        Ok(Self {
            state,
            reader,
            buffer_pos: 0,
            buffer_len: 0,
            buffer,
            block_size,
        })
    }

    /// The block size of the BZip3 stream.
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// Decompress and fill the buffer.
    ///
    /// Returns the original data size. Zero indicates a normal EOF.
    ///
    /// # Errors:
    ///
    /// Types: [`Error::ProcessBlock`], [`io::Error`]
    fn decompress_block(&mut self) -> Result<i32> {
        // Handle the block head. If there's no data to read, it reaches EOF of the bzip3 stream.
        let mut new_size_buf = [0_u8; 4];
        let len = self.reader.try_read_exact(&mut new_size_buf)?;
        let new_size = match len {
            0 => {
                // a normal EOF
                return Ok(0);
            }
            4 => {
                use byteorder::ByteOrder;
                LE::read_i32(&new_size_buf)
            }
            _ => {
                // corrupt stream
                return Err(Error::Io(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "Corrupt file; insufficient block head info",
                )));
            }
        };
        let read_size = self.reader.read_i32::<LE>()?;

        debug_assert!(self.buffer.len() >= read_size as usize);

        let buffer = unsafe { transmute_uninitialized_buffer(&mut self.buffer) };
        self.reader.read_exact(&mut buffer[..(new_size as usize)])?;

        unsafe {
            let result = bz3_decode_block(self.state.raw, buffer.as_mut_ptr(), new_size, read_size);
            if result == -1 {
                return Err(Error::ProcessBlock(self.state.error().into()));
            }
        };

        self.buffer_len = read_size as usize;
        Ok(read_size)
    }
}

impl<R> Read for Bz3Decoder<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.buffer_pos == self.buffer_len {
            self.buffer_pos = 0;
            // re-fill the buffer
            match self.decompress_block() {
                Ok(size) if size == 0 => {
                    // EOF
                    return Ok(0);
                }
                Err(Error::ProcessBlock(msg)) => {
                    return Err(io::Error::new(ErrorKind::Other, msg));
                }
                Err(Error::Io(e)) => {
                    return Err(e);
                }
                Ok(_) => {}
                _ => {
                    unreachable!();
                }
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
