//! The command surface: every command the Go CLI has, with the flags and the
//! help text it has there. `testdata/compat/cli.json` is the same surface as
//! data, taken from the Go source, and `scripts/compat.mjs` holds this
//! binary to it.

use crate::flags::Default::{Bool, Int, Text};
use crate::flags::{Flag, FlagSet};

pub struct Command {
    pub name: &'static str,
    pub alias: Option<&'static str>,
    /// Whether it reads or writes the slink home, which is then made private
    /// before anything else happens.
    pub local_data: bool,
    pub set: FlagSet,
}

const AGENTS: &str = "agent: claude-code|codex|pi|omp|opencode|hermes|dsh|aider";

const fn flag(name: &'static str, default: crate::flags::Default, usage: &'static str) -> Flag {
    Flag {
        name,
        default,
        usage,
    }
}

/// Every name a command line can start with, in the order a completion
/// script offers them and a mistyped one is matched against.
pub const NAMES: [&str; 22] = [
    "share",
    "record",
    "dev",
    "view",
    "open",
    "list",
    "ls",
    "import",
    "tap",
    "setup",
    "on",
    "off",
    "status",
    "login",
    "logout",
    "push",
    "delete",
    "prune",
    "doctor",
    "completion",
    "version",
    "help",
];

pub const COMMANDS: [Command; 17] = [
    Command {
        name: "share",
        alias: None,
        local_data: true,
        set: FlagSet {
            synopsis: "slink share [flags] [session-id | transcript]",
            blurb: "Publish the session you mean, in one step: your newest local session\n  or your agent's newest for this project — whichever is fresher.\n  Signs you in along the way if needed.",
            example: "slink share",
            flags: &[
                flag("from", Text(""), AGENTS),
                flag("session", Text(""), "session id, or a transcript path"),
                flag("pick", Bool(false), "choose from recent sessions"),
                flag("yes", Bool(false), "publish without confirmation"),
                flag("server", Text(""), "override the publish target"),
                flag("key", Text(""), "override the API key"),
            ],
        },
    },
    Command {
        name: "record",
        alias: Some("dev"),
        local_data: true,
        set: FlagSet {
            synopsis: "slink record [flags] -- <command …>   (alias: dev)",
            blurb: "Run a command with its LLM calls recorded to a local session. Without\n  a command, prints proxy exports for this shell instead (Ctrl-C to finish).",
            example: "slink record -- python agent.py",
            flags: &[
                flag(
                    "port",
                    Int(-1),
                    "listen port (default: ephemeral when wrapping, 4141 otherwise)",
                ),
                flag("name", Text(""), "session name"),
            ],
        },
    },
    Command {
        name: "view",
        alias: Some("open"),
        local_data: true,
        set: FlagSet {
            synopsis: "slink view [flags] [session-id | transcript | session.json]",
            blurb: "Open agent work in a local web preview, without signing in or uploading.\n  Finds sessions for this project across harnesses. Multiple sessions open\n  a browser picker; --session identifies the exact session to preview.\n  The preview is saved separately, so later agent work cannot change it.",
            example: "slink view\n  slink view --from pi --session <transcript> --background\n  slink view --no-browser --port 4400",
            flags: &[
                flag("port", Int(0), "listen port (default: a free port)"),
                flag(
                    "no-browser",
                    Bool(false),
                    "print the local URL without opening a browser",
                ),
                flag(
                    "background",
                    Bool(false),
                    "return to the harness while the local viewer stays running",
                ),
                flag("from", Text(""), AGENTS),
                flag(
                    "session",
                    Text(""),
                    "explicit harness session ID or transcript path",
                ),
                flag("span", Text(""), "start at this span in the viewer"),
                flag(
                    "pick",
                    Bool(false),
                    "always show the browser session picker",
                ),
            ],
        },
    },
    Command {
        name: "list",
        alias: Some("ls"),
        local_data: true,
        set: FlagSet {
            synopsis: "slink list [flags]",
            blurb: "List local sessions, newest first. Nothing here has been published.\n  The first column is the session id — view, push, and share accept it.\n  (delete takes a published URL, not a local id.)",
            example: "slink list --limit 25",
            flags: &[
                flag("limit", Int(10), "rows to show"),
                flag(
                    "json",
                    Bool(false),
                    "emit JSON (one array; ids, paths, and metadata)",
                ),
            ],
        },
    },
    Command {
        name: "import",
        alias: None,
        local_data: true,
        set: FlagSet {
            synopsis: "slink import [flags] [transcript]",
            blurb: "Convert your coding agent's newest session for this project into a\n  local session — no re-run needed. Marked fidelity: reconstructed.",
            example: "slink import --from claude-code",
            flags: &[
                flag("from", Text(""), AGENTS),
                flag("session", Text(""), "session id, or a transcript path"),
            ],
        },
    },
    Command {
        name: "tap",
        alias: None,
        local_data: true,
        set: FlagSet {
            synopsis: "slink tap [flags]",
            blurb: "Run the always-on recorder in the foreground. Shells route through it\n  via `eval \"$(slink on)\"`; each burst of traffic becomes its own session.",
            example: "slink tap --port 4141",
            flags: &[
                flag("port", Int(4141), "listen port"),
                flag(
                    "idle",
                    Int(15),
                    "minutes of quiet before any session rolls into a new activity window",
                ),
                flag(
                    "install",
                    Bool(false),
                    "install as a login service (launchd/systemd)",
                ),
                flag("uninstall", Bool(false), "remove the login service"),
                flag("stop", Bool(false), "stop the running tap daemon"),
            ],
        },
    },
    Command {
        name: "setup",
        alias: None,
        local_data: true,
        set: FlagSet {
            synopsis: "slink setup",
            blurb: "Guided always-on capture: installs the tap as a login service so\n  recording is a background fact — local only, prunable, undoable.",
            example: "slink setup",
            flags: &[flag("port", Int(4141), "tap port")],
        },
    },
    Command {
        name: "on",
        alias: None,
        local_data: false,
        set: FlagSet {
            synopsis: "eval \"$(slink on)\"  [flags]",
            blurb: "Route this shell's Anthropic/OpenAI traffic through the always-on tap.\n  Prints export lines — they only take effect inside eval \"$(...)\".",
            example: "eval \"$(slink on)\"",
            flags: &[
                flag(
                    "port",
                    Int(0),
                    "tap port (default: the running tap's, else 4141)",
                ),
                flag("no-start", Bool(false), "don't auto-start the tap"),
            ],
        },
    },
    Command {
        name: "off",
        alias: None,
        local_data: false,
        set: FlagSet {
            synopsis: "eval \"$(slink off)\"",
            blurb: "Stop routing this shell through the tap. Prints unset lines — they\n  only take effect inside eval \"$(...)\".",
            example: "eval \"$(slink off)\"",
            flags: &[],
        },
    },
    Command {
        name: "status",
        alias: None,
        local_data: true,
        set: FlagSet {
            synopsis: "slink status",
            blurb: "One honest screen: recorder, this shell's routing, upstreams,\n  account, and what's been captured.",
            example: "slink status",
            flags: &[flag("json", Bool(false), "emit JSON")],
        },
    },
    Command {
        name: "login",
        alias: None,
        local_data: true,
        set: FlagSet {
            synopsis: "slink login [flags]",
            blurb: "One-time sign-in (GitHub) so published sessions are attributed to you\n  and deletable by you. Capturing never needs an account.",
            example: "slink login",
            flags: &[
                flag(
                    "key",
                    Text(""),
                    "paste an rk_… API key instead of the browser flow",
                ),
                flag("server", Text(""), "login against a specific server"),
            ],
        },
    },
    Command {
        name: "logout",
        alias: None,
        local_data: true,
        set: FlagSet {
            synopsis: "slink logout",
            blurb: "Forget the saved API key. Capturing is unaffected; published\n  sessions stay up (delete them individually with slink delete).",
            example: "slink logout",
            flags: &[],
        },
    },
    Command {
        name: "push",
        alias: None,
        local_data: true,
        set: FlagSet {
            synopsis: "slink push [flags] [session-id | session.json]",
            blurb: "Publish a session: validate, secret-scan locally, confirm, get a link.\n  With no argument, pushes the newest local session. Scriptable; the\n  human-friendly flow is `slink share`.",
            example: "slink push --pick",
            flags: &[
                flag("yes", Bool(false), "publish without confirmation"),
                flag("pick", Bool(false), "choose from recent sessions"),
                flag("server", Text(""), "override the publish target"),
                flag("key", Text(""), "override the API key"),
            ],
        },
    },
    Command {
        name: "delete",
        alias: None,
        local_data: true,
        set: FlagSet {
            synopsis: "slink delete <url | published-id>",
            blurb: "Take a published session offline. The link answers 410 immediately.\n  Only your own publishes can be deleted.",
            example: "slink delete https://session.link/r/9f3kx2mvq7wtd4",
            flags: &[
                flag("yes", Bool(false), "delete without confirmation"),
                flag("server", Text(""), "override the server"),
                flag("key", Text(""), "override the API key"),
            ],
        },
    },
    Command {
        name: "prune",
        alias: None,
        local_data: true,
        set: FlagSet {
            synopsis: "slink prune [flags]",
            blurb: "Delete old local sessions. Bare `slink prune` uses a 30-day window.",
            example: "slink prune --keep 50 --dry-run",
            flags: &[
                flag("older-than", Int(-1), "days; sessions older are pruned"),
                flag("keep", Int(-1), "keep only the newest N"),
                flag(
                    "empty",
                    Bool(false),
                    "prune sessions with no recorded calls",
                ),
                flag("yes", Bool(false), "prune without confirmation"),
                flag("dry-run", Bool(false), "show what would be pruned"),
            ],
        },
    },
    Command {
        name: "doctor",
        alias: None,
        local_data: true,
        set: FlagSet {
            synopsis: "slink doctor",
            blurb: "Check the recording setup: state dir, tap, this shell's routing,\n  upstreams, and account. Read-only; nothing is uploaded.",
            example: "slink doctor",
            flags: &[],
        },
    },
    Command {
        name: "completion",
        alias: None,
        local_data: false,
        set: FlagSet {
            synopsis: "slink completion <bash|zsh|fish>",
            blurb: "Print a tab-completion script. Each script's header says where to\n  put it for that shell.",
            example: "eval \"$(slink completion zsh)\"   # or add that line to ~/.zshrc",
            flags: &[],
        },
    },
];

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS
        .iter()
        .find(|command| command.name == name || command.alias == Some(name))
}

pub const USAGE: &str = "slink — record LLM sessions locally, share the ones worth sharing.

Usage:
  slink <command> [flags]

Quickstart:
  slink view                        # choose a session and preview locally
  (use Share this view in the browser when ready to share)
  slink share                       # or publish directly from the terminal
  (nothing to import? record fresh: slink record -- python agent.py)

Record (local — nothing leaves your machine)
  record -- <cmd>    run a command with its LLM calls recorded  (alias: dev)
  import             convert your coding agent's newest session — no re-run
                     (claude-code, codex, opencode, pi, omp, hermes, dsh,
                     aider)
  setup              guided always-on capture (tap as a login service)
  on / off           route this shell through the tap:  eval \"$(slink on)\"

Review
  list               your local sessions, newest first (ids in column one)
  view               agent sessions → saved local web preview (alias: open)
  status             recorder, routing, upstreams, account, sessions

Share
  share              publish the session you mean — newest local or your
                     coding agent's newest — review, confirm, get a link
  delete             take a published session offline
  login / logout     the free account publishing needs; capture never does
  push               the scriptable core of share: validate, scan, confirm

Housekeeping
  tap                the always-on recorder (foreground; --stop, --install)
  prune              delete old local sessions
  doctor             check the recording setup end to end
  completion         shell tab-completion (bash, zsh, fish)
  version            print the slink version

Flags for a command: slink <command> -h
Sessions stay local in ~/.slink until you publish them (the always-on tap
prunes its old ones after 30 days; one-off recordings are kept).
";

/// The closest command to a mistyped one, within two edits.
pub fn suggest(command: &str) -> Option<&'static str> {
    let mut best = (None, 3);
    for name in NAMES {
        let distance = edit_distance(command, name);
        if distance < best.1 {
            best = (Some(name), distance);
        }
    }
    best.0
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, y) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(x != y);
            current.push(substitute.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

/// A static completion script: commands only, with file names for the rest.
pub fn completion(shell: &str) -> Option<String> {
    let names = NAMES.join(" ");
    Some(match shell {
        "bash" => format!(
            "# slink bash completion — add to ~/.bashrc:  eval \"$(slink completion bash)\"
_slink() {{
  local cur=${{COMP_WORDS[COMP_CWORD]}}
  if [ \"$COMP_CWORD\" -eq 1 ]; then
    COMPREPLY=($(compgen -W \"{names}\" -- \"$cur\"))
  else
    COMPREPLY=($(compgen -f -- \"$cur\"))
  fi
}}
complete -F _slink slink
"
        ),
        "zsh" => format!(
            "# slink zsh completion — add to ~/.zshrc:  eval \"$(slink completion zsh)\"
_slink() {{
  if (( CURRENT == 2 )); then
    compadd {names}
  else
    _files
  fi
}}
# compdef exists only after compinit — initialize if the user's rc hasn't.
if ! (( $+functions[compdef] )); then
  autoload -Uz compinit && compinit -i
fi
compdef _slink slink
"
        ),
        "fish" => format!(
            "# slink fish completion — save as ~/.config/fish/completions/slink.fish
complete -c slink -f -n \"__fish_use_subcommand\" -a \"{names}\"
complete -c slink -n \"not __fish_use_subcommand\" -F
"
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mistyped_command_finds_its_neighbour() {
        assert_eq!(suggest("shar"), Some("share"));
        assert_eq!(suggest("lst"), Some("list"));
        assert_eq!(suggest("imprt"), Some("import"));
        assert_eq!(suggest("zzzzzz"), None);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
    }

    #[test]
    fn every_command_and_alias_is_a_name() {
        for command in &COMMANDS {
            assert!(NAMES.contains(&command.name), "{}", command.name);
            if let Some(alias) = command.alias {
                assert!(NAMES.contains(&alias), "{alias}");
                assert!(find(alias).is_some_and(|found| found.name == command.name));
            }
            assert!(USAGE.contains(command.name), "{}", command.name);
        }
    }
}
