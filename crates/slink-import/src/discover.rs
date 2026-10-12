//! Finding a session on this machine: where a harness keeps its histories,
//! which of them belong to a directory, and what is in a file someone names.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;

use crate::claude_code::first_user_text;
use crate::value::text;
use crate::{Agent, Input};

/// A session that was found, not yet read.
#[derive(Debug, Clone)]
pub struct Found {
    pub harness: &'static str,
    pub file: PathBuf,
    /// When it was last written, in nanoseconds since 1970.
    pub recency: u128,
}

impl Found {
    pub fn load(&self) -> std::io::Result<Input> {
        Ok(transcript_input(
            &self.file,
            self.harness,
            read_lines(&self.file)?,
        ))
    }
}

fn user_home() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(name)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Where Claude Code keeps one directory of transcripts for each project.
/// `CLAUDE_CONFIG_DIR` moves its whole configuration directory.
fn claude_projects_dir() -> PathBuf {
    match env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("projects"),
        None => user_home()
            .unwrap_or_default()
            .join(".claude")
            .join("projects"),
    }
}

/// The longest directory name Claude Code derives from a path alone. A longer
/// one is cut here and ends in a hash of the whole path.
const CLAUDE_SLUG_LIMIT: usize = 200;

/// A project directory's name as Claude Code writes it: every UTF-16 code
/// unit other than an ASCII letter or digit becomes `-`.
fn claude_slug(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .encode_utf16()
        .map(|unit| match u8::try_from(unit) {
            Ok(byte) if byte.is_ascii_alphanumeric() => byte as char,
            _ => '-',
        })
        .collect()
}

/// The transcripts Claude Code keeps under `base` for one working directory.
/// A long path is found by its cut name, since the hash that follows depends
/// on the runtime Claude Code was built for; projects can share that name,
/// so each transcript must also record `cwd` as its directory.
fn claude_transcripts(base: &Path, cwd: &Path) -> Vec<PathBuf> {
    let slug = claude_slug(cwd);
    if slug.len() <= CLAUDE_SLUG_LIMIT {
        return transcripts_in(&base.join(slug));
    }
    let prefix = format!("{}-", &slug[..CLAUDE_SLUG_LIMIT]);
    let mut files = Vec::new();
    for dir in sorted_entries(base) {
        let named = dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(&prefix));
        if !dir.is_dir() || !named {
            continue;
        }
        for file in transcripts_in(&dir) {
            if Path::new(&peek(&file, 40).cwd) == cwd {
                files.push(file);
            }
        }
    }
    files
}

/// The transcript of a pasted session id, for one working directory. Claude
/// Code names a transcript `<uuid>.jsonl`.
pub fn claude_transcript(cwd: &Path, id: &str) -> Option<PathBuf> {
    let name = format!("{id}.jsonl");
    claude_transcripts(&claude_projects_dir(), cwd)
        .into_iter()
        .find(|file| {
            file.file_name()
                .is_some_and(|found| found.to_string_lossy() == name)
        })
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    entries.sort();
    entries
}

/// The `.jsonl` files directly inside `dir`, by name.
fn transcripts_in(dir: &Path) -> Vec<PathBuf> {
    sorted_entries(dir)
        .into_iter()
        .filter(|file| !file.is_dir() && file.to_string_lossy().ends_with(".jsonl"))
        .collect()
}

/// A transcript's lines, blank ones left out. Bytes that are not UTF-8 read
/// as U+FFFD, as they do in Go.
pub fn read_lines(file: &Path) -> std::io::Result<Vec<String>> {
    let raw = fs::read(file)?;
    Ok(String::from_utf8_lossy(&raw)
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect())
}

/// A transcript's lines with what its harness keeps beside them: for Claude
/// Code, the sessions of its sub-agents.
fn transcript_input(file: &Path, harness: &str, lines: Vec<String>) -> Input {
    let agents = if harness == "claude-code" {
        claude_agents(file)
    } else {
        Vec::new()
    };
    Input {
        lines,
        fallback: stem(file),
        agents,
        sub_agent: false,
    }
}

/// The file's name without `.jsonl`.
fn stem(file: &Path) -> String {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    name.strip_suffix(".jsonl").unwrap_or(&name).to_owned()
}

/// A Claude Code session's sub-agents. Claude Code writes each one's
/// transcript to `subagents/agent-<id>.jsonl`, in a directory named after
/// the session, with a `.meta.json` that names the tool call that started
/// it. A sub-agent's own sub-agents are in the same directory, one spawn
/// deeper, so the shallower ones come first.
fn claude_agents(transcript: &Path) -> Vec<Agent> {
    let Some(session) = transcript
        .to_str()
        .and_then(|path| path.strip_suffix(".jsonl"))
    else {
        return Vec::new();
    };
    let mut agents: Vec<(f64, Agent)> = Vec::new();
    for file in transcripts_in(&Path::new(session).join("subagents")) {
        let Ok(lines) = read_lines(&file) else {
            continue;
        };
        let name = stem(&file);
        let id = name.strip_prefix("agent-").unwrap_or(&name).to_owned();
        let meta = file
            .to_str()
            .and_then(|path| path.strip_suffix(".jsonl"))
            .and_then(|path| fs::read_to_string(format!("{path}.meta.json")).ok())
            .and_then(|raw| slink_format::parse(&raw).ok())
            .unwrap_or(Value::Null);
        let described = match text(&meta, "description") {
            "" => text(&meta, "agentType"),
            description => description,
        };
        let agent = Agent {
            call: text(&meta, "toolUseId").to_owned(),
            name: described.to_owned(),
            input: Input {
                lines,
                fallback: id.clone(),
                ..Input::default()
            },
            id,
        };
        agents.push((meta["spawnDepth"].as_f64().unwrap_or(1.0), agent));
    }
    agents.sort_by(|a, b| a.0.total_cmp(&b.0));
    agents.into_iter().map(|(_, agent)| agent).collect()
}

fn modified(file: &Path) -> Option<u128> {
    let modified = fs::metadata(file).ok()?.modified().ok()?;
    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_nanos())
}

/// The session a harness last wrote for `cwd`. With no harness named, the
/// newest across every harness this build can read.
pub fn latest(harness: &str, cwd: &Path) -> Option<Found> {
    if !harness.is_empty() && harness != "claude-code" {
        return None;
    }
    claude_transcripts(&claude_projects_dir(), cwd)
        .into_iter()
        .filter_map(|file| Some((modified(&file)?, file)))
        // The first of two written in the same instant, as Go picks.
        .fold(None, |best: Option<(u128, PathBuf)>, found| match best {
            Some(best) if best.0 >= found.0 => Some(best),
            _ => Some(found),
        })
        .map(|(recency, file)| Found {
            harness: "claude-code",
            file,
            recency,
        })
}

/// The directories a session for `cwd` may be recorded under: `cwd` itself,
/// then each parent, because an agent is usually started at a project's root
/// and used from inside it. The walk stops before the home directory and
/// before the filesystem root; a session started in either is not this
/// project's. Both still count when `cwd` is that directory itself.
pub fn project_dirs(cwd: &Path) -> Vec<PathBuf> {
    let home = user_home().unwrap_or_default();
    let mut dir = cwd.to_path_buf();
    let mut dirs = vec![dir.clone()];
    while dir != home {
        let Some(parent) = dir.parent().map(Path::to_path_buf) else {
            break;
        };
        if parent == home || parent.parent().is_none() {
            break;
        }
        dirs.push(parent.clone());
        dir = parent;
    }
    dirs
}

/// What `file` holds and which harness wrote it: `""` when no harness is
/// recognized in its first entries.
pub fn load_file(file: &Path) -> std::io::Result<(Input, &'static str)> {
    if file.to_string_lossy().ends_with("chat.history.md") {
        return Ok((Input::default(), "aider"));
    }
    let lines = read_lines(file)?;
    let harness = sniff(&lines);
    Ok((transcript_input(file, harness, lines), harness))
}

/// The harness a transcript's first entries belong to.
fn sniff(lines: &[String]) -> &'static str {
    let peek: Vec<Value> = lines
        .iter()
        .filter_map(|line| slink_format::parse(line).ok().filter(Value::is_object))
        .take(20)
        .collect();
    let kind = |entry: &Value| text(entry, "type").to_owned();
    if peek.iter().any(|entry| {
        kind(entry) == "session_meta"
            || (kind(entry) == "response_item" && !entry["payload"].is_null())
    }) {
        return "codex";
    }
    // dsh's header is a "session" line too; only it says whether the session
    // was seeded from another.
    if peek
        .first()
        .is_some_and(|entry| entry.get("isSeeded").is_some() && kind(entry) == "session")
    {
        return "dsh";
    }
    // omp's transcript is pi's with a title line of its own ahead of the header.
    if peek
        .iter()
        .any(|entry| entry.get("pad").is_some() && kind(entry) == "title")
    {
        return "omp";
    }
    if peek
        .iter()
        .any(|entry| matches!(kind(entry).as_str(), "session" | "message"))
    {
        return "pi";
    }
    if peek
        .iter()
        .any(|entry| matches!(kind(entry).as_str(), "user" | "assistant"))
    {
        return "claude-code";
    }
    ""
}

/// The head of a transcript: its working directory, a title the harness
/// recorded, and the first thing the person asked.
struct Peek {
    cwd: String,
    name: String,
    prompt: String,
}

fn peek(file: &Path, max_lines: usize) -> Peek {
    let mut found = Peek {
        cwd: String::new(),
        name: String::new(),
        prompt: String::new(),
    };
    let Ok(raw) = fs::read(file) else {
        return found;
    };
    let text_of_file = String::from_utf8_lossy(&raw);
    let mut users: Vec<Value> = Vec::new();
    let mut summary = String::new();
    for line in text_of_file.split('\n').take(max_lines) {
        let Ok(entry) = slink_format::parse(line) else {
            continue;
        };
        if found.cwd.is_empty() {
            found.cwd = text(&entry, "cwd").to_owned();
        }
        match text(&entry, "type") {
            "ai-title" if !text(&entry, "aiTitle").is_empty() => {
                found.name = text(&entry, "aiTitle").to_owned();
            }
            "summary" if summary.is_empty() => summary = text(&entry, "summary").to_owned(),
            "user" => users.push(entry),
            _ => {}
        }
    }
    if found.name.is_empty() {
        found.name = summary;
    }
    found.prompt = first_user_text(&users);
    found
}

/// The machine's newest session, wherever it was recorded: what an empty
/// import points at, so "nothing here" never reads as "nothing anywhere".
#[derive(Debug, Clone)]
pub struct Elsewhere {
    pub harness: &'static str,
    /// The project directory the session belongs to; empty when unknown.
    pub dir: String,
    pub file: PathBuf,
    /// Best effort; empty when the transcript's head gives none.
    pub title: String,
    pub recency: u128,
}

pub fn newest_anywhere(harness: &str) -> Option<Elsewhere> {
    if !harness.is_empty() && harness != "claude-code" {
        return None;
    }
    let mut best: Option<(u128, PathBuf)> = None;
    for project in sorted_entries(&claude_projects_dir()) {
        if !project.is_dir() {
            continue;
        }
        for file in transcripts_in(&project) {
            if let Some(time) = modified(&file)
                && best.as_ref().is_none_or(|best| time > best.0)
            {
                best = Some((time, file));
            }
        }
    }
    let (recency, file) = best?;
    let head = peek(&file, 40);
    Some(Elsewhere {
        harness: "claude-code",
        dir: head.cwd,
        title: if head.name.is_empty() {
            head.prompt
        } else {
            head.name
        },
        file,
        recency,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_is_named_as_claude_code_names_it() {
        assert_eq!(
            claude_slug(Path::new("/work/dev/my project_1")),
            "-work-dev-my-project-1"
        );
        // One code unit each for a letter outside ASCII, two for an emoji.
        assert_eq!(claude_slug(Path::new("/a/\u{e9}/\u{1f600}")), "-a-----");
    }

    #[test]
    fn an_agent_is_recognized_from_its_first_entries() {
        let lines = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            sniff(&lines(r#"{"type":"user","message":{}}"#)),
            "claude-code"
        );
        assert_eq!(
            sniff(&lines(r#"{"type":"session_meta","payload":{}}"#)),
            "codex"
        );
        assert_eq!(
            sniff(&lines(r#"{"type":"session","isSeeded":false}"#)),
            "dsh"
        );
        assert_eq!(
            sniff(&lines(
                "{\"type\":\"title\",\"pad\":\"\"}\n{\"type\":\"session\"}"
            )),
            "omp"
        );
        assert_eq!(sniff(&lines(r#"{"type":"session","id":"x"}"#)), "pi");
        assert_eq!(sniff(&lines("not json\n[1]\n{\"type\":\"other\"}")), "");
    }
}
