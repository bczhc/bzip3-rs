extern crate core;

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
use std::io::Read;

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

pub trait TryReadExact {
    /// Read exact data
    ///
    /// This function blocks. It reads exact data, and returns bytes it reads. The return value
    /// will always be the buffer size until it reaches EOF.
    ///
    /// When reaching EOF, the return value will be less than the size of the given buffer,
    /// or just zero.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::io::stdin;
    /// use bzip3::TryReadExact;
    ///
    /// let mut stdin = stdin();
    /// let mut buf = [0_u8; 5];
    /// loop {
    ///     let result = stdin.try_read_exact(&mut buf);
    ///     match result {
    ///         Ok(r) => {
    ///             if r == 0 {
    ///                 // EOF
    ///                 break;
    ///             }
    ///             println!("Read: {:?}", &buf[..r]);
    ///         }
    ///         Err(e) => {
    ///             eprintln!("IO error: {}", e);
    ///         }
    ///     }
    /// }
    /// ```
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
