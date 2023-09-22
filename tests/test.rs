extern crate core;

use std::fmt::Write as _;
use std::io::{self, Cursor, Read, Write};

use bytesize::ByteSize;
use rand::{thread_rng, RngCore};
use regex::Regex;

use bzip3::{read, write};

const KB: usize = 1024;

#[test]
fn test() {
    let test_size_array = [
        0_usize,
        1,
        2,
        3,
        4,
        5,
        8191,
        8192,
        8193,
        1048576,
        ByteSize::mib(10).0 as usize,
        ByteSize::mib(30).0 as usize,
    ];
    let block_size_array = [
        ByteSize::kib(65),
        ByteSize::kib(100),
        ByteSize::mib(1),
        ByteSize::mib(5),
        ByteSize::mib(10),
    ]
    .map(|x| x.0 as usize);

    rayon::scope(|scope| {
        for data_size in test_size_array {
            for block_size in block_size_array {
                scope.spawn(move |_| {
                    println!("Test read-based: {:?}", (data_size, block_size));
                    test_read_based(data_size, block_size);
                });
                scope.spawn(move |_| {
                    println!("Test write-based: {:?}", (data_size, block_size));
                    test_write_based(data_size, block_size);
                });
            }
        }
    });
}

#[test]
fn version() {
    let version = bzip3::version();
    assert!(Regex::new(r#"^[0-9]+\.[0-9]+\.[0-9]+$"#)
        .unwrap()
        .is_match(version));
}

#[test]
fn test_compressing_and_decompressing_small_input() {
    // Input to be compressed and decompressed
    let input: &[u8] = &[1, 2, 3];

    let compressed = {
        let mut output = vec![];
        io::copy(
            &mut &*input,
            &mut write::Bz3Encoder::new(&mut output, 100 * KB).unwrap(),
        )
        .unwrap();

        output
    };

    let decompressed = {
        let mut output = vec![];
        io::copy(
            &mut read::Bz3Decoder::new(compressed.as_slice()).unwrap(),
            &mut output,
        )
        .unwrap();

        output
    };

    assert_eq!(input, decompressed);

    // Input to be compressed and decompressed
    let input: &[u8] = &[1, 2, 3];

    let compressed = {
        let mut output = vec![];
        io::copy(
            &mut read::Bz3Encoder::new(input, 100 * KB).unwrap(),
            &mut output,
        )
        .unwrap();

        output
    };

    let decompressed = {
        let mut output = vec![];
        io::copy(&mut &*compressed, &mut write::Bz3Decoder::new(&mut output)).unwrap();

        output
    };

    assert_eq!(input, decompressed);
}

#[test]
fn test_chained_encoders_and_decoders_with_single_block() {
    // 100kb gets shrunk down to 22kb-24kb, so it fits in a single 70kb block
    let input = generate_deterministic_data(100 * KB);
    let mut reader = create_encoder_chain(input.as_slice(), 10, 70 * KB);

    let mut output = vec![];
    let mut writer = create_decoder_chain(10, &mut output);

    io::copy(&mut reader, &mut writer).unwrap();

    drop(writer);
    assert_eq!(input, output);
}

#[test]
fn test_chained_encoders_and_decoders_with_multiple_blocks() {
    // 1400kb gets shrunk down to 163kb-174kb, only fits in multiple blocks of 70kb
    let input = generate_deterministic_data(1400 * KB);
    let mut reader = create_encoder_chain(input.as_slice(), 10, 70 * KB);

    let mut output = vec![];
    let mut writer = create_decoder_chain(10, &mut output);

    io::copy(&mut reader, &mut writer).unwrap();

    drop(writer);
    assert_eq!(input, output);
}

fn create_encoder_chain<'a>(
    reader: impl Read + 'a,
    chain_size: usize,
    block_size: usize,
) -> Box<dyn Read + 'a> {
    assert!(chain_size >= 1);
    let mut encoder: Box<dyn Read> = Box::new(read::Bz3Encoder::new(reader, block_size).unwrap());

    for _ in 1..chain_size {
        encoder = Box::new(read::Bz3Encoder::new(encoder, block_size).unwrap());
    }

    encoder
}

fn create_decoder_chain<'a>(chain_size: usize, reader: impl Write + 'a) -> Box<dyn Write + 'a> {
    assert!(chain_size >= 1);
    let mut decoder: Box<dyn Write> = Box::new(write::Bz3Decoder::new(reader));

    for _ in 1..chain_size {
        decoder = Box::new(write::Bz3Decoder::new(decoder));
    }

    decoder
}

fn test_write_based(data_size: usize, block_size: usize) {
    let data = generate_random_data(data_size);
    let mut reader = Cursor::new(&data);
    let mut writer = Cursor::new(Vec::new());

    let mut encoder = write::Bz3Encoder::new(&mut writer, block_size).unwrap();
    io::copy(&mut reader, &mut encoder).unwrap();
    drop(encoder);

    let compressed = writer.into_inner();

    let mut reader = Cursor::new(compressed);
    let mut writer = Cursor::new(Vec::new());

    let mut decoder = write::Bz3Decoder::new(&mut writer);
    io::copy(&mut reader, &mut decoder).unwrap();
    drop(decoder);

    assert_eq!(writer.into_inner(), data);
}

fn test_read_based(data_size: usize, block_size: usize) {
    let mut data = generate_random_data(data_size);

    let mut compressed = Cursor::new(Vec::new());
    {
        let mut reader = Cursor::new(&mut data);
        let mut encoder = read::Bz3Encoder::new(&mut reader, block_size).unwrap();
        io::copy(&mut encoder, &mut compressed).unwrap();
    }
    let compressed = compressed.into_inner();

    let mut uncompressed = Cursor::new(Vec::new());
    {
        let mut reader = Cursor::new(compressed);
        let mut decoder = read::Bz3Decoder::new(&mut reader).unwrap();
        assert_eq!(decoder.block_size(), block_size);
        io::copy(&mut decoder, &mut uncompressed).unwrap();
    }

    assert_eq!(uncompressed.get_ref().as_slice(), data.as_slice());
}

fn generate_random_data(size: usize) -> Vec<u8> {
    let mut rng = thread_rng();

    let mut data = vec![0_u8; size];
    rng.fill_bytes(&mut data);
    data
}

fn generate_deterministic_data(size: usize) -> Vec<u8> {
    let mut string = String::with_capacity(size + 20);

    for number in 0..u64::MAX {
        if string.len() > size {
            break;
        }
        write!(string, "{number}").unwrap();
    }

    string.truncate(size);
    string.into_bytes()
}
