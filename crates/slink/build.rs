//! Embeds the viewer artifact that `npm run build:viewer` writes to
//! `build/viewer`: generates the list of the files its manifest names, each
//! included from the artifact itself, after checking them against the
//! manifest's digests.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::{env, fs};

use sha2::{Digest, Sha256};

fn main() {
    let dir = match env::var_os("SLINK_VIEWER_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by Cargo"))
            .join("..")
            .join("..")
            .join("build")
            .join("viewer"),
    };
    println!("cargo:rerun-if-env-changed=SLINK_VIEWER_DIR");
    println!(
        "cargo:rustc-env=SLINK_TARGET={}",
        env::var("TARGET").expect("set by Cargo")
    );

    let manifest_path = dir.join("manifest.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest = fs::read_to_string(&manifest_path).unwrap_or_else(|err| {
        panic!(
            "no viewer artifact at {}: {err}\nrun `npm run build:viewer` first",
            dir.display()
        )
    });
    let parsed: serde_json::Value =
        serde_json::from_str(&manifest).expect("the viewer manifest is JSON");
    let files = parsed["files"]
        .as_array()
        .expect("the viewer manifest lists its files");

    let mut generated = format!(
        "pub const MANIFEST: &str = include_str!({:?});\n",
        manifest_path.display().to_string()
    );
    generated.push_str("pub const FILES: &[File] = &[\n");
    for file in files {
        let name = file["path"].as_str().expect("a file has a path");
        let expected = file["sha256"].as_str().expect("a file has a digest");
        let path = dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = fs::read(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        let digest = hex(&Sha256::digest(&bytes));
        assert!(
            digest == expected,
            "{} does not match the viewer manifest; run `npm run build:viewer` again",
            path.display()
        );
        writeln!(
            generated,
            "    File {{ path: {name:?}, sha256: {expected:?}, bytes: include_bytes!({:?}) }},",
            path.display().to_string()
        )
        .expect("writing to a string");
    }
    generated.push_str("];\n");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("set by Cargo")).join("viewer_files.rs");
    fs::write(&out, generated).unwrap_or_else(|err| panic!("{}: {err}", out.display()));
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a string");
        out
    })
}
