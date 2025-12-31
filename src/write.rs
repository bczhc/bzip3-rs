//! Write-based BZip3 compressor and decompressor.

use std::cmp::min;
use std::io;
use std::io::Write;

use crate::errors::*;
use crate::{bound, BlockHeader, BlockSize, Bz3State, MAGIC_NUMBER};
use byteorder::{ByteOrder, WriteBytesExt, LE};
use static_assertions::const_assert;

#[derive(Eq, PartialEq, Copy, Clone)]
enum EncoderWritingState {
    Header,
    Blocks,
}

pub struct Bz3Encoder<W>
where
    W: Write,
{
    writer: W,
    state: Bz3State,
    buf: Vec<u8>,
    buf_pos: usize,
    block_size: usize,
    write_state: EncoderWritingState,
}

impl<W> Bz3Encoder<W>
where
    W: Write,
{
    /// Creates a new bzip3 stream encoder.
    pub fn new(mut writer: W, block_size: BlockSize) -> Self {
        let state = Bz3State::new(block_size);

        let buffer_size = bound(*block_size as _);
        let buffer = vec![0; buffer_size];

        Self {
            writer,
            state,
            buf: buffer,
            buf_pos: 0,
            block_size: *block_size as _,
            write_state: EncoderWritingState::Header,
        }
    }

    /// Compresses up to a whole block and write to `self.writer`.
    fn compress_block_and_flush(&mut self) -> io::Result<()> {
        if self.buf_pos == 0 {
            return Ok(());
        }

        let data_size = self.buf_pos;
        let new_size = self
            .state
            .encode_block(&mut self.buf, data_size)
            .map_err(Error::into_io_error)?;
        self.writer.write_i32::<LE>(new_size as i32)?;
        self.writer.write_i32::<LE>(data_size as i32)?;
        self.writer.write_all(&self.buf[..new_size])?;

        self.buf_pos = 0;
        Ok(())
    }

    fn finish_header_write(&mut self) -> io::Result<()> {
        if self.write_state != EncoderWritingState::Header {
            return Ok(());
        }

        // Write header
        let mut header = [0_u8; MAGIC_NUMBER.len() + 4 /* block size */];
        header[..MAGIC_NUMBER.len()].copy_from_slice(MAGIC_NUMBER);
        LE::write_i32(&mut header[MAGIC_NUMBER.len()..], self.block_size as _);
        self.writer.write_all(&header)?;

        self.write_state = EncoderWritingState::Blocks;
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
    fn write(&mut self, mut buf: &[u8]) -> io::Result<usize> {
        // lazy-write the header
        self.finish_header_write()?;

        // consume all `buf` data
        let all_size = buf.len();
        while !buf.is_empty() {
            let amount = min(buf.len(), self.block_size - self.buf_pos);

            self.buf[self.buf_pos..(self.buf_pos + amount)].copy_from_slice(&buf[..amount]);
            self.buf_pos += amount;

            buf = &buf[amount..];

            if self.buf_pos == self.block_size {
                // Process the whole buffer
                // here the whole data with block_size is filled and needs to be compressed.
                self.compress_block_and_flush()?
            }
        }

        Ok(all_size)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.finish_header_write()?;
        self.compress_block_and_flush()?;
        self.writer.flush()?;
        Ok(())
    }
}

#[derive(Copy, Clone)]
enum DecoderReadingState {
    FileHeader { pos: usize },
    BlockHeader { pos: usize },
    BlockData { header: BlockHeader, pos: usize },
}

pub struct Bz3Decoder<W: Write> {
    writer: W,
    state: Option<Bz3State>,
    buf: Vec<u8>,
    /// A small space for reading file and block header info.
    header_tmp: [u8; DECODER_MINIMAL_HEADER_BUF],
    reading_state: DecoderReadingState,
}

const DECODER_FILE_HEADER_SIZE: usize = MAGIC_NUMBER.len() + 4 /* block_size */;
const DECODER_BLOCK_HEADER_SIZE: usize = 2 * 4 /* new_size and read_size */;
const DECODER_MINIMAL_HEADER_BUF: usize = 9;

const_assert!(DECODER_MINIMAL_HEADER_BUF >= DECODER_FILE_HEADER_SIZE);
const_assert!(DECODER_MINIMAL_HEADER_BUF >= DECODER_BLOCK_HEADER_SIZE);

impl<W> Bz3Decoder<W>
where
    W: Write,
{
    pub const fn new(writer: W) -> Self {
        Self {
            state: None, /* can't initialize Bz3State; block size hasn't been read */
            writer,
            buf: Vec::new(),
            header_tmp: [0_u8; _],
            reading_state: DecoderReadingState::FileHeader { pos: 0 },
        }
    }

    fn decompress_block(&mut self, new_size: usize, read_size: usize) -> Result<()> {
        let state = self.state.as_mut();
        let state = state.unwrap();

        state.decode_block(&mut self.buf, new_size as _, read_size as _)?;
        self.writer.write_all(&self.buf[..read_size])?;
        Ok(())
    }

    fn init_bz3_state(&mut self) -> io::Result<()> {
        if &self.header_tmp[..MAGIC_NUMBER.len()] != MAGIC_NUMBER {
            return Err(Error::into_io_error(Error::InvalidSignature));
        }
        let block_size =
            LE::read_u32(&self.header_tmp[MAGIC_NUMBER.len()..(MAGIC_NUMBER.len() + 4)]);
        let state = Bz3State::new(
            BlockSize::new(block_size)
                .ok_or(Error::BlockSize)
                .map_err(Error::into_io_error)?,
        );
        self.state = Some(state);
        self.buf = vec![0_u8; bound(block_size as _)];
        Ok(())
    }
}

impl<W> Write for Bz3Decoder<W>
where
    W: Write,
{
    fn write(&mut self, mut buf: &[u8]) -> io::Result<usize> {
        let all_in = buf.len();

        // consume all
        while !buf.is_empty() {
            match self.reading_state {
                DecoderReadingState::FileHeader { ref mut pos } => {
                    let amount = min(buf.len(), DECODER_FILE_HEADER_SIZE - *pos);
                    self.header_tmp[*pos..(*pos + amount)].copy_from_slice(&buf[..amount]);
                    *pos += amount;
                    buf = &buf[amount..];

                    if *pos == DECODER_FILE_HEADER_SIZE {
                        self.init_bz3_state()?;
                        self.reading_state = DecoderReadingState::BlockHeader { pos: 0 };
                    }
                }

                DecoderReadingState::BlockHeader { ref mut pos } => {
                    let amount = min(buf.len(), DECODER_BLOCK_HEADER_SIZE - *pos);
                    self.header_tmp[*pos..(*pos + amount)].copy_from_slice(&buf[..amount]);
                    *pos += amount;
                    buf = &buf[amount..];

                    if *pos == DECODER_BLOCK_HEADER_SIZE {
                        self.reading_state = DecoderReadingState::BlockData {
                            header: BlockHeader::read_from_slice(&self.header_tmp[..]),
                            pos: 0,
                        };
                    }
                }

                DecoderReadingState::BlockData {
                    header:
                        BlockHeader {
                            new_size,
                            read_size,
                        },
                    ref mut pos,
                } => {
                    let amount = min(buf.len(), new_size - *pos);
                    self.buf[*pos..(*pos + amount)].copy_from_slice(&buf[..amount]);
                    *pos += amount;
                    buf = &buf[amount..];

                    if *pos == new_size {
                        // decompress a block
                        self.decompress_block(new_size, read_size)
                            .map_err(Error::into_io_error)?;

                        // prepare to read the next block
                        self.reading_state = DecoderReadingState::BlockHeader { pos: 0 }
                    }
                }
            }
        }

        Ok(all_in)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl<W: Write> Bz3Decoder<W> {
    /// Finish the decoder.
    ///
    /// It's better to finish the decoder by this call, which flushes the data and provides
    /// insufficient data detection. Though it's also just fine to simply drop(the object),
    /// but will ignore the data which doesn't trigger a new block decompression. In this
    /// case, the decompressed data stream is considered truncated, and we can't notice this if
    /// only using a drop call.
    pub fn finish(mut self) -> io::Result<()> {
        match self.reading_state {
            DecoderReadingState::FileHeader { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Insufficient data to read for file header",
                ));
            }
            DecoderReadingState::BlockData { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Insufficient data to decode a full block",
                ));
            }
            DecoderReadingState::BlockHeader { pos: x } if x != 0 => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Insufficient data to read for block header",
                ))
            }
            _ => {}
        }
        self.flush()?;
        Ok(())
    }
}

impl<W: Write> Drop for Bz3Decoder<W> {
    fn drop(&mut self) {
        let _r = self.flush();
    }
}
