//! The Rust `slink`. It is built as `slink-rs` and has only the commands below
//! until the port reaches the rest (docs/rust-migration.md).

use std::env;
use std::process::ExitCode;

mod check;
mod tls;
mod viewer;

/// Stamped at release through `SLINK_VERSION`.
const VERSION: &str = match option_env!("SLINK_VERSION") {
    Some(version) => version,
    None => "0.0.0-dev",
};

fn main() -> ExitCode {
    let args: Vec<String> = env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    match args.first().map(String::as_str) {
        Some("version" | "--version" | "-v" | "-version") => {
            println!("slink {VERSION}");
            ExitCode::SUCCESS
        }
        Some("build-check") => check::run(),
        Some(command) => {
            eprintln!("slink-rs: `{command}` is not ported yet");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("slink-rs: only `version` and `build-check` are ported so far");
            ExitCode::FAILURE
        }
    }
}
