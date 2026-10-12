//! Command-line flags, read the way the Go CLI's `flag` package reads them,
//! because scripts and muscle memory were written against that: one dash or
//! two, `-name value` or `-name=value`, flags before or after the arguments,
//! and everything after a bare `--` left alone.

use std::collections::HashMap;
use std::fmt::Write as _;

/// How a command ends other than by finishing: an exit code, and what to say
/// on stderr on the way out.
#[derive(Debug)]
pub struct Exit {
    pub code: u8,
    pub message: String,
}

impl Exit {
    /// A failure with a message, as most are.
    pub fn failure(message: impl Into<String>) -> Exit {
        Exit {
            code: 1,
            message: message.into(),
        }
    }

    /// The command was used wrongly.
    pub fn usage(message: impl Into<String>) -> Exit {
        Exit {
            code: 2,
            message: message.into(),
        }
    }
}

pub type Run = Result<(), Exit>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Default {
    Bool(bool),
    Int(i64),
    Text(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct Flag {
    pub name: &'static str,
    pub default: Default,
    pub usage: &'static str,
}

/// A command's flags and the text of its help.
#[derive(Debug, Clone, Copy)]
pub struct FlagSet {
    pub synopsis: &'static str,
    pub blurb: &'static str,
    pub example: &'static str,
    pub flags: &'static [Flag],
}

/// What a command line gave: each flag's value and the arguments around them.
#[derive(Debug)]
pub struct Parsed {
    values: HashMap<&'static str, Value>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
enum Value {
    Bool(bool),
    Int(i64),
    Text(String),
}

impl Parsed {
    pub fn bool(&self, name: &str) -> bool {
        matches!(self.values.get(name), Some(Value::Bool(true)))
    }

    pub fn int(&self, name: &str) -> i64 {
        match self.values.get(name) {
            Some(Value::Int(number)) => *number,
            _ => 0,
        }
    }

    pub fn text(&self, name: &str) -> &str {
        match self.values.get(name) {
            Some(Value::Text(text)) => text,
            _ => "",
        }
    }

    pub fn arg(&self, index: usize) -> &str {
        self.args.get(index).map_or("", String::as_str)
    }
}

impl FlagSet {
    /// The help text: the synopsis, what the command does, its flags in
    /// alphabetical order, and an example.
    pub fn help(&self) -> String {
        let mut out = format!("Usage:\n  {}\n\n  {}\n", self.synopsis, self.blurb);
        if !self.flags.is_empty() {
            out.push_str("\nFlags:\n");
            let mut flags: Vec<&Flag> = self.flags.iter().collect();
            flags.sort_by_key(|flag| flag.name);
            for flag in flags {
                let kind = match flag.default {
                    Default::Bool(_) => "",
                    Default::Int(_) => " int",
                    Default::Text(_) => " string",
                };
                write!(out, "  -{}{kind}\n    \t{}", flag.name, flag.usage)
                    .expect("writing to a string");
                // A default is shown unless it is the type's zero.
                match flag.default {
                    Default::Bool(true) => out.push_str(" (default true)"),
                    Default::Int(number) if number != 0 => {
                        write!(out, " (default {number})").expect("writing to a string");
                    }
                    Default::Text(text) if !text.is_empty() => {
                        write!(out, " (default {text:?})").expect("writing to a string");
                    }
                    _ => {}
                }
                out.push('\n');
            }
        }
        if !self.example.is_empty() {
            write!(out, "\nExample:\n  {}\n", self.example).expect("writing to a string");
        }
        out
    }

    fn find(&self, name: &str) -> Option<&Flag> {
        self.flags.iter().find(|flag| flag.name == name)
    }

    /// Reads a command line. Asking for help prints it to stdout and ends the
    /// command; a flag that is unknown, has no value or has a bad one ends it
    /// with the problem and the help on stderr.
    pub fn parse(&self, args: &[String]) -> Result<Parsed, Exit> {
        let wrong = |problem: String| Exit::usage(format!("{problem}\n{}", self.help().trim_end()));
        let mut values: HashMap<&'static str, Value> = self
            .flags
            .iter()
            .map(|flag| {
                let value = match flag.default {
                    Default::Bool(set) => Value::Bool(set),
                    Default::Int(number) => Value::Int(number),
                    Default::Text(text) => Value::Text(text.to_owned()),
                };
                (flag.name, value)
            })
            .collect();
        let mut positional = Vec::new();
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            index += 1;
            if arg == "--" {
                positional.extend_from_slice(&args[index..]);
                break;
            }
            if matches!(arg.as_str(), "-h" | "--help" | "-help") {
                print!("{}", self.help());
                return Err(Exit {
                    code: 0,
                    message: String::new(),
                });
            }
            if !arg.starts_with('-') || arg == "-" {
                positional.push(arg.clone());
                continue;
            }
            let body = arg.strip_prefix("--").unwrap_or(&arg[1..]);
            let (name, given) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (body, None),
            };
            if name.is_empty() || name.starts_with('-') {
                return Err(wrong(format!("bad flag syntax: {arg}")));
            }
            let Some(flag) = self.find(name) else {
                return Err(wrong(format!("flag provided but not defined: -{name}")));
            };
            let value = match (flag.default, given) {
                (Default::Bool(_), None) => Value::Bool(true),
                (Default::Bool(_), Some(text)) => match text.as_str() {
                    "1" | "t" | "T" | "true" | "TRUE" | "True" => Value::Bool(true),
                    "0" | "f" | "F" | "false" | "FALSE" | "False" => Value::Bool(false),
                    _ => {
                        return Err(wrong(format!(
                            "invalid boolean value {text:?} for -{name}: parse error"
                        )));
                    }
                },
                (kind, given) => {
                    let text = match given {
                        Some(text) => text,
                        // A value is the next argument, never the `--` that
                        // ends the flags.
                        None => match args.get(index).filter(|next| *next != "--") {
                            Some(next) => {
                                index += 1;
                                next.clone()
                            }
                            None => return Err(wrong(format!("flag needs an argument: {arg}"))),
                        },
                    };
                    match kind {
                        Default::Int(_) => match text.parse::<i64>() {
                            Ok(number) => Value::Int(number),
                            Err(_) => {
                                return Err(wrong(format!(
                                    "invalid value {text:?} for flag -{name}: parse error"
                                )));
                            }
                        },
                        _ => Value::Text(text),
                    }
                }
            };
            values.insert(flag.name, value);
        }
        Ok(Parsed {
            values,
            args: positional,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SET: FlagSet = FlagSet {
        synopsis: "slink thing [flags] [file]",
        blurb: "Does a thing.",
        example: "slink thing --limit 3",
        flags: &[
            Flag {
                name: "limit",
                default: Default::Int(10),
                usage: "rows to show",
            },
            Flag {
                name: "json",
                default: Default::Bool(false),
                usage: "emit JSON",
            },
            Flag {
                name: "from",
                default: Default::Text(""),
                usage: "agent",
            },
        ],
    };

    fn parse(args: &[&str]) -> Result<Parsed, Exit> {
        SET.parse(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn flags_are_read_before_and_after_arguments() {
        let parsed = parse(&["a.json", "--limit", "3", "-json", "b", "--from=pi"]).unwrap();
        assert_eq!(parsed.int("limit"), 3);
        assert!(parsed.bool("json"));
        assert_eq!(parsed.text("from"), "pi");
        assert_eq!(parsed.args, ["a.json", "b"]);
        let defaults = parse(&[]).unwrap();
        assert_eq!(
            (
                defaults.int("limit"),
                defaults.bool("json"),
                defaults.text("from")
            ),
            (10, false, "")
        );
        assert_eq!(defaults.arg(0), "");
    }

    #[test]
    fn everything_after_two_dashes_is_an_argument() {
        let parsed = parse(&["-limit=2", "--", "--json", "-x"]).unwrap();
        assert_eq!(parsed.int("limit"), 2);
        assert!(!parsed.bool("json"));
        assert_eq!(parsed.args, ["--json", "-x"]);
        assert_eq!(parse(&["-"]).unwrap().args, ["-"]);
    }

    #[test]
    fn a_wrong_flag_ends_the_command_with_two() {
        for (args, problem) in [
            (&["--nope"][..], "flag provided but not defined: -nope"),
            (&["--limit"][..], "flag needs an argument: --limit"),
            (&["-limit", "--"][..], "flag needs an argument: -limit"),
            (
                &["--limit", "many"][..],
                "invalid value \"many\" for flag -limit: parse error",
            ),
            (
                &["--json=maybe"][..],
                "invalid boolean value \"maybe\" for -json: parse error",
            ),
            (&["---json"][..], "bad flag syntax: ---json"),
        ] {
            let exit = parse(args).unwrap_err();
            assert_eq!(exit.code, 2, "{args:?}");
            assert!(exit.message.starts_with(problem), "{}", exit.message);
            assert!(
                exit.message.contains("Usage:\n  slink thing"),
                "{}",
                exit.message
            );
        }
        assert!(!parse(&["--json=false"]).unwrap().bool("json"));
    }

    #[test]
    fn help_lists_flags_by_name_with_their_defaults() {
        assert_eq!(
            SET.help(),
            "Usage:\n  slink thing [flags] [file]\n\n  Does a thing.\n\nFlags:\n  -from string\n    \tagent\n  -json\n    \temit JSON\n  -limit int\n    \trows to show (default 10)\n\nExample:\n  slink thing --limit 3\n"
        );
        assert_eq!(parse(&["x", "-h"]).unwrap_err().code, 0);
    }
}
