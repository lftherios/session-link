//! The viewer artifact, embedded at build time (see `build.rs`).

/// One file of the artifact, with the digest its manifest records.
pub struct File {
    pub path: &'static str,
    pub sha256: &'static str,
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/viewer_files.rs"));

pub fn file(path: &str) -> Option<&'static File> {
    FILES.iter().find(|file| file.path == path)
}
