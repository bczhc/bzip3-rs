//! Write-based BZip3 compressor and decompressor.

use std::io;
use std::io::{Cursor, Read, Write};

use byteorder::{ReadBytesExt, WriteBytesExt, LE};

use crate::errors::*;
use crate::{bound, Bz3State, BLOCK_SIZE_MAX, BLOCK_SIZE_MIN, MAGIC_NUMBER};

pub struct Bz3Encoder<W>
where
    W: Write,
{
    writer: W,
    state: Bz3State,
    buffer: Vec<u8>,
    buffer_pos: usize,
    block_size: usize,
}

impl<W> Bz3Encoder<W>
where
    W: Write,
{
    /// Creates a new bzip3 stream encoder.
    ///
    /// Valid block size is between [`BLOCK_SIZE_MIN`] and [`BLOCK_SIZE_MAX`] bytes.
    ///
    /// # Errors
    ///
    /// This returns [`Error::BlockSize`] if the block size is invalid.
    pub fn new(mut writer: W, block_size: usize) -> Result<Self> {
        let state = Bz3State::new(block_size)?;

        let mut header = Cursor::new([0_u8; MAGIC_NUMBER.len() + 4 /* i32 */]);
        header.write_all(MAGIC_NUMBER).unwrap();
        header.write_i32::<LE>(block_size as i32).unwrap();
        writer.write_all(header.get_ref())?;

        let buffer_size = bound(block_size);
        let buffer = vec![0; buffer_size];

        Ok(Self {
            writer,
            state,
            buffer,
            buffer_pos: 0,
            block_size,
        })
    }

    /// Compresses up to a whole block and write to `self.writer`.
    fn compress_block(&mut self) -> Result<()> {
        // self.buffer_pos as the size of data available to be compressed
        let data_size = self.buffer_pos;
        debug_assert!(data_size <= self.block_size);
        let new_size = self.state.encode_block(&mut self.buffer, data_size)?;
        self.writer.write_i32::<LE>(new_size as i32)?;
        self.writer.write_i32::<LE>(data_size as i32)?;
        self.writer.write_all(&self.buffer[..new_size])?;
        Ok(())
    }
}

impl<W> Drop for Bz3Encoder<W>
where
    W: Write,
{
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

impl<W> Write for Bz3Encoder<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut write_size = buf.len();
        let remaining_size = self.block_size - self.buffer_pos;

        if write_size > remaining_size {
            write_size = remaining_size;
        }

        self.buffer[self.buffer_pos..(self.buffer_pos + write_size)]
            .copy_from_slice(&buf[..write_size]);

        self.buffer_pos += write_size;

        if self.buffer_pos == self.block_size {
            // process the whole buffer
            // here the whole data with block_size is filled and needs to be compressed
            self.compress_block().map_err(Error::into_io_error)?;
            self.buffer_pos = 0;
        }

        Ok(write_size)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buffer_pos != 0 {
            self.compress_block().map_err(Error::into_io_error)?;
        }
        self.buffer_pos = 0;
        Ok(())
    }
}

const BLOCK_HEADER_SIZE: usize = 2 * 4 /* i32 */;

pub struct Bz3Decoder<W>
where
    W: Write,
{
    writer: W,
    state: Option<Bz3State>,
    buffer: Vec<u8>,
    buffer_pos: usize,
    header_len: usize,
    block_header_buf: [u8; BLOCK_HEADER_SIZE], /* (i32, i32) */
    block_header_buf_pos: usize,
    /// If present, the block header has been read, and this decoder now is waiting
    /// for reading the block data.
    block_header: Option<BlockHeader>,
}

struct BlockHeader {
    new_size: i32,
    read_size: i32,
}

impl BlockHeader {
    fn read_from<R: Read>(reader: &mut R) -> io::Result<Self> {
        let new_size = reader.read_i32::<LE>()?;
        let read_size = reader.read_i32::<LE>()?;
        Ok(Self {
            new_size,
            read_size,
        })
    }
}

impl<W> Bz3Decoder<W>
where
    W: Write,
{
    pub fn new(writer: W) -> Self {
        let header_len = MAGIC_NUMBER.len() + 4 /* i32 */;
        Self {
            state: None, /* can't initialize Bz3State; block size hasn't been read */
            writer,
            buffer: vec![0_u8; header_len], /* a minimum space for reading magic/header first */
            buffer_pos: 0,
            header_len,
            block_header_buf: [0_u8; 8],
            block_header_buf_pos: 0,
            block_header: None,
        }
    }

    fn initialize(&mut self) -> Result<()> {
        let mut cursor = Cursor::new(&mut self.buffer);
        let mut magic = [0_u8; MAGIC_NUMBER.len()];
        cursor.read_exact(&mut magic).unwrap();
        if &magic != MAGIC_NUMBER {
            return Err(Error::InvalidSignature);
        }
        let block_size = cursor.read_i32::<LE>().unwrap() as usize;
        // reinitialize the buffer
        let buffer_size = bound(block_size);
        self.buffer = vec![0_u8; buffer_size];
        self.state = Some(Bz3State::new(block_size)?);
        Ok(())
    }

    fn decompress_block(&mut self) -> Result<()> {
        let state = self.state.as_mut();
        let state = state.unwrap();

        let Some(block_header) = &self.block_header else {
            unreachable!()
        };
        state.decode_block(
            &mut self.buffer,
            block_header.new_size as _,
            block_header.read_size as _,
        )?;
        self.writer
            .write_all(&self.buffer[..block_header.read_size as usize])?;
        Ok(())
    }
}

impl<W> Write for Bz3Decoder<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.state.is_none() {
            // wait for the bzip3 header to initialize the decoder
            let mut write_size = buf.len();
            let needed_size = self.header_len - self.buffer_pos;
            if write_size > needed_size {
                write_size = needed_size;
            }
            self.buffer[self.buffer_pos..(self.buffer_pos + write_size)]
                .copy_from_slice(&buf[..write_size]);
            self.buffer_pos += write_size;
            if self.buffer_pos == self.header_len {
                // header prepared
                self.initialize().map_err(Error::into_io_error)?;
                self.buffer_pos = 0;
            }
            return Ok(write_size);
        }

        if self.block_header.is_none() {
            // wait for the block header
            let mut write_size = buf.len();
            let needed_size = BLOCK_HEADER_SIZE - self.block_header_buf_pos;
            if write_size > needed_size {
                write_size = needed_size;
            }
            self.block_header_buf
                [self.block_header_buf_pos..(self.block_header_buf_pos + write_size)]
                .copy_from_slice(&buf[..write_size]);

            self.block_header_buf_pos += write_size;
            if self.block_header_buf_pos == BLOCK_HEADER_SIZE {
                // resolve block header
                let mut cursor = Cursor::new(&self.block_header_buf);
                let block_header = BlockHeader::read_from(&mut cursor)?;
                self.block_header = Some(block_header);
                self.block_header_buf_pos = 0;
            }
            Ok(write_size)
        } else {
            // wait for the block data
            let block_header = self.block_header.as_ref().unwrap();
            let needed_size = block_header.new_size as usize - self.buffer_pos;
            let mut write_size = buf.len();
            if write_size > needed_size {
                write_size = needed_size;
            }
            self.buffer[self.buffer_pos..(self.buffer_pos + write_size)]
                .copy_from_slice(&buf[..write_size]);
            self.buffer_pos += write_size;
            if self.buffer_pos == block_header.new_size as usize {
                self.decompress_block().map_err(Error::into_io_error)?;
                // reset block header, wait for the next block's header
                self.block_header = None;
                self.buffer_pos = 0;
            }
            Ok(write_size)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        // this call seems to be not such meaningful
        // because in `write()`, when the block buffer is filled,
        // it immediately decompresses the block and writes to `self.reader`
        Ok(())
    }
}
