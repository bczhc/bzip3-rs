use std::io;
use std::io::Cursor;

use bytesize::ByteSize;
use rand::{thread_rng, RngCore};

use bzip3::{read, write};

#[test]
fn test() {
    println!("Test");
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

    for data_size in test_size_array {
        for block_size in block_size_array {
            test_read_based(data_size, block_size);
            test_write_based(data_size, block_size);
        }
    }
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
        println!("encode: {:?}", (data_size, block_size));
        let mut reader = Cursor::new(&mut data);
        let mut encoder = read::Bz3Encoder::new(&mut reader, block_size).unwrap();
        io::copy(&mut encoder, &mut compressed).unwrap();
    }
    let compressed = compressed.into_inner();

    let mut uncompressed = Cursor::new(Vec::new());
    {
        println!("decode: {:?}", (data_size, block_size));
        let mut reader = Cursor::new(compressed);
        let mut decoder = read::Bz3Decoder::new(&mut reader).unwrap();
        assert_eq!(decoder.block_size() as usize, block_size);
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
