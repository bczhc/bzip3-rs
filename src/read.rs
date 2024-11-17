//! Read-based BZip3 compressor and decompressor.

use std::io::{ErrorKind, Read, Write};
use std::{io, slice};

use byteorder::{ReadBytesExt, WriteBytesExt, LE};

use libbzip3_sys::{bz3_decode_block, bz3_encode_block};

use crate::errors::*;
use crate::{bound, Bz3State, TryReadExact, BLOCK_SIZE_MAX, BLOCK_SIZE_MIN, MAGIC_NUMBER};

pub struct Bz3Encoder<R>
where
    R: Read,
{
    state: Bz3State,
    reader: R,
    /// Temporary buffer for [`Read::read`].
    buffer: Vec<u8>,
    buffer_pos: usize,
    buffer_len: usize,
    block_size: usize,
    /// The underlying `reader` EOF indicator.
    ///
    /// Its function is to ensure that, after EOF is
    /// reached, all further `read` calls emit zero read size return-value.
    eof: bool,
}

impl<R> Bz3Encoder<R>
where
    R: Read,
{
    /// Creates a new read-based bzip3 encoder.
    ///
    /// Valid block size is between [`BLOCK_SIZE_MIN`] and [`BLOCK_SIZE_MAX`] bytes.
    ///
    /// # Errors
    ///
    /// This returns [`Error::BlockSize`] if the block size is invalid.
    pub fn new(reader: R, block_size: usize) -> Result<Self> {
        let state = Bz3State::new(block_size)?;

        let buffer_size = bound(block_size) + MAGIC_NUMBER.len() + 4;
        let mut buffer = vec![0_u8; buffer_size];

        let mut header = Vec::new();
        header.write_all(MAGIC_NUMBER).unwrap();
        header.write_i32::<LE>(block_size as i32).unwrap();
        buffer[..header.len()].copy_from_slice(&header);

        Ok(Self {
            state,
            reader,
            buffer,
            buffer_pos: 0,
            buffer_len: header.len(), /* default buffer holds the header */
            block_size,
            eof: false,
        })
    }

    /// Compress and fill the buffer.
    ///
    /// Return the size read from `self.reader`; zero indicates EOF.
    fn compress_block(&mut self) -> Result<usize> {
        unsafe {
            let buffer = slice::from_raw_parts_mut(self.buffer.as_mut_ptr(), self.buffer.len());

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
            use byteorder::ByteOrder;
            LE::write_i32(buffer, new_size);
            LE::write_i32(&mut buffer[4..], read_size as i32);

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
            // when the underlying `reader` reaches EOF and also
            // the buffer maintained by this struct is empty, it's all the end
            // TODO: inconsistency EOF mark with `read::Bz3Decoder`
            if self.eof {
                return Ok(0);
            }

            // reset buffer position, and re-fill the buffer
            self.buffer_pos = 0;
            match self.compress_block() {
                Ok(read_size) => {
                    // `try_read_exact` defines this is reaching EOF
                    // but still have some data
                    if read_size < self.block_size {
                        self.eof = true;
                    }
                    // also EOF and no more data to process; immediately end this `read` call
                    if read_size == 0 {
                        self.eof = true;
                        return Ok(0);
                    }
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
            buf.as_mut_ptr()
                .copy_from(self.buffer[self.buffer_pos..].as_ptr(), required_length);
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
    /// Temporary buffer for [`Read::read`].
    buffer: Vec<u8>,
    buffer_pos: usize,
    buffer_len: usize,
    block_size: usize,
    /// Underlying `reader` EOF indicator.
    eof: bool,
}

impl<R> Bz3Decoder<R>
where
    R: Read,
{
    /// Creates a read-based bzip3 decoder.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidSignature`] for invalid file header signature, and
    /// [`Error::Io`] on all IO errors.
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

        let buffer_size = bound(block_size);
        let buffer = vec![0_u8; buffer_size];

        Ok(Self {
            state,
            reader,
            buffer_pos: 0,
            buffer_len: 0,
            buffer,
            block_size,
            eof: false,
        })
    }

    /// Returns the bzip3 block size associated with the current state.
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// Decompress and fill the buffer.
    ///
    /// Returning true indicates EOF.
    ///
    /// # Errors:
    ///
    /// Types: [`Error::ProcessBlock`], [`io::Error`]
    fn decompress_block(&mut self) -> Result<bool> {
        // Handle the block head. If there's no data to read, it reaches EOF of the bzip3 stream.
        let mut new_size_buf = [0_u8; 4];
        let len = self.reader.try_read_exact(&mut new_size_buf)?;
        let new_size = match len {
            0 => {
                // a normal EOF
                return Ok(true);
            }
            4 => {
                use byteorder::ByteOrder;
                LE::read_i32(&new_size_buf)
            }
            _ => {
                // unexpected EOF; corrupt stream
                return Err(Error::Io(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "Corrupt file; insufficient block head info",
                )));
            }
        };
        let read_size = self.reader.read_i32::<LE>()? as usize;

        debug_assert!(self.buffer.len() >= read_size);

        let buffer = &mut self.buffer;
        self.reader.read_exact(&mut buffer[..(new_size as usize)])?;

        unsafe {
            let result = bz3_decode_block(
                self.state.raw,
                buffer.as_mut_ptr(),
                new_size,
                read_size as i32,
            );
            if result == -1 {
                return Err(Error::ProcessBlock(self.state.error().into()));
            }
        };

        self.buffer_len = read_size;
        Ok(false)
    }

    /// Decompresses the next block, but skips empty blocks.
    ///
    /// Currently, `decompress_block` will be called (once and only once)
    /// on each `read` call,
    /// and if it meets an empty block, `self.buffer_len` will be zero.
    /// Thus, the `Read::read` function will return zero which means
    /// the stream reaches EOF, but actually it doesn't.
    ///
    /// Returns EOF flag; true indicates EOF
    fn decompress_next_nonempty_block(&mut self) -> Result<bool> {
        // use loop to skip empty blocks
        // one empty block has a `read_size` of zero
        // Example stream:
        // 00000000: 0800 0000 0000 0000 0100 0000 ffff ffff  ................
        loop {
            let eof = self.decompress_block()?;
            if eof {
                return Ok(true);
            }
            if self.buffer_len /* the `read_size` */ == 0 {
                continue;
            }
            return Ok(false);
        }
    }
}

impl<R> Read for Bz3Decoder<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.eof {
            return Ok(0);
        }
        if self.buffer_pos == self.buffer_len {
            self.buffer_pos = 0;
            // re-fill the buffer
            match self.decompress_next_nonempty_block() {
                Ok(false) => {}
                Ok(true) => {
                    self.eof = true;
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
            buf.as_mut_ptr()
                .copy_from(self.buffer[self.buffer_pos..].as_ptr(), required_length);
        }
        self.buffer_pos += required_length;
        Ok(required_length)
    }
}
