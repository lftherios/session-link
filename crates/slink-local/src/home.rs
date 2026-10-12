//! `SLINK_HOME`, or `.slink` in the user's home directory, and the two rules
//! its files live by: only the user can read them, and a file is replaced
//! whole or not at all.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub fn home() -> PathBuf {
    match env::var_os("SLINK_HOME") {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => match user_home() {
            Some(home) => home.join(".slink"),
            None => PathBuf::from(".slink"),
        },
    }
}

/// The user's home directory, as the environment names it.
pub(crate) fn user_home() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(name)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

pub fn capture_dir() -> PathBuf {
    home().join("runs")
}

pub fn preview_dir() -> PathBuf {
    home().join("previews")
}

/// A path for a person to read: the home directory as `~`.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = user_home()
        && let Ok(rest) = path.strip_prefix(&home)
        && !rest.as_os_str().is_empty()
    {
        return format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display());
    }
    path.display().to_string()
}

/// Makes the home and everything in it readable by the user alone. Links are
/// left as they are: what they point at is not the home's to change.
pub fn secure_home() -> io::Result<()> {
    let home = home();
    create_private_dir(&home)?;
    if !fs::symlink_metadata(&home)?.is_dir() {
        return Err(io::Error::other(format!(
            "session-link home must be a directory: {}",
            home.display()
        )));
    }
    secure(&home)
}

fn secure(path: &Path) -> io::Result<()> {
    let kind = fs::symlink_metadata(path)?.file_type();
    if kind.is_symlink() {
        return Ok(());
    }
    if kind.is_dir() {
        restrict(path, 0o700)?;
        for entry in fs::read_dir(path)? {
            secure(&entry?.path())?;
        }
    } else if kind.is_file() {
        restrict(path, 0o600)?;
    }
    Ok(())
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// A Windows profile directory is the user's alone already.
#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Replaces `file` with `data`: written beside it under another name, flushed
/// to disk, then renamed over it, so a reader sees the old file or the new
/// one and a crash leaves one of the two.
pub fn write_private(file: &Path, data: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = file.parent().unwrap_or(Path::new("."));
    create_private_dir(dir)?;
    let (temporary, mut handle) = loop {
        let name = format!(
            ".private-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = dir.join(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(handle) => break (path, handle),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let written = handle
        .write_all(data)
        .and_then(|()| handle.sync_all())
        .and_then(|()| {
            drop(handle);
            fs::rename(&temporary, file)
        });
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
        return written;
    }
    // The rename is on disk once the directory is. Windows has no handle to
    // a directory to flush, and renames there are durable on their own terms.
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = File::open(dir);
    Ok(())
}
