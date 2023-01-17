use std::ffi::CStr;
use std::io;
use std::io::{stdin, stdout, BufWriter};
use std::str::FromStr;

use bytesize::ByteSize;
use clap::{Arg, ArgAction, Command};

use bzip3::write;

fn main() -> anyhow::Result<()> {
    let version = unsafe { CStr::from_ptr(libbzip3_sys::bz3_version()) }
        .to_str()
        .unwrap();
    eprintln!("Bzip3 version: {}", version);

    let matches = Command::new("bzip3")
        .arg(
            Arg::new("block-size")
                .short('b')
                .long("block-size")
                .default_value("1MiB")
                .conflicts_with("decompress"),
        )
        .arg(
            Arg::new("decompress")
                .short('d')
                .long("decompress")
                .required(false)
                .action(ArgAction::SetTrue),
        )
        .get_matches();

    let decompress = matches.get_flag("decompress");

    let mut writer = BufWriter::new(stdout().lock());
    let mut reader = stdin().lock();

    if decompress {
        let mut decoder = write::Bz3Decoder::new(&mut writer);
        io::copy(&mut reader, &mut decoder).unwrap();
    } else {
        let block_size = matches.get_one::<String>("block-size").unwrap();
        let block_size = ByteSize::from_str(block_size).unwrap().0 as usize;
        let mut encoder = write::Bz3Encoder::new(&mut writer, block_size).unwrap();
        io::copy(&mut reader, &mut encoder).unwrap();
    }
    Ok(())
}
