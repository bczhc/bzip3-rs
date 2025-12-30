use bytesize::MIB;
use bzip3::stream::{compress, decompress};
use bzip3::{BlockSize, Error};
use clap::Parser;
use std::io::{stdin, stdout, BufReader, BufWriter};

const DEFAULT_BLOCK_SIZE_MIB: usize = 16_usize;

#[derive(Parser, Debug)]
#[command(author, version, about = "BZip3 Parallel CLI", long_about = None)]
/// This produces results identical to the C version bzip3(1).
struct Args {
    /// Decompression mode.
    #[arg(short, long)]
    decompress: bool,

    /// Block size in megabytes.
    #[arg(short, long, default_value_t = DEFAULT_BLOCK_SIZE_MIB)]
    block_size: usize,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let block_size_bytes = args.block_size * MIB as usize;

    if args.decompress {
        decompress(
            BufReader::new(stdin().lock()),
            BufWriter::new(stdout().lock()),
            None,
        )?;
    } else {
        compress(
            BufReader::new(stdin().lock()),
            BufWriter::new(stdout().lock()),
            BlockSize::new(block_size_bytes as _).ok_or(Error::BlockSize)?,
            None,
        )?;
    }
    Ok(())
}
