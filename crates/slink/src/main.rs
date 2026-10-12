//! The Rust `slink`, built as `slink-rs` until it replaces the Go CLI
//! (docs/rust-migration.md). It knows every command the Go CLI has and says
//! which of them it cannot run yet; docs/rust-cli.md has what is ported.

use std::env;
use std::process::ExitCode;

use commands::{Command, USAGE};
use flags::{Exit, Run};

mod check;
mod commands;
mod flags;
mod import;
mod list;
mod text;
mod tls;
mod view;
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
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(exit) => {
            if !exit.message.is_empty() {
                eprintln!("{}", exit.message);
            }
            ExitCode::from(exit.code)
        }
    }
}

fn run(args: &[String]) -> Run {
    let Some((name, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return Err(Exit {
            code: 1,
            message: String::new(),
        });
    };
    match name.as_str() {
        "help" | "--help" | "-h" | "-help" => {
            match rest.first().and_then(|command| commands::find(command)) {
                Some(command) => dispatch(command, &["-h".to_owned()]),
                None => {
                    print!("{USAGE}");
                    Ok(())
                }
            }
        }
        "version" | "--version" | "-v" | "-version" => {
            println!("slink {VERSION}");
            Ok(())
        }
        // Not a command of the product: it proves a build on its target.
        "build-check" => check::run(),
        other => match commands::find(other) {
            Some(command) => dispatch(command, rest),
            None => {
                let mut message = format!("slink: unknown command {}", text::quoted(other));
                if let Some(close) = commands::suggest(other) {
                    message.push_str(&format!("\n  did you mean: slink {close}?"));
                }
                message.push_str("\n  run `slink --help` for the command list");
                Err(Exit::failure(message))
            }
        },
    }
}

fn dispatch(command: &Command, args: &[String]) -> Run {
    // Help touches no local data, and neither do the commands that only
    // print a script or shell lines.
    let help = args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "-help"));
    if command.local_data && !help {
        slink_local::secure_home().map_err(|error| {
            Exit::failure(format!("cannot protect local session data: {error}"))
        })?;
    }
    let parsed = command.set.parse(args)?;
    match command.name {
        "list" => list::run(&parsed),
        "import" => import::run(&parsed),
        "view" => view::run(&parsed),
        "completion" => match commands::completion(parsed.arg(0)) {
            Some(script) => {
                print!("{script}");
                Ok(())
            }
            None => Err(Exit::usage("which shell? slink completion <bash|zsh|fish>")),
        },
        // Nothing leaves the machine from this binary until publishing is
        // ported whole: there is no half of it that would be safe to have.
        "share" | "push" | "login" | "logout" | "delete" => Err(Exit::failure(format!(
            "slink-rs cannot publish yet — `{}` is not ported, and nothing was uploaded\n  the Go slink publishes: https://session.link/docs",
            command.name
        ))),
        name => Err(Exit::failure(format!(
            "slink-rs: `{name}` is not ported yet"
        ))),
    }
}
