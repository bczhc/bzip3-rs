//! Write-based BZip3 compressor and decompressor.

use std::ffi::CStr;
use std::io::{Cursor, Read, Write};
use std::mem::{size_of, MaybeUninit};
use std::ptr::null_mut;
use std::{io, mem};

use byteorder::{ReadBytesExt, WriteBytesExt, LE};

use libbzip3_sys::{bz3_decode_block, bz3_encode_block, bz3_free, bz3_state, bz3_strerror};

use crate::errors::*;
use crate::{
    check_block_size, create_bz3_state, init_buffer, transmute_uninitialized_buffer,
    uninit_copy_from_slice, MAGIC_NUMBER,
};

pub struct Bz3Encoder<'a, W>
where
    W: Write,
{
    writer: &'a mut W,
    state: *mut bz3_state,
    buffer: Vec<MaybeUninit<u8>>,
    buffer_pos: usize,
    block_size: usize,
}

impl<'a, W> Bz3Encoder<'a, W>
where
    W: Write,
{
    /// The block size must be between 65kiB and 511MiB.
    ///
    /// # Errors
    ///
    /// This returns [`Error::BlockSize`] if the block size is invalid.
    pub fn new(writer: &'a mut W, block_size: usize) -> Result<Self> {
        if check_block_size(block_size).is_err() {
            return Err(Error::BlockSize);
        }
        let block_size = block_size as i32;

        let mut header = Cursor::new([0_u8; MAGIC_NUMBER.len() + size_of::<i32>()]);
        header.write_all(MAGIC_NUMBER).unwrap();
        header.write_i32::<LE>(block_size).unwrap();
        writer.write_all(header.get_ref())?;

        let buffer_size = block_size + block_size / 50 + 32;
        let buffer = init_buffer(buffer_size as usize);

        let state = create_bz3_state(block_size);

        Ok(Self {
            writer,
            state,
            buffer,
            buffer_pos: 0,
            block_size: block_size as usize,
        })
    }

    fn compress_block(&mut self) -> Result<()> {
        // self.buffer_pos as the size of data available to be compressed
        let data_size = self.buffer_pos;
        debug_assert!(data_size <= self.block_size);
        unsafe {
            let new_size = bz3_encode_block(
                self.state,
                transmute_uninitialized_buffer(&mut self.buffer).as_mut_ptr(),
                data_size as i32,
            );
            if new_size == -1 {
                return Err(Error::ProcessBlock(
                    CStr::from_ptr(bz3_strerror(self.state))
                        .to_string_lossy()
                        .into(),
                ));
            }

            self.writer.write_i32::<LE>(new_size)?;
            self.writer.write_i32::<LE>(data_size as i32)?;
            self.writer.write_all(
                &mem::transmute::<_, &[u8]>(self.buffer.as_slice())[..new_size as usize],
            )?;
        }
        Ok(())
    }
}

impl<'a, W> Drop for Bz3Encoder<'a, W>
where
    W: Write,
{
    fn drop(&mut self) {
        let _ = self.flush();
        unsafe {
            bz3_free(self.state);
        }
    }
}

impl<'a, W> Write for Bz3Encoder<'a, W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut write_size = buf.len();
        let remaining_size = self.block_size - self.buffer_pos;

        if write_size > remaining_size {
            write_size = remaining_size;
        }

        uninit_copy_from_slice(
            &buf[..write_size],
            &mut self.buffer[self.buffer_pos..(self.buffer_pos + write_size)],
        );

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
        self.compress_block().map_err(Error::into_io_error)?;
        self.buffer_pos = 0;
        Ok(())
    }
}

const BLOCK_HEADER_SIZE: usize = 2 * size_of::<i32>();

pub struct Bz3Decoder<'a, W>
where
    W: Write,
{
    writer: &'a mut W,
    state: *mut bz3_state,
    buffer: Vec<MaybeUninit<u8>>,
    buffer_pos: usize,
    header_len: usize,
    block_header_buf: [u8; BLOCK_HEADER_SIZE], /* (i32, i32) */
    block_header_buf_pos: usize,
    /// if present, the block header has been read, and this decoder now is waiting
    /// for reading the block data
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

impl<'a, W> Bz3Decoder<'a, W>
where
    W: Write,
{
    pub fn new(writer: &'a mut W) -> Self {
        let header_len = MAGIC_NUMBER.len() + size_of::<i32>();
        Self {
            state: null_mut(), /* here can't get the block size */
            writer,
            buffer: init_buffer(header_len), /* need header data to initialize first */
            buffer_pos: 0,
            header_len,
            block_header_buf: [0_u8; 8],
            block_header_buf_pos: 0,
            block_header: None,
        }
    }

    fn initialize(&mut self) -> Result<()> {
        let buffer = unsafe { transmute_uninitialized_buffer(&mut self.buffer) };
        let mut cursor = Cursor::new(buffer);
        let mut magic = [0_u8; MAGIC_NUMBER.len()];
        cursor.read_exact(&mut magic).unwrap();
        if &magic != MAGIC_NUMBER {
            return Err(Error::InvalidSignature);
        }
        let block_size = cursor.read_i32::<LE>().unwrap();
        // reinitialize the buffer
        let buffer_size = block_size + block_size / 50 + 32;
        self.buffer = init_buffer(buffer_size as usize);
        self.state = create_bz3_state(block_size);
        Ok(())
    }

    fn decompress_block(&mut self) -> Result<()> {
        let Some(block_header) = &self.block_header else { unreachable!() };
        unsafe {
            let buffer = transmute_uninitialized_buffer(&mut self.buffer);
            let result = bz3_decode_block(
                self.state,
                buffer.as_mut_ptr(),
                block_header.new_size,
                block_header.read_size,
            );
            if result == -1 {
                return Err(Error::ProcessBlock(
                    CStr::from_ptr(bz3_strerror(self.state))
                        .to_string_lossy()
                        .into(),
                ));
            }
            self.writer.write_all(
                &mem::transmute::<_, &[u8]>(self.buffer.as_slice())
                    [..block_header.read_size as usize],
            )?;
        }
        Ok(())
    }
}

impl<'a, W> Write for Bz3Decoder<'a, W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.state.is_null() {
            // wait for the bzip3 header to initialize the decoder
            let mut write_size = buf.len();
            let needed_size = self.header_len - self.buffer_pos;
            if write_size > needed_size {
                write_size = needed_size;
            }
            uninit_copy_from_slice(
                &buf[..write_size],
                &mut self.buffer[self.buffer_pos..(self.buffer_pos + write_size)],
            );
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
            uninit_copy_from_slice(
                &buf[..write_size],
                &mut self.buffer[self.buffer_pos..(self.buffer_pos + write_size)],
            );
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

impl<'a, W> Drop for Bz3Decoder<'a, W>
where
    W: Write,
{
    fn drop(&mut self) {
        unsafe {
            bz3_free(self.state);
        }
    }
}
