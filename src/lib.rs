extern crate core;

use std::mem;
use std::mem::MaybeUninit;
use std::{ffi::CStr, io::Read};

/// # BZip3-rs
///
/// BZip3 file structure:
///
/// \[ magic number (\[u8; 5\]) | block size (i32) | block1 | block2 | blockN... \]
///
/// Structure of each block:
/// \[ new size (i32) | read size (i32) | data \]
///
/// `new size` is the data size after compression, and `read size` is the original data size.
use bytesize::ByteSize;

use libbzip3_sys::{bz3_new, bz3_state};

pub mod errors;
pub mod read;
pub mod stream;
pub mod write;

/// The signature of a bzip3 file.
pub const MAGIC_NUMBER: &[u8; 5] = b"BZ3v1";

pub(crate) fn check_block_size(block_size: usize) -> Result<(), ()> {
    if block_size < ByteSize::kib(65).0 as usize || block_size > ByteSize::mib(511).0 as usize {
        Err(())
    } else {
        Ok(())
    }
}

pub(crate) trait TryReadExact {
    /// Read exact data
    ///
    /// This function blocks. It reads exact data, and returns bytes it reads. The return value
    /// will always be the buffer size until it reaches EOF.
    ///
    /// When reaching EOF, the return value will be less than the size of the given buffer,
    /// or just zero.
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

fn init_buffer(size: usize) -> Vec<MaybeUninit<u8>> {
    let mut buffer = Vec::<MaybeUninit<u8>>::with_capacity(size);
    unsafe {
        buffer.set_len(size);
    }
    buffer
}

fn create_bz3_state(block_size: i32) -> *mut bz3_state {
    unsafe {
        let state = bz3_new(block_size);
        if state.is_null() {
            panic!("Allocation fails");
        }
        state
    }
}

#[inline(always)]
unsafe fn transmute_uninitialized_buffer(buffer: &mut [MaybeUninit<u8>]) -> &mut [u8] {
    mem::transmute(buffer)
}

fn uninit_copy_from_slice(src: &[u8], dst: &mut [MaybeUninit<u8>]) {
    unsafe {
        let transmute: &[MaybeUninit<u8>] = mem::transmute(src);
        dst.copy_from_slice(transmute);
    }
}

pub fn version() -> &'static str {
    unsafe { CStr::from_ptr(libbzip3_sys::bz3_version()) }
        .to_str()
        .expect("Invalid UTF-8")
}
