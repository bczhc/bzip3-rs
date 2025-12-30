use bytesize::MIB;
use bzip3::stream::{parallel_compress, parallel_decompress};
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
        parallel_decompress(
            BufReader::new(stdin().lock()),
            BufWriter::new(stdout().lock()),
        )?;
    } else {
        parallel_compress(
            BufReader::new(stdin().lock()),
            BufWriter::new(stdout().lock()),
            block_size_bytes,
        )?;
    }
    Ok(())
}
