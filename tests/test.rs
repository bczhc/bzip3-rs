#![warn(clippy::all, clippy::nursery)]

extern crate core;

use bytesize::{mib, ByteSize, MIB};
use hex_literal::hex;
use rand::{rng, RngCore};
use std::fmt::Write as _;
use std::io::{self, Cursor, Read, Write};

use bzip3::stream::{compress, decompress};
use bzip3::{read, write, BlockSize, BLOCK_SIZE_MAX, BLOCK_SIZE_MIN, MAGIC_NUMBER};

const KB: usize = 1024;

pub const HEADER_BS16MIB: &[u8; 9] = b"BZ3v1\0\0\0\x01";

pub const EMPTY_DATA_BLOCK: [u8; 16] = hex!("0800 0000 0000 0000 0100 0000 ffff ffff");

#[test]
fn test_compress_and_decompress() {
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
fn test_compressing_and_decompressing_small_input() {
    // Input to be compressed and decompressed
    let input: &[u8] = &[1, 2, 3];

    let compressed = {
        let mut output = vec![];
        io::copy(
            &mut &*input,
            &mut write::Bz3Encoder::new(&mut output, BlockSize::kib(100).unwrap()),
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
            &mut read::Bz3Encoder::new(input, BlockSize::kib(100).unwrap()).unwrap(),
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

#[test]
fn avoid_creating_empty_blocks_by_flush_calls() {
    let mut buf = Vec::new();
    let mut encoder = write::Bz3Encoder::new(&mut buf, BlockSize::mib(16).unwrap());
    encoder.flush().unwrap();
    encoder.flush().unwrap();
    encoder.flush().unwrap();
    encoder.flush().unwrap();
    drop(encoder);
    assert_eq!(buf, {
        let mut vec = Vec::from(*MAGIC_NUMBER);
        vec.extend_from_slice(&hex!("00000001") /* 16MiB in LE */);
        vec
    });

    let mut buf = Vec::new();
    let mut encoder = write::Bz3Encoder::new(&mut buf, BlockSize::mib(16).unwrap());
    encoder.flush().unwrap();
    encoder.write_all(b"hello").unwrap();
    drop(encoder);
    assert_eq!(find_subsequence(&buf, &EMPTY_DATA_BLOCK), None);
}

#[test]
fn decode_empty_blocks() {
    let block_size = hex!("0000 0001");
    let data_block = hex!("0d00000005000000d5a212e7ffffffff68656c6c6f");
    let mut archive: Vec<u8> = Vec::new();
    archive.write_all(MAGIC_NUMBER).unwrap();
    archive.write_all(&block_size).unwrap();
    for _ in 0..10 {
        archive.write_all(&EMPTY_DATA_BLOCK).unwrap();
    }
    archive.write_all(&data_block).unwrap();
    archive.write_all(&EMPTY_DATA_BLOCK).unwrap();
    archive.write_all(&data_block).unwrap();

    // read-based
    let decoder = read::Bz3Decoder::new(archive.as_slice()).unwrap();
    assert_eq!(io::read_to_string(decoder).unwrap(), "hellohello");

    // write-based
    let mut writer = Cursor::new(Vec::new());
    let mut decoder = write::Bz3Decoder::new(&mut writer);
    io::copy(&mut Cursor::new(archive), &mut decoder).unwrap();
    drop(decoder);
    assert_eq!(
        String::from_utf8(writer.into_inner()).unwrap(),
        "hellohello"
    );
}

fn create_encoder_chain<'a>(
    reader: impl Read + 'a,
    chain_size: usize,
    block_size: usize,
) -> Box<dyn Read + 'a> {
    assert!(chain_size >= 1);
    let mut encoder: Box<dyn Read> =
        Box::new(read::Bz3Encoder::new(reader, BlockSize::new(block_size as _).unwrap()).unwrap());

    for _ in 1..chain_size {
        encoder = Box::new(
            read::Bz3Encoder::new(encoder, BlockSize::new(block_size as _).unwrap()).unwrap(),
        );
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

    let mut encoder = write::Bz3Encoder::new(&mut writer, BlockSize::new(block_size as _).unwrap());
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
        let mut encoder =
            read::Bz3Encoder::new(&mut reader, BlockSize::new(block_size as _).unwrap()).unwrap();
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
    let mut rng = rng();

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

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[test]
fn block_size() {
    assert!(BlockSize::new(BLOCK_SIZE_MIN).is_some());
    assert!(BlockSize::new(BLOCK_SIZE_MAX).is_some());
    assert!(BlockSize::new(BLOCK_SIZE_MIN - 1).is_none());
    assert!(BlockSize::new(BLOCK_SIZE_MAX + 1).is_none());
}

#[test]
fn test_parallel() {
    let sizes = [
        0,
        10,
        1024,
        BLOCK_SIZE_MIN,
        BLOCK_SIZE_MIN + 1234,
        BLOCK_SIZE_MIN * 2 + 7,
    ];
    let block_size = BLOCK_SIZE_MIN;

    for &size in &sizes {
        let original_data = generate_deterministic_data(size as _);

        // compression
        let mut compressed_output = Vec::new();
        compress(
            Cursor::new(&original_data),
            &mut compressed_output,
            BlockSize::new(block_size).unwrap(),
            None,
        )
        .unwrap_or_else(|_| panic!("Compression failed for size {}", size));

        // decompression
        let mut decompressed_output = Vec::new();
        decompress(
            Cursor::new(&compressed_output),
            &mut decompressed_output,
            None,
        )
        .unwrap_or_else(|_| panic!("Decompression failed for size {}", size));

        assert_eq!(
            original_data, decompressed_output,
            "Data mismatch for size {}",
            size
        );
    }
}

#[test]
fn test_parallel_compression_reproducibility() {
    let sizes = [
        0,
        1024,
        BLOCK_SIZE_MIN,
        BLOCK_SIZE_MIN + 1,
        BLOCK_SIZE_MIN * 2,
        100 * 1024,
        200 * 1024,
        mib(100_u64) as _,
        mib(300_u64) as _,
    ];

    let block_size = BLOCK_SIZE_MIN;

    for &size in &sizes {
        let data = generate_deterministic_data(size as _);

        let mut first_run = Vec::new();
        compress(
            Cursor::new(&data),
            &mut first_run,
            BlockSize::new(block_size).unwrap(),
            None,
        )
        .unwrap_or_else(|_| panic!("First compression failed for size {}", size));

        let mut second_run = Vec::new();
        compress(
            Cursor::new(&data),
            &mut second_run,
            BlockSize::new(block_size).unwrap(),
            None,
        )
        .unwrap_or_else(|_| panic!("Second compression failed for size {}", size));

        assert_eq!(
            first_run, second_run,
            "Non-reproducible output at size {}",
            size
        );

        if size > 0 || !first_run.is_empty() {
            assert_eq!(
                &first_run[0..5],
                MAGIC_NUMBER,
                "Missing magic number at size {}",
                size
            );
        }
    }
}

#[test]
fn test_parallel_empty_input() {
    let original_data: Vec<u8> = Vec::new();
    let mut compressed = Vec::new();
    compress(
        Cursor::new(&original_data),
        &mut compressed,
        BlockSize::MIN,
        None,
    )
    .unwrap();

    let mut decompressed = Vec::new();
    decompress(Cursor::new(&compressed), &mut decompressed, None).unwrap();

    assert!(decompressed.is_empty());
}

#[test]
fn test_parallel_large() {
    let block_size = BLOCK_SIZE_MIN;
    let data_size = 10 * MIB as usize;
    let original_data = generate_deterministic_data(data_size);

    let mut compressed_buffer = Vec::new();
    compress(
        Cursor::new(&original_data),
        &mut compressed_buffer,
        BlockSize::new(block_size).unwrap(),
        None,
    )
    .expect("Parallel compression failed");

    assert!(compressed_buffer.len() > 9);
    assert_eq!(&compressed_buffer[0..5], MAGIC_NUMBER);

    let mut decompressed_buffer = Vec::new();
    decompress(
        Cursor::new(&compressed_buffer),
        &mut decompressed_buffer,
        None,
    )
    .expect("Parallel decompression failed");

    assert_eq!(
        original_data.len(),
        decompressed_buffer.len(),
        "Decompressed size mismatch"
    );
    assert_eq!(
        original_data, decompressed_buffer,
        "Data corruption in parallel round trip"
    );
}

mod write_based {
    use crate::{write, BlockSize, EMPTY_DATA_BLOCK, HEADER_BS16MIB};
    use std::io::Write;

    #[test]
    fn encoder_write_header_on_finish() {
        let mut buf: Vec<u8> = Vec::new();
        let e = write::Bz3Encoder::new(&mut buf, BlockSize::DEFAULT);
        drop(e);
        assert_eq!(buf, HEADER_BS16MIB);

        let mut buf = Vec::new();
        let mut e = write::Bz3Encoder::new(&mut buf, BlockSize::DEFAULT);
        e.write_all(b"aaa").unwrap();
        e.write_all(b"bbb").unwrap();
        drop(e);
        assert_eq!(&buf[..HEADER_BS16MIB.len()], HEADER_BS16MIB);
    }

    #[test]
    fn decoder_detect_incomplete_data() {
        let mut buf = Vec::new();
        let d = write::Bz3Decoder::new(&mut buf);
        assert_eq!(
            format!("{}", d.finish().err().unwrap()),
            "Insufficient data to read for file header"
        );

        let mut buf = Vec::new();
        let mut d = write::Bz3Decoder::new(&mut buf);
        d.write_all(HEADER_BS16MIB).unwrap();
        assert!(d.finish().is_ok());

        let mut buf = Vec::new();
        let mut d = write::Bz3Decoder::new(&mut buf);
        d.write_all(HEADER_BS16MIB).unwrap();
        d.write_all(
            &EMPTY_DATA_BLOCK[..5], /* insufficient block header data*/
        )
        .unwrap();
        assert_eq!(
            format!("{}", d.finish().err().unwrap()),
            "Insufficient data to read for block header"
        );

        let mut buf = Vec::new();
        let mut d = write::Bz3Decoder::new(&mut buf);
        d.write_all(HEADER_BS16MIB).unwrap();
        d.write_all(&EMPTY_DATA_BLOCK[..8]).unwrap();
        assert_eq!(
            format!("{}", d.finish().err().unwrap()),
            "Insufficient data to decode a full block"
        );

        let mut buf = Vec::new();
        let mut d = write::Bz3Decoder::new(&mut buf);
        d.write_all(HEADER_BS16MIB).unwrap();
        d.write_all(
            &EMPTY_DATA_BLOCK[..10], /* full block header + truncated compressed data */
        )
        .unwrap();
        assert_eq!(
            format!("{}", d.finish().err().unwrap()),
            "Insufficient data to decode a full block"
        );

        let mut buf = Vec::new();
        let mut d = write::Bz3Decoder::new(&mut buf);
        d.write_all(HEADER_BS16MIB).unwrap();
        // a full block
        d.write_all(&EMPTY_DATA_BLOCK).unwrap();
        assert!(d.finish().is_ok());
    }
}
