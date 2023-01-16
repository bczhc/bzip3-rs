use std::io;
use std::io::{Cursor, Read, Write};

use bytesize::ByteSize;
use rand::RngCore;

use bzip3::read::{Bz3Decoder, Bz3Encoder};

#[test]
fn test() {
    println!("Test");
    let mut rng = rand::thread_rng();
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
            let mut data = vec![0_u8; data_size];
            rng.fill_bytes(&mut data);

            let mut compressed = Cursor::new(Vec::new());
            {
                println!("encode: {:?}", (data_size, block_size));
                let mut reader = Cursor::new(&mut data);
                let mut encoder = Bz3Encoder::new(&mut reader, block_size).unwrap();
                io_generic_copy(&mut encoder, &mut compressed).unwrap();
            }
            let compressed = compressed.into_inner();

            let mut uncompressed = Cursor::new(Vec::new());
            {
                println!("decode: {:?}", (data_size, block_size));
                let mut reader = Cursor::new(compressed);
                let mut decoder = Bz3Decoder::new(&mut reader).unwrap();
                assert_eq!(decoder.block_size() as usize, block_size);
                io_generic_copy(&mut decoder, &mut uncompressed).unwrap();
            }

            assert_eq!(uncompressed.get_ref().as_slice(), data.as_slice());
        }
    }
}

fn io_generic_copy<R: Read, W: Write>(src: &mut R, dst: &mut W) -> io::Result<()> {
    let mut buf = [0_u8; 4096];
    loop {
        let read_size = src.read(&mut buf)?;
        if read_size == 0 {
            break;
        }
        dst.write_all(&buf[..read_size])?;
    }
    Ok(())
}
