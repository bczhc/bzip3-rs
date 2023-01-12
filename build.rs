extern crate bindgen;

use std::env;
use std::path::Path;

const DEFAULT_LIB_DIR: &str = "/usr/lib";
const DEFAULT_INCLUDE_DIR: &str = "/usr/include";

fn main() {
    let mut bzip3_lib_dir = String::from(DEFAULT_LIB_DIR);
    let mut bzip3_include_dir = String::from(DEFAULT_INCLUDE_DIR);

    let bindings_output = Path::new("src/bindings.rs");

    if let Ok(d) = env::var("BZIP3_LIB_DIR") {
        bzip3_lib_dir = d;
    }
    if let Ok(d) = env::var("BZIP3_INCLUDE_DIR") {
        bzip3_include_dir = d;
    }

    println!("cargo:rustc-link-search={}", bzip3_lib_dir);

    println!("cargo:rustc-link-lib=bzip3");

    let header_file = Path::new(&bzip3_include_dir).join("libbz3.h");

    if !header_file.exists() {
        panic!(
            "Header file doesn't exist.
Note: You can specify BZIP3_LIB_DIR and BZIP3_INCLUDE_DIR environment variables"
        );
    }

    // TODO: handle OsStr (e.g. arbitrary bytes path on filesystems like ext4)
    println!("cargo:rerun-if-changed={}", header_file.to_string_lossy());

    let bindings = bindgen::Builder::default()
        .header(header_file.to_string_lossy())
        .generate()
        .expect("Unable to generate bindings");

    bindings
        .write_to_file(bindings_output)
        .expect("Couldn't write bindings!");
}
