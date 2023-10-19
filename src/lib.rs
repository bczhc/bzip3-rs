//! BZip3-rs
//! ----
//! BZip3 compression for Rust.
//!
//! # BZip3 file structure:
//!
//! \[ magic number (\[u8; 5\]) | block size (i32) | block1 | block2 | blockN... \]
//!
//! Structure of each block:
//! \[ new size (i32) | read size (i32) | data \]
//!
//! Due to the naming from the original bzip3 library,
//! `new size` indicates the data size after compression, and `read size` indicates the original data size.
//!
//! # Examples
//!
//! ```
//! use std::io::Read;
//! use bzip3::read::{Bz3Decoder, Bz3Encoder};
//!
//! let data = "hello, world".as_bytes();
//! let block_size = 100 * 1024; // 100 kiB
//!
//! let mut compressor = Bz3Encoder::new(data, block_size).unwrap();
//! let mut decompressor = Bz3Decoder::new(&mut compressor).unwrap();
//!
//! let mut contents = String::new();
//! decompressor.read_to_string(&mut contents).unwrap();
//! assert_eq!(contents, "hello, world");
//! ```
extern crate core;

use std::{ffi::CStr, io::Read};

use bytesize::ByteSize;

use libbzip3_sys::{bz3_free, bz3_new, bz3_state, bz3_strerror};

pub mod errors;
pub mod read;
pub mod stream;
pub mod write;
pub use errors::{Error, Result};

/// The signature of a bzip3 file.
pub const MAGIC_NUMBER: &[u8; 5] = b"BZ3v1";

pub(crate) trait TryReadExact {
    /// Read exact data
    ///
    /// This function blocks. It reads exact data, and returns bytes it reads. The return value
    /// will always be the buffer size until it reaches EOF.
    ///
    /// When reaching EOF, the return value will be less than the size of the given buffer,
    /// or just zero.
    ///
    /// This simulates C function `fread`.
    fn try_read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
}

impl<R> TryReadExact for R
where
    R: Read,
{
    fn try_read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut read = 0_usize;
        loop {
            let result = self.read(&mut buf[read..]);
            match result {
                Ok(r) => {
                    if r == 0 {
                        return Ok(read);
                    }
                    read += r;
                    if read == buf.len() {
                        return Ok(read);
                    }
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }
}

pub fn version() -> &'static str {
    unsafe { CStr::from_ptr(libbzip3_sys::bz3_version()) }
        .to_str()
        .expect("Invalid UTF-8")
}

pub struct Bz3State {
    raw: *mut bz3_state,
}

impl Bz3State {
    pub fn from_raw(state: *mut bz3_state) -> Bz3State {
        Bz3State { raw: state }
    }

    pub fn new(block_size: usize) -> Result<Self> {
        if block_size < ByteSize::kib(65).0 as usize || block_size > ByteSize::mib(511).0 as usize {
            return Err(Error::BlockSize);
        }

        unsafe {
            let state = bz3_new(block_size as i32);
            if state.is_null() {
                panic!("Allocation fails");
            }
            Ok(Self::from_raw(state))
        }
    }

    #[inline]
    pub fn as_raw(&mut self) -> *mut bz3_state {
        self.raw
    }

    pub fn error(&mut self) -> &'static str {
        unsafe {
            // in bzip3 source code, this returns static string literals
            CStr::from_ptr(bz3_strerror(self.raw))
                .to_str()
                .expect("Invalid UTF-8")
        }
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
unsafe impl Sync for Bz3State {}
