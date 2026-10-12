//! Finding a session on this machine: which harness keeps one for a
//! directory, which is newest, and what is in a file someone names.
//!
//! Each harness says where its own sessions are, in its own module. This is
//! what they share, and the questions a command asks across all of them.

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use ruzstd::decoding::StreamingDecoder;
use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
use serde_json::Value;

use crate::claude_code::first_user_text;
use crate::value::text;
use crate::{Input, aider, claude_code, codex, dsh, hermes, opencode, pi};

/// How a session that was found is read.
#[derive(Debug, Clone)]
pub enum Loader {
    /// A transcript file, with what its harness keeps beside it.
    File(PathBuf),
    /// One run in an Aider chat history, by the time it started.
    AiderRun { file: PathBuf, id: String },
    /// A session in a harness's database, by its id.
    Database { path: PathBuf, id: String },
}

/// A session that was found, not yet read.
#[derive(Debug, Clone)]
pub struct Found {
    pub harness: &'static str,
    /// When it was last written, in nanoseconds since 1970.
    pub recency: i64,
    pub loader: Loader,
}

impl Found {
    /// A transcript file as it is now.
    pub(crate) fn file(harness: &'static str, file: &Path) -> Option<Found> {
        Some(Found {
            harness,
            recency: modified(file)?,
            loader: Loader::File(file.to_owned()),
        })
    }

    pub fn load(&self) -> Result<Input, String> {
        match &self.loader {
            Loader::File(file) => {
                let lines = read_lines(file).map_err(|error| error.to_string())?;
                Ok(transcript_input(file, self.harness, lines))
            }
            Loader::AiderRun { file, id } => aider::load_run(file, id),
            Loader::Database { path, id } if self.harness == "opencode" => {
                opencode::load_session(path, id)
            }
            Loader::Database { path, id } => hermes::load_session(path, id),
        }
    }
}

/// A session offered to a person: which one it is, and enough to tell it
/// from the others before anything is imported.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub found: Found,
    pub id: String,
    /// What a terminal shows: the name, else the first prompt, else the id.
    pub title: String,
    /// The project directory it was recorded in.
    pub dir: PathBuf,
    pub file: Option<PathBuf>,
    /// The transcripts of the session's sub-agents, which an import reads
    /// along with `file`.
    pub related: Vec<PathBuf>,
    /// A title the harness recorded.
    pub name: String,
    /// The first thing the person asked.
    pub prompt: String,
    /// When it started: seconds since 1970 and nanoseconds.
    pub started: Option<(i64, u32)>,
}

impl Candidate {
    /// A candidate for a transcript file, with nothing described yet.
    pub(crate) fn file(
        harness: &'static str,
        file: &Path,
        dir: &Path,
        id: String,
    ) -> Option<Candidate> {
        if !file.is_file() {
            return None;
        }
        Some(Candidate {
            found: Found::file(harness, file)?,
            id,
            title: String::new(),
            dir: dir.to_owned(),
            file: Some(file.to_owned()),
            related: Vec::new(),
            name: String::new(),
            prompt: String::new(),
            started: None,
        })
    }

    /// Fills in what a transcript's head and tail say of it: its sub-agents'
    /// files, a recorded title, the first prompt and when it started.
    pub(crate) fn describe_file(&mut self, related: Vec<PathBuf>) {
        let Some(file) = self.file.clone() else {
            return;
        };
        self.related = related;
        let head = peek(&file, 40);
        (self.name, self.prompt, self.started) = (head.name, head.prompt, head.started);
        let latest = latest_ai_title(&file);
        if !latest.is_empty() {
            self.name = latest;
        }
    }
}

/// Where each harness keeps its sessions. A test names its own, so discovery
/// never reads or changes the user's real stores.
#[derive(Debug, Clone)]
pub struct Locations {
    pub claude: PathBuf,
    pub pi: PathBuf,
    pub omp: PathBuf,
    pub codex: PathBuf,
    pub opencode: PathBuf,
    pub hermes: PathBuf,
    pub dsh: PathBuf,
}

impl Default for Locations {
    fn default() -> Locations {
        Locations {
            claude: claude_code::store(),
            pi: pi::store("pi"),
            omp: pi::store("omp"),
            codex: codex::store(),
            opencode: opencode::store(),
            hermes: hermes::store(),
            dsh: dsh::store(),
        }
    }
}

/* ------------------------------------------------------------ what is shared */

pub(crate) fn user_home() -> PathBuf {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(name)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// A path a harness's own configuration gives, with `~` as the home.
pub(crate) fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => user_home(),
        Some(rest) if rest.starts_with(['/', '\\']) => user_home().join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

pub(crate) fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    entries.sort();
    entries
}

/// The `.jsonl` files directly inside `dir`, by name.
pub(crate) fn transcripts_in(dir: &Path) -> Vec<PathBuf> {
    sorted_entries(dir)
        .into_iter()
        .filter(|file| !file.is_dir() && file.to_string_lossy().ends_with(".jsonl"))
        .collect()
}

/// When a file was last written, in nanoseconds since 1970.
pub(crate) fn modified(file: &Path) -> Option<i64> {
    let modified = fs::metadata(file).ok()?.modified().ok()?;
    i64::try_from(modified.duration_since(UNIX_EPOCH).ok()?.as_nanos()).ok()
}

/// The most recently written of some transcripts: the first of two written
/// in the same instant.
pub(crate) fn newest_of(files: Vec<PathBuf>, harness: &'static str) -> Option<Found> {
    let mut best: Option<(i64, PathBuf)> = None;
    for file in files {
        if let Some(time) = modified(&file)
            && best.as_ref().is_none_or(|best| time > best.0)
        {
            best = Some((time, file));
        }
    }
    let (_, file) = best?;
    Found::file(harness, &file)
}

/// The most recently written `.jsonl` directly inside `dir`, and when.
pub(crate) fn newest_file(dir: &Path) -> Option<(PathBuf, i64)> {
    let mut best: Option<(PathBuf, i64)> = None;
    for file in transcripts_in(dir) {
        if let Some(time) = modified(&file)
            && best.as_ref().is_none_or(|best| time > best.1)
        {
            best = Some((file, time));
        }
    }
    best
}

/// The directory a harness keeps a transcript's large content in, beside the
/// directory of transcripts, when it has one.
pub(crate) fn blobs_beside(transcript: &Path) -> PathBuf {
    // The root is its own directory, as it is to Go's `filepath.Dir`.
    let above = |path: &Path| path.parent().unwrap_or(path).to_owned();
    above(&above(&above(transcript))).join("blobs")
}

/// Every Zstandard frame opens with these bytes.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// The most text a compressed transcript is unpacked to. A small file can be
/// made to unpack without end.
const LONGEST_TRANSCRIPT: usize = 1 << 30;

/// The largest window a frame may ask for: the most Go's decoder allows, and
/// four times what ruzstd allows unasked. Nothing is set aside for it ahead
/// of the text that fills it.
const LARGEST_WINDOW: u64 = 1 << 29;

/// The longest first line that is read; Codex's holds the whole system
/// prompt. Go's reader gives up on a longer one, so a longer one is not read
/// here either.
const LONGEST_FIRST_LINE: usize = 16 * 1024 * 1024;

/// A transcript file, whole. dsh compresses its logs, one Zstandard frame
/// for each write, and those are decoded here. A last frame that was cut off
/// mid-write gives up the lines it completed, as it does in dsh.
pub(crate) fn read_transcript(file: &Path) -> std::io::Result<Vec<u8>> {
    let raw = fs::read(file)?;
    if !raw.starts_with(&ZSTD_MAGIC) {
        return Ok(raw);
    }
    let (mut text, failure) = unpack(&raw, LONGEST_TRANSCRIPT, false);
    if let Some(failure) = failure {
        match text.iter().rposition(|byte| *byte == b'\n') {
            Some(end) => text.truncate(end + 1),
            None => {
                return Err(std::io::Error::other(format!(
                    "{}: {failure}",
                    file.display()
                )));
            }
        }
    }
    Ok(text)
}

/// The text of a compressed transcript, one frame after another: as far as
/// `limit` bytes, and with `first_line` no further than the frame the first
/// line ends in. The second value says why decoding stopped before the end,
/// when it did; the text ahead of that is what Go's decoder gives before it
/// fails, and the reason is in its words where it has them.
fn unpack(raw: &[u8], limit: usize, first_line: bool) -> (Vec<u8>, Option<String>) {
    let mut source = raw;
    let mut text = Vec::new();
    while !source.is_empty() && text.len() < limit && !(first_line && text.contains(&b'\n')) {
        let budget = limit - text.len();
        if let Err(failure) = unpack_frame(&mut source, &mut text, budget) {
            // Go stops reading at the limit, and never meets what follows.
            let met = text.len() < limit;
            text.truncate(limit);
            return (text, met.then_some(failure));
        }
    }
    (text, None)
}

/// Decodes the frame `source` starts with onto `text`, `budget` bytes of it
/// at most, and leaves `source` after it. A frame marked as one to skip adds
/// nothing.
fn unpack_frame(source: &mut &[u8], text: &mut Vec<u8>, budget: usize) -> Result<(), String> {
    let (whole, before) = (*source, text.len());
    let mut frame = match StreamingDecoder::new_with_max_window_size(&mut *source, LARGEST_WINDOW) {
        Ok(frame) => frame,
        Err(FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::SkipFrame {
            length,
            ..
        })) => {
            let Some(rest) = source.get(length as usize..) else {
                *source = &[];
                return Err("unexpected EOF".to_owned());
            };
            *source = rest;
            return Ok(());
        }
        Err(FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::BadMagicNumber(_))) => {
            return Err("invalid input: magic number mismatch".to_owned());
        }
        Err(error) => return Err(cut_short(source, &error)),
    };
    let header = frame.decoder.bytes_read_from_source() as usize;
    let read = frame.by_ref().take(budget as u64).read_to_end(text);
    let decoder = frame.into_frame_decoder();
    let failure = match read {
        Err(error) => cut_short(source, &error),
        // The budget ran out first.
        Ok(_) if !decoder.is_finished() || decoder.can_collect() > 0 => return Ok(()),
        Ok(_)
            if decoder
                .get_checksum_from_data()
                .is_some_and(|sum| Some(sum) != decoder.get_calculated_checksum()) =>
        {
            "CRC check failed".to_owned()
        }
        Ok(_) => return Ok(()),
    };
    // ruzstd holds back a frame's last window of text until the frame is
    // complete, and one that fails gives it up. Go's decoder hands over each
    // block as it decodes it, so the blocks ahead of the failure are read
    // again here.
    text.truncate(before);
    text.extend(whole_blocks(whole, header, decoder.blocks_decoded()));
    Err(failure)
}

/// Why a decoder stopped: Go's words when the file ended before the frame
/// did, the decoder's own otherwise.
fn cut_short(rest: &[u8], error: &dyn std::fmt::Display) -> String {
    if rest.is_empty() {
        "unexpected EOF".to_owned()
    } else {
        error.to_string()
    }
}

/// The text of the first `decoded` blocks of a frame whose header is
/// `header` bytes long. A frame's last block is not among them: Go's decoder
/// gives that one only once the checksum after it holds.
fn whole_blocks(frame: &[u8], header: usize, decoded: usize) -> Vec<u8> {
    let (mut end, mut last) = (header, None);
    for _ in 0..decoded {
        let Some(&[first, second, third]) = frame.get(end..end + 3) else {
            break;
        };
        if first & 1 == 1 {
            break;
        }
        // Three bytes: whether it is the last block, its kind and its size.
        // A block of one repeated byte holds that byte alone.
        let size = u32::from_le_bytes([first, second, third, 0]) >> 3;
        last = Some(end);
        end += 3 + if (first >> 1) & 3 == 1 {
            1
        } else {
            size as usize
        };
    }
    let (Some(last), Some(blocks)) = (last, frame.get(..end)) else {
        return Vec::new();
    };
    // The same blocks as a frame of their own: the last of them marked as
    // the frame's last, and a checksum's four bytes after it when the frame
    // has one. ruzstd reads a checksum and leaves checking it to its caller.
    let mut ended = blocks.to_vec();
    ended[last] |= 1;
    if frame[4] & 4 != 0 {
        ended.extend([0; 4]);
    }
    let mut text = Vec::new();
    if let Ok(mut blocks) =
        StreamingDecoder::new_with_max_window_size(ended.as_slice(), LARGEST_WINDOW)
    {
        let _ = blocks.read_to_end(&mut text);
    }
    text
}

/// Bytes as text. Each byte that is not part of a UTF-8 character reads as
/// U+FFFD, one for one, as Go's JSON decoder reads it.
pub(crate) fn lossy(raw: &[u8]) -> std::borrow::Cow<'_, str> {
    if let Ok(text) = str::from_utf8(raw) {
        return text.into();
    }
    let mut text = String::with_capacity(raw.len());
    for chunk in raw.utf8_chunks() {
        text.push_str(chunk.valid());
        text.extend(chunk.invalid().iter().map(|_| '\u{fffd}'));
    }
    text.into()
}

/// A transcript's lines, blank ones left out.
pub fn read_lines(file: &Path) -> std::io::Result<Vec<String>> {
    let raw = read_transcript(file)?;
    Ok(lossy(&raw)
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect())
}

/// The first line of a transcript, as JSON: where a harness says what the
/// session is. `Null` when the file has none.
pub(crate) fn first_json_line(file: &Path) -> Value {
    first_line(file)
        .and_then(|line| slink_format::parse(&lossy(&line)).ok())
        .filter(Value::is_object)
        .unwrap_or(Value::Null)
}

/// The first line of a file, and no more of the file than that. A log is
/// taken for compressed by its name here, `.zstd`, and not by its first
/// bytes, as Go takes it: a file that is one and not the other has no first
/// line to read.
fn first_line(file: &Path) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    if file.to_string_lossy().ends_with(".zstd") {
        let raw = fs::read(file).ok()?;
        if !raw.starts_with(&ZSTD_MAGIC) {
            return None;
        }
        line = unpack(&raw, LONGEST_FIRST_LINE, true).0;
    } else {
        let start = fs::File::open(file).ok()?.take(LONGEST_FIRST_LINE as u64);
        BufReader::new(start).read_until(b'\n', &mut line).ok()?;
    }
    match line.iter().position(|byte| *byte == b'\n') {
        Some(end) => line.truncate(end),
        None if line.len() >= LONGEST_FIRST_LINE => return None,
        None => {}
    }
    Some(line)
}

/// The file's name without `.jsonl`.
pub(crate) fn stem(file: &Path) -> String {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    name.strip_suffix(".jsonl").unwrap_or(&name).to_owned()
}

/// A transcript's lines with what its harness keeps beside them: the store
/// its images are in, and the sessions of its sub-agents.
pub(crate) fn transcript_input(file: &Path, harness: &str, lines: Vec<String>) -> Input {
    match harness {
        "claude-code" => claude_code::input(file, lines),
        "pi" | "omp" => pi::input(file, lines, harness),
        "codex" => codex::input(file, lines),
        "dsh" => dsh::input(file, lines),
        _ => Input {
            lines,
            fallback: stem(file),
            blobs: blobs_beside(file),
            ..Input::default()
        },
    }
}

/// The head of a transcript: its working directory, a title the harness
/// recorded, the first thing the person asked, and when it started.
pub(crate) struct Peek {
    pub cwd: String,
    pub name: String,
    pub prompt: String,
    pub started: Option<(i64, u32)>,
}

/// Reads the first `max_lines` lines of a transcript in any of the shapes
/// the line-based harnesses write: Claude Code's entries, pi's messages and
/// Codex's response items.
pub(crate) fn peek(file: &Path, max_lines: usize) -> Peek {
    let mut found = Peek {
        cwd: String::new(),
        name: String::new(),
        prompt: String::new(),
        started: None,
    };
    let Ok(raw) = fs::read(file) else {
        return found;
    };
    let text_of_file = lossy(&raw);
    let mut users: Vec<Value> = Vec::new();
    let mut summary = String::new();
    for line in text_of_file.split('\n').take(max_lines) {
        let Ok(entry) = slink_format::parse(line) else {
            continue;
        };
        let payload = &entry["payload"];
        if found.cwd.is_empty() {
            found.cwd = match text(&entry, "cwd") {
                // Codex puts it in the payload of its first line.
                "" => text(payload, "cwd").to_owned(),
                cwd => cwd.to_owned(),
            };
        }
        if found.started.is_none() {
            let stamp = match text(&entry, "timestamp") {
                "" => text(payload, "timestamp"),
                stamp => stamp,
            };
            found.started = slink_format::instant(stamp);
        }
        match text(&entry, "type") {
            "ai-title" if !text(&entry, "aiTitle").is_empty() => {
                found.name = text(&entry, "aiTitle").to_owned();
            }
            "summary" if summary.is_empty() => summary = text(&entry, "summary").to_owned(),
            "user" => users.push(entry),
            "message" if entry["message"]["role"] == "user" => {
                users.push(serde_json::json!({"type": "user", "message": entry["message"]}));
            }
            "response_item" if payload["role"] == "user" => {
                let said: String = payload["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|part| part["type"] == "input_text" || part["type"] == "text")
                    .map(|part| text(part, "text"))
                    .collect();
                users.push(serde_json::json!({"type": "user", "message": {"content": said}}));
            }
            _ => {}
        }
    }
    if found.name.is_empty() {
        found.name = summary;
    }
    found.prompt = first_user_text(&users);
    found
}

/// The lines in the last 512 KiB of a transcript, where a harness rewrites a
/// session's title as work goes on.
pub(crate) fn tail_lines(file: &Path) -> Vec<String> {
    let Ok(raw) = fs::read(file) else {
        return Vec::new();
    };
    let from = raw.len().saturating_sub(512 * 1024);
    lossy(&raw[from..]).split('\n').map(str::to_owned).collect()
}

/// The most recent title Claude Code recorded, or `""`.
pub(crate) fn latest_ai_title(file: &Path) -> String {
    for line in tail_lines(file).iter().rev() {
        if !line.contains("\"ai-title\"") {
            continue;
        }
        if let Ok(entry) = slink_format::parse(line)
            && text(&entry, "type") == "ai-title"
            && !text(&entry, "aiTitle").is_empty()
        {
            return text(&entry, "aiTitle").to_owned();
        }
    }
    String::new()
}

/* -------------------------------------------------- what a command asks */

/// The directories a session for `cwd` may be recorded under: `cwd` itself,
/// then each parent, because an agent is usually started at a project's root
/// and used from inside it. The walk stops before the home directory and
/// before the filesystem root; a session started in either is not this
/// project's. Both still count when `cwd` is that directory itself.
pub fn project_dirs(cwd: &Path) -> Vec<PathBuf> {
    let home = user_home();
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

/// The harness and id of the session whose agent is running this process,
/// when its harness says which that is. Only dsh does.
pub fn active_session() -> Option<(&'static str, String)> {
    dsh::active_id().map(|id| ("dsh", id))
}

/// The session a harness last wrote for `cwd`. With no harness named, the
/// newest across all of them.
pub fn latest(harness: &str, cwd: &Path) -> Option<Found> {
    // A command an agent runs in its own shell tool is about the session
    // running it, whichever was written to last.
    if let Some(found) = dsh::active().filter(|_| harness.is_empty() || harness == "dsh") {
        return Some(found);
    }
    let of = |harness: &str| match harness {
        "claude-code" => claude_code::latest(cwd),
        "pi" | "omp" => pi::latest(cwd, harness),
        "codex" => codex::latest(cwd),
        "opencode" => opencode::latest(cwd),
        "hermes" => hermes::latest(cwd),
        "dsh" => dsh::latest(cwd),
        "aider" => aider::latest(cwd),
        _ => None,
    };
    if !harness.is_empty() {
        return of(harness);
    }
    let mut best: Option<Found> = None;
    for harness in [
        "claude-code",
        "pi",
        "omp",
        "codex",
        "opencode",
        "hermes",
        "dsh",
        "aider",
    ] {
        if let Some(found) = of(harness)
            && best
                .as_ref()
                .is_none_or(|best| found.recency > best.recency)
        {
            best = Some(found);
        }
    }
    best
}

/// A session in a harness's database, by the id the harness gave it.
pub fn latest_by_id(harness: &str, id: &str) -> Option<Found> {
    let (harness, path) = match harness {
        "opencode" => ("opencode", opencode::store()),
        "hermes" => ("hermes", hermes::store()),
        _ => return None,
    };
    Some(Found {
        harness,
        recency: 0,
        loader: Loader::Database {
            path,
            id: id.to_owned(),
        },
    })
}

/// The transcript of a pasted session id, for one working directory: Claude
/// Code and dsh both name a session's file or directory by its id.
pub fn transcript_by_id(cwd: &Path, id: &str) -> Option<PathBuf> {
    claude_code::transcript(cwd, id).or_else(|| dsh::transcript(cwd, id))
}

/// The Aider run that started at the time `id` names, in the chat history of
/// one directory.
pub fn aider_run(cwd: &Path, id: &str) -> Option<Found> {
    aider::run(cwd, id)
}

/// What `file` holds and which harness wrote it: `""` when no harness is
/// recognized in its first entries.
pub fn load_file(file: &Path) -> Result<(Input, &'static str), String> {
    if aider::is_chat(file) {
        return Ok((aider::load_run(file, "")?, "aider"));
    }
    let lines = read_lines(file).map_err(|error| error.to_string())?;
    let harness = sniff(&lines);
    Ok((transcript_input(file, harness, lines), harness))
}

/// The harness a transcript's first entries belong to.
fn sniff(lines: &[String]) -> &'static str {
    let head: Vec<Value> = lines
        .iter()
        // A line that is `null` counts as an entry with nothing in it, as
        // it does to Go, and so can be the first one.
        .filter_map(|line| {
            slink_format::parse(line)
                .ok()
                .filter(|entry| entry.is_object() || entry.is_null())
        })
        .take(20)
        .collect();
    // In this order: dsh's header is a "session" line as pi's is, and omp's
    // transcript is pi's with a title line of its own ahead of the header.
    if codex::recognizes(&head) {
        "codex"
    } else if dsh::recognizes(&head) {
        "dsh"
    } else if pi::recognizes(&head, "omp") {
        "omp"
    } else if pi::recognizes(&head, "pi") {
        "pi"
    } else if claude_code::recognizes(&head) {
        "claude-code"
    } else {
        ""
    }
}

/// The machine's newest session, wherever it was recorded: what an empty
/// import points at, so "nothing here" never reads as "nothing anywhere".
#[derive(Debug, Clone)]
pub struct Elsewhere {
    pub harness: &'static str,
    /// The project directory the session belongs to; empty when unknown.
    pub dir: String,
    /// The transcript, for a harness that keeps files.
    pub file: Option<PathBuf>,
    /// Best effort; empty when the harness cannot say cheaply.
    pub title: String,
    pub recency: i64,
}

pub fn newest_anywhere(harness: &str) -> Option<Elsewhere> {
    let of = |harness: &str| match harness {
        "claude-code" => claude_code::anywhere(),
        "pi" | "omp" => pi::anywhere(harness),
        "codex" => codex::anywhere(),
        "opencode" => opencode::anywhere(),
        "hermes" => hermes::anywhere(),
        "dsh" => dsh::anywhere(),
        _ => None,
    };
    if !harness.is_empty() {
        return of(harness);
    }
    let mut best: Option<Elsewhere> = None;
    for harness in [
        "claude-code",
        "pi",
        "omp",
        "codex",
        "opencode",
        "hermes",
        "dsh",
    ] {
        if let Some(found) = of(harness)
            && best
                .as_ref()
                .is_none_or(|best| found.recency > best.recency)
        {
            best = Some(found);
        }
    }
    best
}

/// The sessions recorded for `cwd` and its parents, newest first, at most
/// `limit` of them. An `id` names one session and is matched before the
/// limit, so an older session named outright is still reached. The second
/// value says which harnesses could not be read.
pub fn recent(
    locations: &Locations,
    harness: &str,
    cwd: &Path,
    id: &str,
    limit: usize,
) -> (Vec<Candidate>, Vec<String>) {
    let limit = if limit == 0 { 30 } else { limit };
    let dirs = project_dirs(cwd);
    let wanted = |name: &str| harness.is_empty() || harness == name;
    let has = |store: &Path| !store.as_os_str().is_empty();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let mut gather = |found: Result<Vec<Candidate>, String>| match found {
        Ok(found) => candidates.extend(found),
        Err(problem) => problems.push(problem),
    };
    if wanted("claude-code") && has(&locations.claude) {
        gather(claude_code::candidates(&locations.claude, &dirs, id));
    }
    if wanted("pi") && has(&locations.pi) {
        gather(pi::candidates(&locations.pi, &dirs, id, "pi"));
    }
    if wanted("omp") && has(&locations.omp) {
        gather(pi::candidates(&locations.omp, &dirs, id, "omp"));
    }
    if wanted("codex") && has(&locations.codex) {
        gather(codex::candidates(&locations.codex, &dirs, id));
    }
    if wanted("dsh") && has(&locations.dsh) {
        gather(dsh::candidates(&locations.dsh, &dirs, id));
    }
    if wanted("aider") {
        gather(aider::candidates(&dirs, id));
    }
    if wanted("opencode") {
        gather(opencode::candidates(&locations.opencode, &dirs, id, limit));
    }
    if wanted("hermes") {
        gather(hermes::candidates(&locations.hermes, &dirs, id, limit));
    }
    let key = |candidate: &Candidate| format!("{}:{}", candidate.found.harness, candidate.id);
    candidates.sort_by(|a, b| {
        b.found
            .recency
            .cmp(&a.found.recency)
            .then_with(|| key(a).cmp(&key(b)))
    });
    if id.is_empty() {
        candidates.truncate(limit);
    }
    for candidate in &mut candidates {
        match candidate.found.harness {
            "claude-code" => claude_code::describe(candidate),
            "pi" | "omp" => pi::describe(candidate),
            "codex" => codex::describe(candidate),
            "dsh" => dsh::describe(candidate),
            "aider" => aider::describe(candidate),
            // A harness with a database stores its own titles.
            _ => candidate.name = candidate.title.clone(),
        }
        if candidate.title.is_empty() {
            candidate.title = candidate.id.clone();
        }
    }
    (candidates, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blob_store_is_looked_for_three_directories_up() {
        for (transcript, store) in [
            (
                "/home/me/.omp/agent/sessions/-work/a.jsonl",
                "/home/me/.omp/agent/blobs",
            ),
            // The root is its own directory.
            ("/tmp/a.jsonl", "/blobs"),
            ("/a.jsonl", "/blobs"),
            ("a.jsonl", "blobs"),
        ] {
            assert_eq!(
                blobs_beside(Path::new(transcript)),
                Path::new(store),
                "{transcript}"
            );
        }
    }

    #[test]
    fn an_empty_first_entry_does_not_hide_whose_transcript_it_is() {
        let lines = |text: &[&str]| {
            text.iter()
                .map(|line| (*line).to_owned())
                .collect::<Vec<_>>()
        };
        let header = r#"{"type":"session","version":4,"id":"s","isSeeded":false}"#;
        assert_eq!(sniff(&lines(&[header])), "dsh");
        // dsh is told by its first entry alone, and `null` is an entry.
        assert_ne!(sniff(&lines(&["null", header])), "dsh");
        // A line that is not an entry at all is passed over.
        assert_eq!(sniff(&lines(&["[]", "not json", header])), "dsh");
    }

    #[test]
    fn an_agent_is_recognized_from_its_first_entries() {
        let lines = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            sniff(&lines(r#"{"type":"user","message":{}}"#)),
            "claude-code"
        );
        assert_eq!(sniff(&lines("not json\n[1]\n{\"type\":\"other\"}")), "");
    }

    #[test]
    fn a_compressed_log_is_read_as_its_lines() {
        // A dsh fixture's log, stored as dsh stores it.
        let store =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/import/dsh/v4-run/sessions");
        let mut logs = Vec::new();
        let mut pending = vec![store];
        while let Some(dir) = pending.pop() {
            for entry in sorted_entries(&dir) {
                if entry.is_dir() {
                    pending.push(entry);
                } else if entry.to_string_lossy().ends_with(".jsonl.zstd") {
                    logs.push(entry);
                }
            }
        }
        assert!(!logs.is_empty(), "the fixture has a compressed log");
        for log in logs {
            let lines = read_lines(&log).unwrap();
            assert!(lines.len() > 1, "{}", log.display());
            for line in &lines {
                assert!(
                    slink_format::parse(line).is_ok_and(|entry| entry.is_object()),
                    "{line}"
                );
            }
            assert_eq!(
                first_json_line(&log),
                slink_format::parse(&lines[0]).unwrap()
            );
        }
    }

    /// Text as one Zstandard frame, with a checksum after it.
    fn frame(text: &[u8]) -> Vec<u8> {
        ruzstd::encoding::compress_to_vec(text, ruzstd::encoding::CompressionLevel::Fastest)
    }

    #[test]
    fn frames_are_read_one_after_another() {
        let packed = [frame(b"one\n"), frame(b"two\nthr"), frame(b"ee\n")].concat();
        assert_eq!(
            unpack(&packed, usize::MAX, false),
            (b"one\ntwo\nthree\n".to_vec(), None)
        );
        // No further than the frame the first line ends in, or than a limit.
        assert_eq!(unpack(&packed, usize::MAX, true).0, b"one\n");
        assert_eq!(unpack(&packed, 6, false), (b"one\ntw".to_vec(), None));
        // A frame marked as one to skip holds nothing to read.
        let skipped = [&[0x50, 0x2a, 0x4d, 0x18, 3, 0, 0, 0][..], b"xyz"].concat();
        let around = [frame(b"one\n"), skipped.clone(), frame(b"two\n")].concat();
        assert_eq!(
            unpack(&around, usize::MAX, false),
            (b"one\ntwo\n".to_vec(), None)
        );
        // What follows the last frame and is not one stops the reading, in
        // the words Go's decoder has for it.
        for (after, why) in [
            (&b"ab"[..], "unexpected EOF"),
            (b"plain text\n", "invalid input: magic number mismatch"),
            (&skipped[..9], "unexpected EOF"),
            (&frame(b"two\n")[..7], "unexpected EOF"),
        ] {
            assert_eq!(
                unpack(&[&frame(b"one\n"), after].concat(), usize::MAX, false),
                (b"one\n".to_vec(), Some(why.to_owned())),
                "{after:?}"
            );
        }
    }

    #[test]
    fn a_frame_cut_off_gives_the_blocks_ahead_of_the_cut() {
        // More text than three blocks hold, so that the frame has several.
        let text: Vec<u8> = (0..40_000)
            .flat_map(|line| format!("line {line:05}\n").into_bytes())
            .collect();
        let ahead = [frame(b"first\n"), frame(&text)].concat();
        assert_eq!(
            unpack(&ahead, usize::MAX, false).0,
            [b"first\n", &text[..]].concat()
        );
        let read = |packed: &[u8]| {
            let (text, failure) = unpack(packed, usize::MAX, false);
            (text[b"first\n".len()..].to_vec(), failure)
        };
        // Cut inside the last block: the blocks before it are whole.
        let (kept, failure) = read(&ahead[..ahead.len() - 40]);
        assert_eq!(failure.as_deref(), Some("unexpected EOF"));
        assert!(
            kept.len() > 128 * 1024 && kept.len() < text.len(),
            "{}",
            kept.len()
        );
        assert_eq!(kept, text[..kept.len()]);
        // The last block is not given until the checksum after it holds: not
        // when the file ends inside the checksum, nor when it is wrong.
        assert_eq!(
            read(&ahead[..ahead.len() - 2]),
            (kept.clone(), Some("unexpected EOF".to_owned()))
        );
        let mut wrong = ahead.clone();
        *wrong.last_mut().unwrap() ^= 0xff;
        assert_eq!(read(&wrong), (kept, Some("CRC check failed".to_owned())));
        // A frame of one block has nothing ahead of a cut.
        let packed = frame(b"first\n");
        assert_eq!(
            unpack(&packed[..packed.len() - 5], usize::MAX, false),
            (Vec::new(), Some("unexpected EOF".to_owned()))
        );
    }

    #[test]
    fn a_byte_that_is_not_text_reads_as_one_replacement_character() {
        assert_eq!(lossy(b"plain \xc3\xa9"), "plain \u{e9}");
        // A character cut short gives one for each of its bytes, as in Go.
        assert_eq!(
            lossy(b"a\xe2\x82 b\xff c\xf0\x9f\x98"),
            "a\u{fffd}\u{fffd} b\u{fffd} c\u{fffd}\u{fffd}\u{fffd}"
        );
    }

    #[test]
    fn a_home_relative_path_is_expanded() {
        let home = user_home();
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/x/y"), home.join("x/y"));
        assert_eq!(expand_home("/abs/~"), PathBuf::from("/abs/~"));
        assert_eq!(expand_home("~user/x"), PathBuf::from("~user/x"));
    }
}
