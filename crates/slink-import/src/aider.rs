//! Aider. It appends every run to one Markdown file per project,
//! `.aider.chat.history.md`, and marks each line by who it is from: `#### `
//! is what the person typed, `> ` is Aider's own notice (the model in use, an
//! edit it applied, a commit, a command's question), and anything else is the
//! model's reply. A run starts at a `# aider chat started at` line. The file
//! holds no tool calls, since Aider's edits are text in the reply, and no
//! times after the first, so a reply becomes an `llm_call` with the typed
//! lines before it as its input, and Aider's notices are kept where they
//! appeared as the evidence of what it did. The notices ahead of the first
//! typed line are the run's setup.
//!
//! The marks are Aider's only structure. A reply line that itself begins with
//! `> ` or `#### ` reads as a notice or as typed text, here as in Aider's own
//! reader.
//!
//! Aider has no session store. The history is in the directory Aider was
//! started in, the git root when there is one, and what was typed at its
//! prompt is in `.aider.input.history` beside it.

use std::cell::Cell;
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Timelike, Utc};
use serde_json::{Map, Value, json};

use crate::Input;
use crate::discover::{Candidate, Found, Loader, modified};
use crate::value::text;

pub(crate) const PORTED: bool = true;

const LABEL: &str = "session-import@0.1.0";

/// The name Aider gives a project's chat history.
const CHAT_NAME: &str = ".aider.chat.history.md";

/// What a run opens with. The time after it is the local time of the machine
/// Aider ran on.
const OPENING: &str = "# aider chat started at ";

/* ------------------------------------------------------------ clock times */

/// The zone Aider's clock times are read in. Aider writes them without one;
/// they are the local time of the machine, which is where a session is
/// imported.
#[derive(Debug, Clone, Copy)]
enum Zone {
    Local,
    Utc,
}

impl Zone {
    /// How many seconds the zone is ahead of UTC at an instant.
    fn ahead(self, unix: i64) -> i64 {
        match self {
            Zone::Utc => 0,
            Zone::Local => DateTime::from_timestamp(unix, 0).map_or(0, |at| {
                i64::from(
                    Local
                        .offset_from_utc_datetime(&at.naive_utc())
                        .local_minus_utc(),
                )
            }),
        }
    }
}

thread_local! {
    /// Set while this thread reads Aider's clock times as UTC.
    static AS_UTC: Cell<bool> = const { Cell::new(false) };
}

/// Runs `work` with Aider's clock times read as UTC on this thread, and not
/// in the zone of the machine. A fixture means the same times wherever its
/// test runs; nothing else has a reason to call this.
pub fn in_utc<T>(work: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            AS_UTC.set(self.0);
        }
    }
    let _restore = Restore(AS_UTC.replace(true));
    work()
}

/// The zone the functions other modules call read in.
fn zone() -> Zone {
    if AS_UTC.get() { Zone::Utc } else { Zone::Local }
}

/// When something happened.
type Instant = DateTime<Utc>;

/// The instant a clock stands for when it reads `clock`, in seconds counted
/// as if the clock showed UTC, in a zone that is `ahead(at)` seconds ahead of
/// UTC at the instant `at`.
///
/// This is Go's `time.Date`: the zone is asked how far ahead it was at the
/// reading taken as UTC, then again at the instant that answer gives, and the
/// second answer stands. Away from a clock change the two agree. At one, the
/// answer depends on which side of the change the reading taken as UTC falls,
/// so a zone ahead of UTC reads a time its clocks skipped with the offset
/// from before the change and a time they showed twice with the offset after
/// it, and a zone behind UTC does the reverse.
fn instant_of(clock: i64, ahead: impl Fn(i64) -> i64) -> i64 {
    clock - ahead(clock - ahead(clock))
}

/// Takes a number of `least` to `most` digits off the front of `rest`.
fn number(rest: &mut &[u8], least: usize, most: usize) -> Option<u32> {
    let digits = rest
        .iter()
        .take(most)
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits < least {
        return None;
    }
    let (number, after) = rest.split_at(digits);
    *rest = after;
    Some(
        number
            .iter()
            .fold(0, |value, digit| value * 10 + u32::from(digit - b'0')),
    )
}

/// Takes one byte off the front of `rest`, which must be `wanted`.
fn literal(rest: &mut &[u8], wanted: u8) -> Option<()> {
    let (first, after) = rest.split_first()?;
    if *first != wanted {
        return None;
    }
    *rest = after;
    Some(())
}

/// Reads one of Aider's clock times in a zone: `2026-10-06 09:00:07`, with a
/// fraction of a second in the input history. It takes what Go's layout
/// `2006-01-02 15:04:05.999999` takes and nothing else: any number of spaces
/// before the hour, an hour of one digit, a comma for the point, and digits
/// past the ninth, which are dropped. The time a run opens with has none of
/// that: `opening` passes on only the plain form.
fn clock_time(written: &str, zone: Zone) -> Option<Instant> {
    let mut rest = written.as_bytes();
    let year = number(&mut rest, 4, 4)?;
    literal(&mut rest, b'-')?;
    let month = number(&mut rest, 2, 2)?;
    literal(&mut rest, b'-')?;
    let day = number(&mut rest, 2, 2)?;
    let spaces = rest.iter().take_while(|byte| **byte == b' ').count();
    if spaces == 0 {
        return None;
    }
    rest = &rest[spaces..];
    let hour = number(&mut rest, 1, 2)?;
    literal(&mut rest, b':')?;
    let minute = number(&mut rest, 2, 2)?;
    literal(&mut rest, b':')?;
    let second = number(&mut rest, 2, 2)?;
    let mut nanos = 0;
    if rest.len() >= 2 && matches!(rest[0], b'.' | b',') && rest[1].is_ascii_digit() {
        let fraction = &rest[1..];
        let digits = fraction
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        nanos = (0..9).fold(0, |nanos, place| {
            let digit = if place < digits {
                u32::from(fraction[place] - b'0')
            } else {
                0
            };
            nanos * 10 + digit
        });
        rest = &fraction[digits..];
    }
    if !rest.is_empty() {
        return None;
    }
    // A day the month does not have, or an hour the day does not, is no time.
    let clock = NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)?
        .and_hms_opt(hour, minute, second)?
        .and_utc()
        .timestamp();
    DateTime::from_timestamp(instant_of(clock, |at| zone.ahead(at)), nanos)
}

/// Whether an instant is Go's zero time, the first of the year 1, which the
/// Go importer cannot tell from no time at all.
fn unset(at: Instant) -> bool {
    at.timestamp() == -62_135_596_800 && at.timestamp_subsec_nanos() == 0
}

/// An instant as a document gives it: in UTC, to the millisecond.
fn iso(at: Instant) -> String {
    // A year before 1 has its sign ahead of four digits, as Go writes it.
    let sign = if at.year() < 0 { "-" } else { "" };
    format!(
        "{sign}{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        at.year().unsigned_abs(),
        at.month(),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.timestamp_subsec_millis(),
    )
}

/// An instant in nanoseconds since 1970, wrapping outside the years a 64-bit
/// count of them reaches, as Go's does.
fn unix_nanos(at: Instant) -> i64 {
    at.timestamp()
        .wrapping_mul(1_000_000_000)
        .wrapping_add(i64::from(at.timestamp_subsec_nanos()))
}

/* -------------------------------------------------------- reading the marks */

/// A file's bytes as text. Each byte that is not part of a UTF-8 character
/// reads as one U+FFFD, which is how many Go writes for it.
fn decoded(raw: &[u8]) -> String {
    let mut decoded = String::with_capacity(raw.len());
    for chunk in raw.utf8_chunks() {
        decoded.push_str(chunk.valid());
        decoded.extend(chunk.invalid().iter().map(|_| '\u{fffd}'));
    }
    decoded
}

/// A character Go's regular expressions count as space.
fn blank(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

/// The time a run started, as Aider wrote it, when `line` opens a run.
fn opening(line: &str) -> Option<&str> {
    let rest = line.strip_prefix(OPENING)?;
    let stamp = rest.get(..19)?;
    let shaped = stamp.bytes().enumerate().all(|(at, byte)| match at {
        4 | 7 => byte == b'-',
        10 => byte == b' ',
        13 | 16 => byte == b':',
        _ => byte.is_ascii_digit(),
    });
    (shaped && rest[19..].chars().all(blank)).then_some(stamp)
}

/// The name of a run inside its history file: the time it started, as Aider
/// wrote it.
fn run_id(stamp: &str) -> String {
    stamp.replacen(' ', "T", 1)
}

/// Aider's version, from the notice it announces itself with when a run
/// starts.
fn version_in(notice: &str) -> Option<&str> {
    let rest = notice.strip_prefix("Aider v")?;
    let end = rest.find(blank).unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// The model, from the notice that names it when a run starts and again when
/// the model is switched.
fn model_in(notice: &str) -> Option<&str> {
    let rest = notice
        .strip_prefix("Main model: ")
        .or_else(|| notice.strip_prefix("Model: "))?;
    let end = rest.find(blank).unwrap_or(rest.len());
    // The name runs to a space or to the end of the notice.
    if end == 0 || !(rest[end..].is_empty() || rest[end..].starts_with(' ')) {
        return None;
    }
    let name = &rest[..end];
    // A comma after it is punctuation.
    Some(
        name.strip_suffix(',')
            .filter(|short| !short.is_empty())
            .unwrap_or(name),
    )
}

/// Who a stretch of lines is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    Person,
    Model,
    Aider,
}

/// A stretch of lines from one writer.
#[derive(Debug)]
struct Block<'a> {
    writer: Writer,
    lines: Vec<&'a str>,
}

/// A line without its mark, the space after the mark, or the spaces Aider
/// ends a line with.
fn unmarked<'a>(line: &'a str, mark: &str) -> &'a str {
    let rest = line.strip_prefix(mark).unwrap_or(line);
    rest.strip_prefix(' ').unwrap_or(rest).trim_end_matches(' ')
}

/// Splits a run into blocks. Aider puts a blank line before what the person
/// typed and around a reply; blank lines inside a reply are part of it.
fn blocks(lines: &[String]) -> Vec<Block<'_>> {
    fn add<'a>(blocks: &mut Vec<Block<'a>>, writer: Writer, line: &'a str) {
        match blocks.last_mut() {
            Some(block) if block.writer == writer => block.lines.push(line),
            _ => blocks.push(Block {
                writer,
                lines: vec![line],
            }),
        }
    }
    let mut blocks: Vec<Block> = Vec::new();
    // Inside one typed entry, which a blank line ends.
    let mut typing = false;
    for line in lines {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with("#### ") || line == "####" {
            let typed = unmarked(line, "####");
            if typing {
                add(&mut blocks, Writer::Person, typed);
            } else {
                blocks.push(Block {
                    writer: Writer::Person,
                    lines: vec![typed],
                });
            }
            typing = true;
        } else if line.starts_with("> ") || line == ">" {
            add(&mut blocks, Writer::Aider, unmarked(line, ">"));
            typing = false;
        } else if line.trim().is_empty() {
            if let Some(block) = blocks.last_mut()
                && block.writer == Writer::Model
                && !typing
            {
                block.lines.push("");
            }
            typing = false;
        } else {
            add(&mut blocks, Writer::Model, line);
            typing = false;
        }
    }
    blocks
}

/// What a block says, as one text.
fn block_text(lines: &[&str]) -> String {
    lines.join("\n").trim().to_owned()
}

/* -------------------------------------------------------------- the import */

pub(crate) fn import(input: &Input) -> Result<Option<Value>, String> {
    to_run(input, zone()).map(Some)
}

/// A run as it is read: its spans so far, and what the person has typed that
/// nothing has answered yet.
struct Reading {
    spans: Vec<Value>,
    sequence: u64,
    pending: Vec<Value>,
    /// When the first waiting entry was typed.
    pending_at: Option<Instant>,
}

impl Reading {
    fn next(&mut self) -> String {
        self.sequence += 1;
        format!("s{}", self.sequence)
    }

    /// Adds a span that answers what was waiting, begun when the first
    /// waiting entry was typed if the input history says when that was.
    fn add_timed(&mut self, mut span: Value) {
        if let Some(at) = self.pending_at.take().filter(|at| !unset(*at)) {
            span["started_at"] = json!(iso(at));
        }
        self.spans.push(span);
    }

    /// Records typed entries that got no reply: a command to Aider itself,
    /// or a prompt that was interrupted.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let first = text(&self.pending[0]["content"][0], "text");
        let name = first.split('\n').next().unwrap_or_default().to_owned();
        let span = json!({
            "id": self.next(),
            "parent_id": "root",
            "type": "custom",
            "name": name,
            "input": {"messages": std::mem::take(&mut self.pending)},
        });
        self.add_timed(span);
    }
}

/// Reads one run. The input's lines are that run's lines, blank ones
/// included, from its `# aider chat started at` line on. Its session carries
/// what the file around it says: the project directory, and the entries of
/// Aider's input history that fall in the run, which give the time each one
/// was typed.
fn to_run(input: &Input, zone: Zone) -> Result<Value, String> {
    let not_a_run =
        || "not an aider run: it does not begin with a \"# aider chat started at\" line".to_owned();
    let (first, lines) = input.lines.split_first().ok_or_else(not_a_run)?;
    let stamp = opening(first).ok_or_else(not_a_run)?;
    let started = clock_time(stamp, zone)
        .filter(|at| !unset(*at))
        .ok_or_else(not_a_run)?;

    let mut reading = Reading {
        spans: vec![json!({
            "id": "root",
            "parent_id": null,
            "type": "agent",
            "status": "ok",
            "started_at": iso(started),
        })],
        sequence: 0,
        pending: Vec::new(),
        pending_at: None,
    };

    // The times the person's entries were typed, in order, for the entries
    // whose text is in the input history exactly as it is in the chat.
    let typed = input.session["inputs"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let mut cursor = 0;
    let mut typed_at = |said: &str| {
        for (at, entry) in typed.iter().enumerate().skip(cursor) {
            if text(entry, "text").trim() == said
                && let Some(when) = clock_time(text(entry, "at"), zone)
            {
                cursor = at + 1;
                return Some(when);
            }
        }
        None
    };

    let mut version = String::new();
    let mut model = "unknown".to_owned();
    let mut prompt = String::new();
    // Something has been typed or answered.
    let mut begun = false;
    for block in blocks(lines) {
        let said = block_text(&block.lines);
        match block.writer {
            Writer::Person => {
                begun = true;
                if let Some(at) = typed_at(&said)
                    && reading.pending.is_empty()
                {
                    reading.pending_at = Some(at);
                }
                if prompt.is_empty() && !said.is_empty() && !said.starts_with('/') {
                    prompt.clone_from(&said);
                }
                reading.pending.push(json!({
                    "role": "user",
                    "content": [{"type": "text", "text": said}],
                }));
            }
            Writer::Model => {
                begun = true;
                if said.is_empty() {
                    continue;
                }
                let span = json!({
                    "id": reading.next(),
                    "parent_id": "root",
                    "type": "llm_call",
                    "name": "turn",
                    "status": "ok",
                    "model": {"id": model},
                    "input": {"messages": std::mem::take(&mut reading.pending)},
                    "output": {"messages": [{
                        "role": "assistant",
                        "content": [{"type": "text", "text": said}],
                    }]},
                });
                reading.add_timed(span);
            }
            Writer::Aider => {
                reading.flush();
                for line in &block.lines {
                    if let Some(found) = version_in(line) {
                        found.clone_into(&mut version);
                    }
                    if let Some(found) = model_in(line) {
                        found.clone_into(&mut model);
                    }
                }
                let notices = block.lines.join("\n");
                let span = if begun {
                    // What Aider reported doing. It runs its own tools
                    // (applying edits, committing, linting) and this is all
                    // it says of them.
                    json!({
                        "id": reading.next(),
                        "parent_id": "root",
                        "type": "tool_call",
                        "name": "aider",
                        "input": {"name": "aider"},
                        "output": {"result": notices},
                    })
                } else {
                    // What Aider says before anything is typed is the setup
                    // of the run: its version, the model, the files in the
                    // chat.
                    json!({
                        "id": reading.next(),
                        "parent_id": "root",
                        "type": "custom",
                        "name": "aider startup",
                        "input": {"messages": [{
                            "role": "system",
                            "content": [{"type": "text", "text": notices}],
                        }]},
                    })
                };
                reading.spans.push(span);
            }
        }
    }
    reading.flush();

    let mut name = prompt.split('\n').next().unwrap_or_default().to_owned();
    if name.chars().count() > 80 {
        name = name.chars().take(80).chain(['…']).collect();
    }
    if name.is_empty() {
        name = format!("aider {stamp}");
    }
    let mut spans = reading.spans;
    spans[0]["name"] = json!(name);

    let mut metadata = Map::new();
    metadata.insert("session_id".into(), json!(run_id(stamp)));
    let cwd = text(&input.session, "cwd");
    if !cwd.is_empty() {
        metadata.insert("cwd".into(), json!(cwd));
    }
    if !version.is_empty() {
        metadata.insert("harness_version".into(), json!(version));
    }
    Ok(json!({
        "schema": "session/v0",
        "name": name,
        "created_at": iso(started),
        "source": {"kind": "import", "harness": "aider", "label": LABEL, "fidelity": "reconstructed"},
        "metadata": metadata,
        "spans": spans,
    }))
}

/* -------------------------------------------------- where its sessions are */

/// One run of a chat history.
struct Run {
    /// The time the run started, as Aider wrote it.
    id: String,
    started: Instant,
    /// From the run's opening line to the next run's.
    lines: Vec<String>,
}

/// Splits a chat history into its runs, oldest first. Lines ahead of the
/// first opening line are the end of a run whose start is gone, and are left
/// out.
fn runs(file: &Path, zone: Zone) -> io::Result<Vec<Run>> {
    let raw = fs::read(file)?;
    let mut runs: Vec<Run> = Vec::new();
    for line in decoded(&raw).split('\n') {
        if let Some(stamp) = opening(line)
            && let Some(started) = clock_time(stamp, zone)
        {
            runs.push(Run {
                id: run_id(stamp),
                started,
                lines: Vec::new(),
            });
        }
        if let Some(run) = runs.last_mut() {
            run.lines.push(line.to_owned());
        }
    }
    Ok(runs)
}

/// The first thing typed in a run that was not a command to Aider itself.
fn first_prompt(run: &Run) -> String {
    blocks(&run.lines[1..])
        .iter()
        .filter(|block| block.writer == Writer::Person)
        .map(|block| block_text(&block.lines))
        .find(|said| !said.is_empty() && !said.starts_with('/'))
        .unwrap_or_default()
}

/// Reads the input history Aider keeps beside a chat history: each entry
/// typed at its prompt, under a line with the time.
fn typed_entries(chat: &Path) -> Vec<Value> {
    let name = chat
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let name = format!(
        "{}input.history",
        name.strip_suffix("chat.history.md").unwrap_or(&name)
    );
    let Ok(raw) = fs::read(chat.with_file_name(name)) else {
        return Vec::new();
    };
    let history = decoded(&raw);
    let mut entries: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in history.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(at) = line.strip_prefix("# ") {
            entries.push((at.trim(), Vec::new()));
        } else if let Some(typed) = line.strip_prefix('+')
            && let Some((_, lines)) = entries.last_mut()
        {
            lines.push(typed);
        }
    }
    entries
        .into_iter()
        .map(|(at, lines)| json!({"at": at, "text": lines.join("\n")}))
        .collect()
}

/// A path made whole and plain by its text alone, as Go's `filepath.Abs`
/// makes it: under the working directory when it is relative, with `.` left
/// out and `..` taking the name before it.
fn absolute(file: &Path) -> Option<PathBuf> {
    let whole = if file.is_absolute() {
        file.to_owned()
    } else {
        env::current_dir().ok()?.join(file)
    };
    let mut plain = PathBuf::new();
    for part in whole.components() {
        match part {
            Component::CurDir => {}
            // Above the root there is only the root.
            Component::ParentDir => {
                plain.pop();
            }
            part => plain.push(part),
        }
    }
    Some(plain)
}

/// The importer's input for one run of a chat history: the one that started
/// at the time `id` names, or the last. It carries the input history's
/// entries from that run, and the project directory when the file is where
/// Aider itself keeps it.
fn read_run(file: &Path, id: &str, zone: Zone) -> Result<Input, String> {
    let mut runs = runs(file, zone).map_err(|error| error.to_string())?;
    let pick = if id.is_empty() {
        runs.len().checked_sub(1)
    } else {
        runs.iter().rposition(|run| run.id == id)
    };
    let Some(pick) = pick else {
        return Err(if id.is_empty() {
            format!("{} holds no aider run", file.display())
        } else {
            format!("no aider run started at {id} in {}", file.display())
        });
    };
    let started = runs[pick].started;
    let next = runs.get(pick + 1).map(|run| run.started);
    let inputs: Vec<Value> = typed_entries(file)
        .into_iter()
        .filter(|entry| {
            clock_time(text(entry, "at"), zone)
                .is_some_and(|at| at >= started && next.is_none_or(|next| at < next))
        })
        .collect();
    let run = runs.swap_remove(pick);
    let mut session = Map::new();
    session.insert("id".into(), json!(run.id));
    session.insert("inputs".into(), Value::Array(inputs));
    if let Some(whole) = absolute(file)
        && whole.file_name().is_some_and(|name| name == CHAT_NAME)
        && let Some(project) = whole.parent()
    {
        session.insert("cwd".into(), json!(project.to_string_lossy()));
    }
    Ok(Input {
        lines: run.lines,
        fallback: run.id,
        session: Value::Object(session),
        ..Input::default()
    })
}

pub(crate) fn latest(cwd: &Path) -> Option<Found> {
    let file = cwd.join(CHAT_NAME);
    let recency = modified(&file)?;
    if runs(&file, zone()).unwrap_or_default().is_empty() {
        return None;
    }
    Some(Found {
        harness: "aider",
        recency,
        loader: Loader::AiderRun {
            file,
            id: String::new(),
        },
    })
}

/// The runs in the chat history of each of `dirs`; one named by `id` only,
/// when an id is given. Aider has no store of its own: a project's history
/// is a file in the project.
pub(crate) fn candidates(dirs: &[PathBuf], id: &str) -> Result<Vec<Candidate>, String> {
    let mut found = Vec::new();
    for dir in dirs {
        let file = dir.join(CHAT_NAME);
        let Some(written) = modified(&file).filter(|_| file.is_file()) else {
            continue;
        };
        let runs = runs(&file, zone()).unwrap_or_default();
        for (at, run) in runs.iter().enumerate() {
            if !id.is_empty() && run.id != id {
                continue;
            }
            // A run was last active when the file was last written, or
            // before the run after it began.
            let active = runs
                .get(at + 1)
                .map_or(written, |next| unix_nanos(next.started));
            found.push(Candidate {
                found: Found {
                    harness: "aider",
                    recency: active,
                    loader: Loader::AiderRun {
                        file: file.clone(),
                        id: run.id.clone(),
                    },
                },
                id: run.id.clone(),
                title: String::new(),
                dir: dir.clone(),
                file: Some(file.clone()),
                related: Vec::new(),
                name: String::new(),
                prompt: first_prompt(run),
                started: Some((
                    run.started.timestamp(),
                    run.started.timestamp_subsec_nanos(),
                )),
            });
        }
    }
    Ok(found)
}

/// Fills in a candidate's title: Aider gives a run none, so it is the run's
/// first prompt.
pub(crate) fn describe(candidate: &mut Candidate) {
    candidate.title.clone_from(&candidate.prompt);
}

/// Whether a file is an Aider chat history: it is named like one, or its
/// first line opens a run.
pub(crate) fn is_chat(file: &Path) -> bool {
    let named = file
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("chat.history.md"));
    if named {
        return true;
    }
    let mut head = Vec::new();
    if let Ok(opened) = fs::File::open(file) {
        // What was read before a failure is still looked at.
        let _ = opened.take(4096).read_to_end(&mut head);
    }
    decoded(&head)
        .split('\n')
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| opening(line).is_some())
}

/// One run of a chat history as an importer's input: the one that started at
/// the time `id` names, or the last when `id` is empty.
pub(crate) fn load_run(file: &Path, id: &str) -> Result<Input, String> {
    read_run(file, id, zone())
}

/// The run that started at the time `id` names, in the chat history of `cwd`.
pub(crate) fn run(cwd: &Path, id: &str) -> Option<Found> {
    let file = cwd.join(CHAT_NAME);
    let run = runs(&file, zone())
        .unwrap_or_default()
        .into_iter()
        .find(|run| run.id == id)?;
    Some(Found {
        harness: "aider",
        recency: unix_nanos(run.started),
        loader: Loader::AiderRun {
            file,
            id: id.to_owned(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(written: &str) -> Option<String> {
        clock_time(written, Zone::Utc).map(iso)
    }

    #[test]
    fn marks_say_who_a_line_is_from() {
        let lines = [
            "> Aider v0.86.2  ",
            ">  ",
            "> Repo-map: disabled  ",
            "",
            "#### first line  ",
            // An empty line inside one typed entry.
            "####   ",
            "#### third line  ",
            "",
            "#### a second entry  ",
            "",
            "A reply.\r",
            "",
            "    indented code",
            "",
            "",
            "> Tokens: 12 sent, 3 received.  ",
        ]
        .map(str::to_owned);
        let got: Vec<(Writer, String)> = blocks(&lines)
            .iter()
            .map(|block| (block.writer, block.lines.join("|")))
            .collect();
        let want = [
            (Writer::Aider, "Aider v0.86.2||Repo-map: disabled"),
            (Writer::Person, "first line||third line"),
            (Writer::Person, "a second entry"),
            (Writer::Model, "A reply.||    indented code||"),
            (Writer::Aider, "Tokens: 12 sent, 3 received."),
        ]
        .map(|(writer, lines)| (writer, lines.to_owned()));
        assert_eq!(got, want);
    }

    #[test]
    fn a_byte_that_is_not_text_is_one_replacement_character() {
        assert_eq!(
            decoded("caf\u{e9} \u{1f600}".as_bytes()),
            "caf\u{e9} \u{1f600}"
        );
        assert_eq!(decoded(b"a\xffb"), "a\u{fffd}b");
        // A character cut short is as many as the bytes left of it, where
        // Rust's own lossy reading gives one.
        assert_eq!(decoded(b"a\xe2\x82b"), "a\u{fffd}\u{fffd}b");
        assert_eq!(decoded(b"\xf0\x9f\x98"), "\u{fffd}\u{fffd}\u{fffd}");
        assert_eq!(decoded(b"\xc0\x80\xed\xa0\x80"), "\u{fffd}".repeat(5));
    }

    #[test]
    fn a_line_opens_a_run_only_in_aiders_own_words() {
        let stamp = Some("2026-10-06 09:00:00");
        assert_eq!(
            opening("# aider chat started at 2026-10-06 09:00:00"),
            stamp
        );
        assert_eq!(
            opening("# aider chat started at 2026-10-06 09:00:00 \t\r"),
            stamp
        );
        for line in [
            "# aider chat started at 2026-10-06 09:00",
            "# aider chat started at 2026-10-06T09:00:00",
            "# aider chat started at 2026-10-06 09:00:00.",
            "# aider chat started at 2026-10-06 09:00:00\u{a0}",
            "# aider chat started at 2026-10-06 09:00:0\u{665}",
            " # aider chat started at 2026-10-06 09:00:00",
            "# Aider chat started at 2026-10-06 09:00:00",
        ] {
            assert_eq!(opening(line), None, "{line}");
        }
    }

    #[test]
    fn a_clock_time_is_read_as_go_reads_it() {
        let read = |written: &str| utc(written);
        assert_eq!(
            read("2026-10-06 09:00:07.250000").as_deref(),
            Some("2026-10-06T09:00:07.250Z")
        );
        // What Go's layout lets through beyond what Aider writes.
        for written in [
            "2026-10-06 09:00:07,25",
            "2026-10-06    9:00:07.2500000000009",
        ] {
            assert_eq!(
                read(written).as_deref(),
                Some("2026-10-06T09:00:07.250Z"),
                "{written}"
            );
        }
        assert_eq!(
            read("2026-10-06 09:00:07.").as_deref(),
            None,
            "a point with no digit after it is text left over"
        );
        assert_eq!(
            read("2024-02-29 23:59:59").as_deref(),
            Some("2024-02-29T23:59:59.000Z")
        );
        // Digits past the third are cut, not rounded.
        assert_eq!(
            read("2026-10-06 09:00:07.999999").as_deref(),
            Some("2026-10-06T09:00:07.999Z")
        );
        for written in [
            "",
            "2026-10-06",
            "2026-10-06T09:00:07",
            "2026-10-0609:00:07",
            "2026-10-06\t09:00:07",
            "2026-1-06 09:00:07",
            "2026-10-6 09:00:07",
            "2026-10-06 09:0:07",
            "2026-10-06 09:00:7",
            "2026-13-06 09:00:07",
            "2026-00-06 09:00:07",
            "2026-02-29 09:00:07",
            "2026-10-00 09:00:07",
            "2026-10-06 24:00:07",
            "2026-10-06 09:60:07",
            "2026-10-06 09:00:60",
            "2026-10-06 09:00:07 UTC",
            "2026-10-06 09:00:07.25 ",
            "+026-10-06 09:00:07",
            "26-10-06 09:00:07",
        ] {
            assert_eq!(read(written), None, "{written:?}");
        }
    }

    #[test]
    fn a_year_is_written_as_go_writes_it() {
        assert_eq!(
            utc("0000-01-01 00:00:00").as_deref(),
            Some("0000-01-01T00:00:00.000Z")
        );
        assert_eq!(
            utc("9999-12-31 23:59:59").as_deref(),
            Some("9999-12-31T23:59:59.000Z")
        );
        let before = DateTime::from_timestamp(-62_167_219_200 - 1, 0).unwrap();
        assert_eq!(iso(before), "-0001-12-31T23:59:59.000Z");
        // The first instant of the year 1 is Go's zero time.
        assert!(unset(clock_time("0001-01-01 00:00:00", Zone::Utc).unwrap()));
        assert!(!unset(
            clock_time("0001-01-01 00:00:00.000001", Zone::Utc).unwrap()
        ));
    }

    /// A zone with one clock change, at the instant `change`.
    fn changing(change: i64, before: i64, after: i64) -> impl Fn(i64) -> i64 {
        move |at| if at < change { before } else { after }
    }

    #[test]
    fn a_time_at_a_clock_change_is_the_instant_go_makes_of_it() {
        const HOUR: i64 = 3600;
        // A reading on the day of the change, in seconds.
        let reading = |hours: i64, minutes: i64| hours * HOUR + minutes * 60;

        // Behind UTC, as New York is: five hours, four in summer. Its clocks
        // go from 02:00 to 03:00 at 07:00 UTC and from 02:00 back to 01:00
        // at 06:00 UTC.
        let spring = changing(7 * HOUR, -5 * HOUR, -4 * HOUR);
        assert_eq!(instant_of(reading(1, 30), &spring), reading(6, 30));
        assert_eq!(instant_of(reading(3, 30), &spring), reading(7, 30));
        // 02:30 was skipped: it is read with the offset after the change.
        assert_eq!(instant_of(reading(2, 30), &spring), reading(6, 30));
        let autumn = changing(6 * HOUR, -4 * HOUR, -5 * HOUR);
        assert_eq!(instant_of(reading(0, 30), &autumn), reading(4, 30));
        assert_eq!(instant_of(reading(2, 30), &autumn), reading(7, 30));
        // 01:30 came twice: it is the first of the two.
        assert_eq!(instant_of(reading(1, 30), &autumn), reading(5, 30));

        // Ahead of UTC, as Athens is: two hours, three in summer. Its clocks
        // go from 03:00 to 04:00, and from 04:00 back to 03:00, at 01:00 UTC.
        let spring = changing(HOUR, 2 * HOUR, 3 * HOUR);
        assert_eq!(instant_of(reading(2, 30), &spring), reading(0, 30));
        assert_eq!(instant_of(reading(4, 30), &spring), reading(1, 30));
        // 03:30 was skipped: it is read with the offset before the change.
        assert_eq!(instant_of(reading(3, 30), &spring), reading(1, 30));
        let autumn = changing(HOUR, 3 * HOUR, 2 * HOUR);
        assert_eq!(instant_of(reading(2, 30), &autumn), reading(0, 30) - HOUR);
        assert_eq!(instant_of(reading(4, 30), &autumn), reading(2, 30));
        // 03:30 came twice: it is the second of the two.
        assert_eq!(instant_of(reading(3, 30), &autumn), reading(1, 30));
    }

    #[test]
    fn aider_names_its_version_and_its_model() {
        assert_eq!(version_in("Aider v0.86.2"), Some("0.86.2"));
        assert_eq!(
            version_in("Aider v0.86.2.dev+local  x"),
            Some("0.86.2.dev+local")
        );
        assert_eq!(version_in("Aider v"), None);
        assert_eq!(version_in("Aider v 1"), None);
        assert_eq!(version_in(" Aider v1"), None);

        for (notice, model) in [
            (
                "Model: openai/gpt-small with whole edit format",
                "openai/gpt-small",
            ),
            (
                "Main model: anthropic/claude-sonnet with diff edit format",
                "anthropic/claude-sonnet",
            ),
            ("Model: gpt-small", "gpt-small"),
            ("Model: gpt-small, infinite output", "gpt-small"),
            ("Model: gpt-small,", "gpt-small"),
            ("Model: a,b with", "a,b"),
            ("Model: a,, with", "a,"),
            ("Model: , with", ","),
        ] {
            assert_eq!(model_in(notice), Some(model), "{notice}");
        }
        for notice in [
            "Model:",
            "Model:  gpt-small",
            "Model: gpt-small\twith",
            "Weak model: gpt-small",
            "main model: gpt-small",
        ] {
            assert_eq!(model_in(notice), None, "{notice}");
        }
    }

    #[test]
    fn a_path_is_made_whole_by_its_text() {
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        let at = |rest: &str| Path::new(root).join(rest);
        assert_eq!(absolute(&at("a/./b/../c")), Some(at("a/c")));
        assert_eq!(absolute(&at("../a")), Some(at("a")));
        let here = env::current_dir().unwrap();
        assert_eq!(absolute(Path::new("a/../b")), Some(here.join("b")));
    }
}
