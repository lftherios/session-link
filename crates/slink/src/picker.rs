//! The sessions page: what `slink view` shows when it was not told which
//! session, or when a reader goes back from one. Each row saves a preview
//! of its session when it is opened, and not before.

use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use regex::Regex;
use slink_local::{Drafts, display_path, session_title_key};

/// The page's own styles and scripts, as the Go CLI serves them.
const LIST_CSS: &str = include_str!("assets/list.css.html");
const LIST_SCRIPT: &str = include_str!("assets/list.js.html");
const PICKER_SCRIPT: &str = include_str!("assets/picker.js.html");

const DOT: &str = "<span aria-hidden=\"true\">·</span>";
const ICON_SEARCH: &str = "<svg aria-hidden=\"true\" width=\"18\" height=\"18\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><circle cx=\"11\" cy=\"11\" r=\"8\"/><path d=\"m21 21-4.3-4.3\"/></svg>";
const ICON_CHEVRON: &str = "<svg aria-hidden=\"true\" width=\"16\" height=\"16\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"m9 18 6-6-6-6\"/></svg>";
const ICON_POWER: &str = "<svg aria-hidden=\"true\" width=\"14\" height=\"14\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M12 2v10\"/><path d=\"M18.4 6.6a9 9 0 1 1-12.77.04\"/></svg>";

/// What the page shows of one session.
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub id: String,
    pub harness: String,
    /// The project directory it was recorded in; empty when unknown.
    pub dir: String,
    /// A title the harness recorded.
    pub name: String,
    /// The first thing the person asked.
    pub prompt: String,
    /// When it was last written and when it started, in nanoseconds since
    /// 1970; zero when unknown.
    pub updated: i64,
    pub started: i64,
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}

fn local(nanos: i64) -> DateTime<Local> {
    Local.timestamp_nanos(nanos)
}

/// How long ago a time was: "just now" to "3w ago", then the plain date once
/// a relative one stops meaning anything.
fn age(then: i64, now: i64) -> String {
    let seconds = (now - then) / 1_000_000_000;
    match seconds {
        // A stamp from the future: a relative age would lie.
        i64::MIN..-60 => local(then).format("%Y-%m-%d").to_string(),
        -60..60 => "just now".into(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        86_400..604_800 => format!("{}d ago", seconds / 86_400),
        604_800..2_592_000 => format!("{}w ago", seconds / 604_800),
        _ => local(then).format("%-d %b %Y").to_string(),
    }
}

/// The name of a time's local day, for a list's headings.
pub(crate) fn day_label(then: i64, now: i64) -> String {
    if then == 0 {
        return "Undated".into();
    }
    let (then, now) = (local(then), local(now));
    let days = now
        .date_naive()
        .signed_duration_since(then.date_naive())
        .num_days();
    match days {
        i64::MIN..=0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => then.format("%A").to_string(),
        _ if then.year() == now.year() => then.format("%B %-d").to_string(),
        _ => then.format("%B %-d, %Y").to_string(),
    }
}

/// How long ago a session from today was active, and the time of day for an
/// older one; the title holds the whole date.
fn row_stamp(then: i64, now: i64) -> String {
    if then == 0 {
        return String::new();
    }
    let at = local(then);
    let label = if day_label(then, now) == "Today" {
        age(then, now)
    } else {
        at.format("%-I:%M %p").to_string()
    };
    format!(
        "<time datetime=\"{}\" title=\"{}\">{}</time>",
        at.with_timezone(&Utc).format("%Y-%m-%dT%H:%M:%SZ"),
        escape(&at.format("%A, %B %-d, %Y at %-I:%M %p").to_string()),
        escape(&label)
    )
}

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:untitled(?: session)?|session|[a-f\d-]{24,}|rollout-.*|.*\.(?:jsonl?|spool))$",
    )
    .expect("the pattern compiles")
});

/// Whether a recorded name is a title. Placeholders, file names, paths and
/// injected context are not, as in the viewer.
fn meaningful(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && !PLACEHOLDER.is_match(name)
        && !name.starts_with('/')
        && !name.starts_with('<')
}

/// How people name each agent.
pub(crate) fn harness_label(harness: &str) -> &str {
    match harness {
        "claude-code" => "Claude Code",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        "hermes" => "Hermes",
        "dsh" => "DeepSeek Harness",
        "aider" => "Aider",
        "" => "Local capture",
        other => other,
    }
}

/// The rows under day headings, in the order days first appear.
fn group(rows: &[(String, String)]) -> String {
    let mut days: Vec<(&str, String)> = Vec::new();
    for (day, html) in rows {
        match days.iter_mut().find(|(known, _)| known == day) {
            Some((_, list)) => list.push_str(html),
            None => days.push((day, html.clone())),
        }
    }
    days.iter()
        .map(|(day, list)| {
            let day = escape(day);
            format!("<section class=\"group\" aria-label=\"{day}\"><h2>{day}</h2><div class=\"rows\">{list}</div></section>")
        })
        .collect()
}

/// The page's body: the project's name, a way to stop the viewer, a search
/// field and the sessions by day. `now` is in nanoseconds since 1970.
pub fn body(project: &Path, rows: &[Row], warning: &str, drafts: &Drafts, now: i64) -> String {
    let rendered: Vec<(String, String)> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let harness = harness_label(&row.harness);
            // The title the session's page shows: one typed for the session,
            // a title the harness recorded, or the untitled label.
            let mut title = drafts.title_for("", &session_title_key(&row.harness, &row.id));
            if title.is_empty() && meaningful(&row.name) {
                title = row.name.trim().to_owned();
            }
            let untitled = title.is_empty();
            if untitled {
                title = format!("Untitled · {harness}");
                let started = if row.started == 0 { row.updated } else { row.started };
                if started != 0 {
                    title.push_str(&local(started).format(" · %b %-d, %Y").to_string());
                }
            }
            let mut meta: Vec<String> = Vec::new();
            if !untitled {
                meta.push(escape(harness));
            }
            if row.harness == "hermes" {
                meta.push("experimental".into());
            }
            if !row.dir.is_empty() && Path::new(&row.dir) != project {
                let base = Path::new(&row.dir).file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
                meta.push(format!(
                    "<span title=\"{}\">{}</span>",
                    escape(&display_path(Path::new(&row.dir))),
                    escape(&base)
                ));
            }
            if !row.id.is_empty() {
                let short = if row.id.len() > 12 { row.id.get(..8).unwrap_or(&row.id) } else { &row.id };
                meta.push(format!("<code title=\"{}\">{}</code>", escape(&row.id), escape(short)));
            }
            let prompt = row.prompt.trim();
            if !prompt.is_empty() && prompt != title {
                meta.push(format!("<span class=\"row-preview\">{}</span>", escape(prompt)));
            }
            let class = if untitled { "row-title untitled" } else { "row-title" };
            let search = [title.as_str(), &row.prompt, harness, &row.harness, &row.id, &row.dir].join(" ").to_lowercase();
            let html = format!(
                "<button class=\"row\" data-source=\"{index}\" data-search=\"{}\"><span class=\"{class}\">{}</span><span class=\"row-meta\">{}</span><span class=\"row-side\">{}<span class=\"row-open\">{ICON_CHEVRON}</span></span></button>",
                escape(&search),
                escape(&title),
                meta.join(DOT),
                row_stamp(row.updated, now),
            );
            (day_label(row.updated, now), html)
        })
        .collect();
    let name = project
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let warning = if warning.is_empty() {
        String::new()
    } else {
        format!(
            "<p class=\"list-status err\">Some sessions could not be listed: {}</p>",
            escape(warning)
        )
    };
    let status = "<p id=\"result\" class=\"list-status\" role=\"status\"></p>";
    let list = if rendered.is_empty() {
        format!(
            "{status}{warning}<div class=\"empty-state\"><strong>No sessions for this project yet</strong>Open a transcript with <code>slink view --session &lt;path&gt;</code>, or record new work with <code>slink record -- &lt;command&gt;</code>.</div>"
        )
    } else {
        let count = if rendered.len() == 1 {
            "1 session".to_owned()
        } else {
            format!("{} sessions", rendered.len())
        };
        format!(
            "<label class=\"find\">{ICON_SEARCH}<input type=\"search\" id=\"search\" aria-label=\"Find a session\" placeholder=\"Search {count}…\" autocomplete=\"off\"></label>\n    {status}{warning}<div class=\"groups\">{}</div>\n    <p id=\"no-match\" class=\"list-status empty-match\" hidden>No sessions match this search.</p>",
            group(&rendered)
        )
    };
    format!(
        "{LIST_CSS}\n    <header class=\"list-head\"><h1 title=\"{}\">{}</h1><button class=\"quiet\" id=\"stop\">{ICON_POWER}Stop viewer</button></header>\n    {list}{LIST_SCRIPT}\n{PICKER_SCRIPT}",
        escape(&display_path(project)),
        escape(&name),
    )
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, NaiveDate};

    use super::*;

    fn at(year: i32, month: u32, day: u32, hour: u32) -> i64 {
        let naive = NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, 0, 0)
            .unwrap();
        Local
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap()
    }

    #[test]
    fn a_day_is_named_by_how_far_back_it_is() {
        let now = at(2026, 9, 11, 15);
        let back = |days: i64| {
            (local(now) - Duration::days(days))
                .timestamp_nanos_opt()
                .unwrap()
        };
        assert_eq!(day_label(now - 2 * 3600 * 1_000_000_000, now), "Today");
        assert_eq!(day_label(back(1), now), "Yesterday");
        assert_eq!(
            day_label(back(3), now),
            local(back(3)).format("%A").to_string()
        );
        assert_eq!(day_label(at(2026, 8, 1, 9), now), "August 1");
        assert_eq!(day_label(at(2025, 12, 24, 9), now), "December 24, 2025");
        assert_eq!(day_label(0, now), "Undated");
    }

    #[test]
    fn the_page_has_a_quiet_header_and_groups_by_day() {
        let previews = std::env::temp_dir()
            .join(format!("slink-picker-{}", std::process::id()))
            .join("previews");
        let drafts = Drafts::beside(&previews);
        drafts
            .set_title(
                "unused",
                &session_title_key("pi", "renamed"),
                "Competitor comparison",
            )
            .unwrap();
        let now = Local::now().timestamp_nanos_opt().unwrap();
        let minutes = |count: i64| now - count * 60 * 1_000_000_000;
        let rows = [
            Row {
                id: "abcdef12-3456-7890".into(),
                harness: "claude-code".into(),
                name: "Fix <b>header</b>".into(),
                prompt: "Please fix the header".into(),
                updated: minutes(5),
                ..Row::default()
            },
            Row {
                id: "older".into(),
                harness: "codex".into(),
                prompt: "Review the project status".into(),
                updated: (local(now) - Duration::days(1))
                    .timestamp_nanos_opt()
                    .unwrap(),
                started: at(2026, 9, 10, 9),
                ..Row::default()
            },
            Row {
                id: "renamed".into(),
                harness: "pi".into(),
                name: "rollout-2026.jsonl".into(),
                prompt: "Compare competitors".into(),
                updated: minutes(60),
                ..Row::default()
            },
        ];
        let page = body(Path::new("/work/project"), &rows, "", &drafts, now);
        for wanted in [
            "<h1 title=\"/work/project\">project</h1>",
            "aria-label=\"Yesterday\"",
            "<span class=\"row-title\">Fix &lt;b&gt;header&lt;/b&gt;</span>",
            "<span class=\"row-title untitled\">Untitled · Codex · Sep 10, 2026</span>",
            "<span class=\"row-title\">Competitor comparison</span>",
            "<span class=\"row-preview\">Review the project status</span>",
            "<code title=\"abcdef12-3456-7890\">abcdef12</code>",
            "placeholder=\"Search 3 sessions…\"",
            "id=\"stop\"",
            "data-source=\"2\"",
        ] {
            assert!(page.contains(wanted), "the page is missing {wanted}");
        }
        for gone in ["Open preview", "class=\"row-title\">rollout-2026.jsonl"] {
            assert!(!page.contains(gone), "the page still shows {gone}");
        }
        let empty = body(
            Path::new("/work/project"),
            &[],
            "codex: unreadable",
            &drafts,
            now,
        );
        assert!(
            empty.contains("No sessions for this project yet")
                && empty.contains("Some sessions could not be listed: codex: unreadable")
        );
        std::fs::remove_dir_all(previews.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_placeholder_is_not_a_title() {
        for name in [
            "Untitled",
            "untitled session",
            "session",
            "0c3b8f0e-5d0a-4b7e-9a51-3f6f2a1d7c44",
            "rollout-2026-09.jsonl",
            "notes.json",
            "/tmp/x",
            "<system>",
            "  ",
        ] {
            assert!(!meaningful(name), "{name}");
        }
        for name in ["Fix the header", "Session notes for Friday", "abc"] {
            assert!(meaningful(name), "{name}");
        }
    }
}
