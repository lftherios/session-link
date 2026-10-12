//! Coding agents' own session histories, read as `session/v0`.
//!
//! An importer maps what a harness wrote to disk onto the format: nothing is
//! run again and nothing is invented, so an import is marked `reconstructed`
//! and what the transcript does not hold stays missing. Each importer must
//! produce the document its fixture under `testdata/import` pins, and the Go
//! CLI's importer is the oracle for its bytes.
//!
//! Claude Code is the one harness ported so far. The other seven are known
//! by name, so that a command can say which of them it cannot read yet.

mod agents;
mod claude_code;
mod discover;
mod normalize;
mod value;

use std::fmt;

use serde_json::Value;

pub use discover::{
    Elsewhere, Found, claude_transcript, latest, load_file, newest_anywhere, project_dirs,
    read_lines,
};

/// Every harness the product imports from, in the order a list of them is
/// printed.
pub const HARNESSES: [&str; 8] = [
    "aider",
    "claude-code",
    "codex",
    "dsh",
    "hermes",
    "omp",
    "opencode",
    "pi",
];

/// Whether this build can import from `harness`.
pub fn ported(harness: &str) -> bool {
    harness == "claude-code"
}

/// What a harness recorded of one session, as its importer wants it.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// The transcript's lines, blank ones left out.
    pub lines: Vec<String>,
    /// The session's name when the transcript gives none.
    pub fallback: String,
    /// The sub-agents this session started. A harness records each one's
    /// work apart from the session.
    pub agents: Vec<Agent>,
    /// Set for the transcript of a sub-agent, read to be nested in the
    /// session that started it.
    pub sub_agent: bool,
}

/// One sub-agent's session as its harness recorded it.
#[derive(Debug, Clone, Default)]
pub struct Agent {
    /// What the parent's record calls it.
    pub id: String,
    /// The tool call that started it, when the harness says so itself.
    pub call: String,
    pub name: String,
    pub input: Input,
}

/// Why nothing was imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A harness this build has no importer for yet.
    NotPorted(String),
    /// Not the name of any harness.
    Unknown(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotPorted(harness) => write!(
                f,
                "{harness} sessions cannot be imported by slink-rs yet — the Go slink imports them"
            ),
            Error::Unknown(harness) => write!(f, "no importer for {harness}"),
        }
    }
}

impl std::error::Error for Error {}

/// The session in `input` as a `session/v0` document, or `None` when the
/// transcript holds no messages.
pub fn import(harness: &str, input: &Input) -> Result<Option<Value>, Error> {
    match harness {
        "claude-code" => Ok(claude_code::import(input)),
        known if HARNESSES.contains(&known) => Err(Error::NotPorted(known.to_owned())),
        other => Err(Error::Unknown(other.to_owned())),
    }
}
