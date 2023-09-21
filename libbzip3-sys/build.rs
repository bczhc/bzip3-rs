extern crate bindgen;

use cfg_if::cfg_if;
use std::env;
use std::path::{Path, PathBuf};

const DEFAULT_LIB_DIR: &str = "/usr/lib";
const DEFAULT_INCLUDE_DIR: &str = "/usr/include";
#[cfg(feature = "bundled")]
const BZIP3_REPO_DIR: &str = "./bzip3";

#[allow(unused)]
fn main() {
    let mut bzip3_include_dir = String::from(DEFAULT_INCLUDE_DIR);

    let bindings_output = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");

    let mut bzip3_lib_dir = String::from(DEFAULT_LIB_DIR);
    if let Ok(d) = env::var("BZIP3_LIB_DIR") {
        bzip3_lib_dir = d;
    }
    if let Ok(d) = env::var("BZIP3_INCLUDE_DIR") {
        bzip3_include_dir = d;
    }

    let mut header_file = Path::new(&bzip3_include_dir).join("libbz3.h");

    cfg_if! {
        if #[cfg(feature = "bundled")] {
            header_file = bundled::get_bzip3_header();
        }
    }

    if !header_file.exists() {
        panic!(
            "Header file doesn't exist: {:?}
Note: You can specify BZIP3_LIB_DIR and BZIP3_INCLUDE_DIR environment variables",
            header_file
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

    cfg_if! {
        if #[cfg(feature = "bundled")] {
            bundled::compile();
        } else {
            println!("cargo:rustc-link-search={}", bzip3_lib_dir);
            println!("cargo:rustc-link-lib=bzip3");
        }
    }
}

#[cfg(feature = "bundled")]
mod bundled {
    use crate::BZIP3_REPO_DIR;
    use regex::Regex;
    use std::fs::File;
    use std::io::Read;
    use std::path::PathBuf;

    fn parse_version() -> String {
        let bzip3_news_path = PathBuf::from(BZIP3_REPO_DIR).join("NEWS");
        if !bzip3_news_path.exists() {
            panic!("NEWS file doesn't exist; can't get library version");
        }

        let mut news_file = File::open(bzip3_news_path).unwrap();
        let mut read = String::new();
        news_file.read_to_string(&mut read).unwrap();
        drop(news_file);

        let version: Option<String> = (|| {
            let version_regex = Regex::new(r#"^v([0-9]+\.[0-9]+\.[0-9]+):$"#).unwrap();
            let mut lines = read.lines();
            let last = lines.rfind(|x| version_regex.is_match(x))?;

            let version = version_regex.captures_iter(last).next()?.get(1)?.as_str();
            Some(version.into())
        })();

        version.expect("Cannot find library version from NEWS file")
    }

    pub fn compile() {
        let version = parse_version();

        let src_file = PathBuf::from(BZIP3_REPO_DIR).join("src").join("libbz3.c");
        let include_dir = PathBuf::from(BZIP3_REPO_DIR).join("include");
        if !src_file.exists() {
            panic!("Missing source file: {:?}", src_file);
        }
        if !include_dir.exists() {
            panic!("Missing include dir: {:?}", include_dir);
        }
        cc::Build::new()
            .file(src_file)
            .include(include_dir)
            .define("VERSION", Some(format!(r#""{}""#, version).as_str()))
            .warnings(false)
            .compile("bzip3");
    }

    pub fn get_bzip3_header() -> PathBuf {
        let path = PathBuf::from(BZIP3_REPO_DIR)
            .join("include")
            .join("libbz3.h");
        if !path.exists() {
            panic!("Missing header file: {:?}", path)
        }
        path
    }
}
