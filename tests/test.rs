use bytesize::ByteSize;
use std::io;
use std::io::Cursor;

#[test]
fn test() {
    let data = b"hello, world";
    let mut reader1 = Cursor::new(data);
    let mut writer1 = Cursor::new(Vec::new());

    let block_size = ByteSize::mib(5).0 as usize;

    bzip3::stream::compress(&mut reader1, &mut writer1, block_size).unwrap();

    let mut reader2 = Cursor::new(data);
    let mut writer2 = Cursor::new(Vec::new());
    let mut encoder = bzip3::read::Bz3Encoder::new(&mut reader2, block_size).unwrap();
    io::copy(&mut encoder, &mut writer2).unwrap();

    assert_eq!(writer1.get_ref(), writer2.get_ref());
}
