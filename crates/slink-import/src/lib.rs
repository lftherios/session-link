//! Coding agents' own session histories, read as `session/v0`.
//!
//! An importer maps what a harness wrote to disk onto the format: nothing is
//! run again and nothing is invented, so an import is marked `reconstructed`
//! and what the transcript does not hold stays missing. Each importer must
//! produce the document its fixture under `testdata/import` pins, and the Go
//! CLI's importer is the oracle for its bytes.
//!
//! Each harness has one module: its importer, and where it keeps sessions.
//! `discover` asks all of them at once.

mod agents;
mod aider;
mod claude_code;
mod codex;
mod db;
mod discover;
mod dsh;
mod hermes;
mod images;
mod normalize;
mod opencode;
mod pi;
mod value;

use std::fmt;
use std::path::PathBuf;

use serde_json::Value;

pub use aider::in_utc as aider_in_utc;
pub use discover::{
    Candidate, Elsewhere, Found, Loader, Locations, active_session, aider_run, latest,
    latest_by_id, load_file, newest_anywhere, project_dirs, read_lines, recent, transcript_by_id,
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
    match harness {
        "aider" => aider::PORTED,
        "claude-code" => true,
        "codex" => codex::PORTED,
        "dsh" => dsh::PORTED,
        "hermes" => hermes::PORTED,
        "omp" | "pi" => pi::PORTED,
        "opencode" => opencode::PORTED,
        _ => false,
    }
}

/// What a harness recorded of one session, as its importer wants it. A
/// harness that keeps transcript files gives `lines`; one that keeps a
/// database gives `session` and `messages`.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// The transcript's lines, blank ones left out.
    pub lines: Vec<String>,
    /// The session's row, for a harness with a database.
    pub session: Value,
    /// Its messages' rows.
    pub messages: Vec<Value>,
    /// The session's name when the harness gives none.
    pub fallback: String,
    /// The directory a harness keeps a transcript's large content in, beside
    /// the transcript, when it has one. omp moves images there.
    pub blobs: PathBuf,
    /// The sub-agents this session started. A harness records each one's
    /// work apart from the session: in a file beside it, in another rollout,
    /// or as another row.
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
    /// The importer could not read what it was given.
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotPorted(harness) => write!(
                f,
                "{harness} sessions cannot be imported by slink-rs yet — the Go slink imports them"
            ),
            Error::Unknown(harness) => write!(f, "no importer for {harness}"),
            Error::Failed(why) => write!(f, "import failed: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// The session in `input` as a `session/v0` document, or `None` when the
/// harness recorded no messages.
pub fn import(harness: &str, input: &Input) -> Result<Option<Value>, Error> {
    if HARNESSES.contains(&harness) && !ported(harness) {
        return Err(Error::NotPorted(harness.to_owned()));
    }
    let run = match harness {
        "aider" => aider::import(input),
        "claude-code" => Ok(claude_code::import(input)),
        "codex" => codex::import(input),
        "dsh" => dsh::import(input),
        "hermes" => hermes::import(input),
        "omp" | "pi" => pi::import(input, harness),
        "opencode" => opencode::import(input),
        other => return Err(Error::Unknown(other.to_owned())),
    };
    run.map_err(Error::Failed)
}
