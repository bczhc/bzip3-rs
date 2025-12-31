#![warn(clippy::all, clippy::nursery)]

//! BZip3-rs
//! ----
//! BZip3 compression for Rust.
//!
//! # BZip3 file structure:
//!
//! \[ magic number (\[u8; 5\]) | block size (u32) | block 1 | block 2 | ... | block N \]
//!
//! Structure of each block:
//! \[ new size (u32) | read size (u32) | data \]
//!
//! Due to the naming from the original bzip3 library,
//! `new size` indicates the data size after compression, and `read size` indicates the original
//! data size.
//!
//! # Examples
//!
//! ## Use read/write-based wrapper
//!
//! ```
//! use std::io::Read;
//! use bzip3::BlockSize;
//! use bzip3::read::{Bz3Decoder, Bz3Encoder};
//!
//! let data = "hello, world".as_bytes();
//! let block_size = BlockSize::kib(100).unwrap();
//!
//! let mut compressor = Bz3Encoder::new(data, block_size).unwrap();
//! let mut decompressor = Bz3Decoder::new(&mut compressor).unwrap();
//!
//! let mut contents = String::new();
//! decompressor.read_to_string(&mut contents).unwrap();
//! assert_eq!(contents, "hello, world");
//! ```
//!
//! ## Use stream processor
//!
//! ```no_run
//! use std::io::{stdin, stdout};
//! use bzip3::{stream, BlockSize};
//!
//! let reader = stdin().lock();
//! let writer = stdout().lock();
//!
//! stream::compress(reader, writer, BlockSize::DEFAULT, Some(8 /* 8 threads */)).unwrap();
//! ```
extern crate core;

use byteorder::{ByteOrder, LE};
use bytesize::{KIB, MIB};
use libbzip3_sys::{
    bz3_bound, bz3_decode_block, bz3_encode_block, bz3_free, bz3_new, bz3_state, bz3_strerror,
    BZ3_ERR_DATA_SIZE_TOO_SMALL,
};
use std::io::ErrorKind;
use std::ops::Deref;
use std::{ffi::CStr, io::Read};

pub mod errors;
pub mod read;
pub mod stream;
pub mod write;
pub use errors::{Error, Result};

/// Signature of a bzip3 file.
pub const MAGIC_NUMBER: &[u8; 5] = b"BZ3v1";

/// A block size wrapper with range checked.
///
/// `block_size` in the C library, `size_t`, `uint32_t` and `int32_t`
/// are all used inconsistently. So to pass the value to them, just do `as _`.
#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct BlockSize(u32);

impl Deref for BlockSize {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub const BLOCK_SIZE_MIN: u32 = 65 * KIB as u32;

pub const BLOCK_SIZE_MAX: u32 = 511 * MIB as u32;

impl BlockSize {
    /// Minimum block size.
    pub const MIN: Self = Self(BLOCK_SIZE_MIN);

    /// Maximum block size.
    pub const MAX: Self = Self(BLOCK_SIZE_MAX);

    /// Default block size used in bzip3 CLI
    pub const DEFAULT: Self = Self(16 * MIB as u32);

    const fn bytes(size: u32) -> Option<Self> {
        if !matches!(size, BLOCK_SIZE_MIN..=BLOCK_SIZE_MAX) {
            return None;
        }
        Some(Self(size))
    }

    pub const fn new(size: u32) -> Option<Self> {
        Self::bytes(size)
    }

    pub const fn kib(kib: u32) -> Option<Self> {
        Self::bytes(kib.saturating_mul(KIB as u32))
    }

    pub const fn mib(mib: u32) -> Option<Self> {
        Self::bytes(mib.saturating_mul(MIB as u32))
    }
}

pub(crate) trait ReadExt {
    /// Reads and fill `buf`.
    ///
    /// This function behaves like [`Read::read_exact`] but gives the size already read on
    /// EOF reached. That is, the return value will always be `buf.len()` while EOF not reached.
    ///
    /// This simulates C function `fread`.
    fn read_full(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
}

impl<R> ReadExt for R
where
    R: Read,
{
    fn read_full(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut read = 0_usize;
        while read < buf.len() {
            match self.read(&mut buf[read..]) {
                Ok(0) => break, /* EOF */
                Ok(r) => read += r,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(read)
    }
}

/// Version of the underlying bzip3 library.
pub fn version() -> &'static str {
    // SAFETY: `bz3_version` from the C lib is supposed to return a static string.
    unsafe { CStr::from_ptr(libbzip3_sys::bz3_version()) }
        .to_str()
        .expect("Invalid UTF-8")
}

// TODO: It may be a const function?
/// Returns the recommended output buffer size for the compression function.
pub fn bound(input: usize) -> usize {
    unsafe { bz3_bound(input) }
}

/// Wrapper for the raw Bz3State.
pub struct Bz3State {
    block_size: u32,
    raw: *mut bz3_state,
}

impl Bz3State {
    /// Creates a new Bz3State.
    pub fn new(block_size: BlockSize) -> Self {
        unsafe {
            let state = bz3_new(*block_size as _);
            if state.is_null() {
                // This is fatal. Just panic.
                panic!("Allocation fails");
            }
            Self {
                raw: state,
                block_size: *block_size as _,
            }
        }
    }

    #[inline]
    pub const fn as_raw(&mut self) -> *mut bz3_state {
        self.raw
    }

    pub fn error(&mut self) -> &'static str {
        // SAFETY: in bzip3 source code, this returns static string literals.
        unsafe {
            CStr::from_ptr(bz3_strerror(self.raw))
                .to_str()
                .expect("Invalid UTF-8")
        }
    }

    fn check_block_process_code(&mut self, code: i32) -> Result<()> {
        // TODO: more errors
        if code == -1 {
            return Err(Error::ProcessBlock(self.error().into()));
        }
        if code == BZ3_ERR_DATA_SIZE_TOO_SMALL {
            return Err(Error::BlockSize);
        }
        Ok(())
    }

    /// Compresses a block in-place.
    ///
    ///
    /// - `input_size` is the original data size before compression. It must not exceed the block
    ///   size associated with the state.
    /// - `buf` must be able to hold the data after compression. That is,
    ///   `buf.len() >= bound(input_size)` must be required, in some cases where the compressed
    ///   data is larger than the original one.
    ///
    /// Returns the size of data written to `buf`.
    pub fn encode_block(&mut self, buf: &mut [u8], input_size: usize) -> Result<usize> {
        debug_assert!(input_size <= self.block_size as _);
        debug_assert!(buf.len() >= bound(input_size));
        let result = unsafe { bz3_encode_block(self.raw, buf.as_mut_ptr(), input_size as _) };
        self.check_block_process_code(result)?;

        Ok(result as usize)
    }

    /// Decompresses a block in-place.
    ///
    /// `buf` must be able to hold both compressed and original data.
    ///
    /// The original doc states as below:
    ///
    ///  * `buffer` must be able to hold at least `bz3_bound(orig_size)` bytes
    ///  * in order to ensure decompression will succeed for all possible bzip3 blocks.
    ///  *
    ///  * In most (but not all) cases, `orig_size` should usually be sufficient.
    ///  * If it is not sufficient, you must allocate a buffer of size `bz3_bound(orig_size)` temporarily.
    ///  *
    ///  * If `buffer_size` is too small, `BZ3_ERR_DATA_SIZE_TOO_SMALL` will be returned.
    ///  * The size must not exceed the block size associated with the state.
    pub fn decode_block(
        &mut self,
        buf: &mut [u8],
        compressed_size: usize,
        original_size: usize,
    ) -> Result<()> {
        debug_assert!(buf.len() >= original_size && buf.len() >= compressed_size);
        debug_assert!(compressed_size <= i32::MAX as usize);
        let result = unsafe {
            bz3_decode_block(
                self.raw,
                buf.as_mut_ptr(),
                buf.len(),
                compressed_size as _,
                original_size as _,
            )
        };
        self.check_block_process_code(result)?;
        if result as usize != original_size {
            return Err(Error::ProcessBlock(
                "Data not match the origin size after decompression".into(),
            ));
        }
        Ok(())
    }
}

impl Drop for Bz3State {
    fn drop(&mut self) {
        unsafe {
            bz3_free(self.raw);
        }
    }
}

unsafe impl Send for Bz3State {}

#[derive(Copy, Clone)]
pub struct BlockHeader {
    pub new_size: usize,
    pub read_size: usize,
}

impl BlockHeader {
    fn read_from_slice(value: &[u8]) -> Self {
        let new_size = LE::read_i32(&value[..4]);
        let read_size = LE::read_i32(&value[4..8]);
        Self {
            new_size: new_size as _,
            read_size: read_size as _,
        }
    }

    pub fn to_bytes(self) -> [u8; 8] {
        let mut buf = [0_u8; 8];
        LE::write_i32(&mut buf[..4], self.new_size as _);
        LE::write_i32(&mut buf[4..8], self.read_size as _);
        buf
    }
}

#[cfg(test)]
pub mod test {
    use crate as bzip3;
    use crate::{bound, BlockSize, Bz3State};
    use regex::Regex;

    #[test]
    fn version() {
        let version = bzip3::version();
        assert!(Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+$")
            .unwrap()
            .is_match(version));
    }

    #[test]
    fn encode_decode_raw() {
        let data = b"hello, world";
        let mut buf = vec![0_u8; bound(data.len())];
        buf[..data.len()].copy_from_slice(data);
        let mut bs = Bz3State::new(BlockSize::mib(1).unwrap());
        let compressed_size = bs.encode_block(&mut buf, data.len()).unwrap();

        bs.decode_block(&mut buf, compressed_size, data.len())
            .unwrap();
        let decompressed = &buf[..data.len()];
        assert_eq!(decompressed, &data[..]);
    }
}
