//! What a person makes of a saved preview before sharing it, kept beside
//! the previews and never inside a document: a title they typed, the draft
//! of an excerpt, and which excerpt was made from which session.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::hex;
use crate::home::{create_private_dir, write_private};
use crate::share::Draft;

/// Why a draft was not saved.
#[derive(Debug)]
pub enum SaveError {
    /// Another viewer saved this revision first.
    Taken,
    Failed(io::Error),
}

/// An excerpt made from a session, as the compose page lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedView {
    pub id: String,
    pub title: String,
    /// When it was made, in nanoseconds since 1970.
    pub saved_at: i64,
}

/// Which session an excerpt was made from, and the draft it was made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewRecord {
    pub source: String,
    pub draft: Option<Draft>,
    pub saved_at: i64,
}

/// The directory of drafts that goes with a directory of previews.
#[derive(Debug, Clone)]
pub struct Drafts {
    previews: PathBuf,
    dir: PathBuf,
}

/// The file a title typed for a session is kept under, whichever snapshot
/// of the session it was typed on. Empty when the session cannot be named.
pub fn session_title_key(harness: &str, id: &str) -> String {
    if harness.is_empty() || id.is_empty() {
        return String::new();
    }
    let sum = Sha256::digest(format!("{harness}\n{id}").as_bytes());
    format!("session-{}", hex(&sum[..16]))
}

/// A name that is one path component and nothing else.
fn file_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

impl Drafts {
    pub fn beside(previews: &Path) -> Drafts {
        Drafts {
            previews: previews.to_owned(),
            dir: previews.parent().unwrap_or(Path::new("")).join("drafts"),
        }
    }

    fn read_json(&self, parts: &[&str]) -> Option<Value> {
        let file = parts
            .iter()
            .fold(self.dir.clone(), |path, part| path.join(part));
        slink_format::parse(&fs::read_to_string(file).ok()?).ok()
    }

    /// The title typed under one name, or `""`.
    fn local_title(&self, id: &str) -> String {
        self.read_json(&["titles", &format!("{id}.json")])
            .and_then(|title| title["title"].as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    /// The title typed for a session, else the one typed for this snapshot:
    /// a later snapshot of the same session keeps its name.
    pub fn title_for(&self, id: &str, key: &str) -> String {
        let typed = if key.is_empty() {
            String::new()
        } else {
            self.local_title(key)
        };
        if typed.is_empty() {
            self.local_title(id)
        } else {
            typed
        }
    }

    /// Saves a typed title under the snapshot and, when the session can be
    /// named, under the session.
    pub fn set_title(&self, id: &str, key: &str, title: &str) -> io::Result<()> {
        let text = slink_format::encode(&json!({"title": title}));
        write_private(
            &self.dir.join("titles").join(format!("{id}.json")),
            text.as_bytes(),
        )?;
        if !key.is_empty() {
            write_private(
                &self.dir.join("titles").join(format!("{key}.json")),
                text.as_bytes(),
            )?;
        }
        Ok(())
    }

    /// The newest revision of a preview's draft, and its number. A preview
    /// with no draft yet has the empty one, at revision zero.
    pub fn read_draft(&self, id: &str) -> Result<(i64, Draft), String> {
        let empty = Draft {
            title: "Shared session excerpt".into(),
            ..Draft::default()
        };
        let dir = self.dir.join(id);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((0, empty)),
            Err(error) => return Err(error.to_string()),
        };
        let newest = entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let revision: i64 = path
                    .file_name()?
                    .to_str()?
                    .strip_suffix(".json")?
                    .parse()
                    .ok()?;
                (path.is_file() && revision > 0).then_some((revision, path))
            })
            .max_by_key(|(revision, _)| *revision);
        let Some((_, file)) = newest else {
            return Ok((0, empty));
        };
        let saved =
            slink_format::parse(&fs::read_to_string(file).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        Ok((
            saved["revision"].as_i64().unwrap_or(0),
            Draft::from_json(&saved["draft"])?,
        ))
    }

    /// Installs a whole revision, only if its number is unused. Making a
    /// hard link either happens or does not across every viewer running,
    /// which a lock inside one process or a rename over a shared file would
    /// not give; and a reader never sees part of a draft.
    pub fn write_draft(&self, id: &str, revision: i64, draft: &Draft) -> Result<(), SaveError> {
        let dir = self.dir.join(id);
        create_private_dir(&dir).map_err(SaveError::Failed)?;
        let text = slink_format::encode(&json!({"revision": revision, "draft": draft.to_json()}));
        let temporary = dir.join(format!(".draft-{}-{revision}", std::process::id()));
        write_private(&temporary, text.as_bytes()).map_err(SaveError::Failed)?;
        let linked = fs::hard_link(&temporary, dir.join(format!("{revision:020}.json")));
        let _ = fs::remove_file(&temporary);
        match linked {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(SaveError::Taken),
            Err(error) => Err(SaveError::Failed(error)),
        }
    }

    /// Notes which session an excerpt was made from. This stays here,
    /// outside the excerpt: how it was edited is not part of what is shared.
    pub fn record_view(
        &self,
        exported: &str,
        source: &str,
        draft: &Draft,
        saved_at: i64,
    ) -> io::Result<()> {
        let text = slink_format::encode(
            &json!({"source": source, "draft": draft.to_json(), "saved_at": saved_at}),
        );
        write_private(
            &self.dir.join("exports").join(format!("{exported}.json")),
            text.as_bytes(),
        )
    }

    pub fn view_record(&self, id: &str) -> Option<ViewRecord> {
        if !file_id(id) {
            return None;
        }
        let record = self.read_json(&["exports", &format!("{id}.json")])?;
        Some(ViewRecord {
            source: record["source"].as_str().unwrap_or_default().to_owned(),
            draft: record
                .get("draft")
                .filter(|draft| !draft.is_null())
                .and_then(|draft| Draft::from_json(draft).ok()),
            saved_at: record["saved_at"].as_i64().unwrap_or(0),
        })
    }

    /// The session an excerpt can be edited from, when that is known.
    pub fn edit_source(&self, id: &str) -> Option<String> {
        self.view_record(id)
            .map(|record| record.source)
            .filter(|source| file_id(source))
    }

    /// Where this viewer last published a saved preview, so the share panel
    /// can offer the link again: its `url`, `published_at` in milliseconds,
    /// and how many people it was shared with, when it was.
    pub fn published(&self, id: &str) -> Option<Value> {
        if !file_id(id) {
            return None;
        }
        let record = self.read_json(&["published", &format!("{id}.json")])?;
        let url = record["url"].as_str().filter(|url| !url.is_empty())?;
        let mut published =
            json!({"url": url, "published_at": record["published_at"].as_i64().unwrap_or(0)});
        if let Some(recipients) = record["recipients"].as_u64().filter(|count| *count > 0) {
            published["recipients"] = json!(recipients);
        }
        Some(published)
    }

    /// Keeps the newest link a saved preview was published as. Losing it
    /// loses only the reminder: the publish happened and was reported.
    pub fn record_published(
        &self,
        id: &str,
        url: &str,
        recipients: usize,
        at: i64,
    ) -> io::Result<()> {
        if url.is_empty() || !file_id(id) {
            return Ok(());
        }
        let text = slink_format::encode(&Value::String(url.to_owned()));
        let count = if recipients > 0 {
            format!(",\"recipients\":{recipients}")
        } else {
            String::new()
        };
        write_private(
            &self.dir.join("published").join(format!("{id}.json")),
            format!("{{\"url\":{text}{count},\"published_at\":{at}}}").as_bytes(),
        )
    }

    /// The excerpts made from one session whose previews are still saved,
    /// newest first.
    pub fn saved_views(&self, source: &str) -> Vec<SavedView> {
        let mut views: Vec<SavedView> = fs::read_dir(self.dir.join("exports"))
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let id = path
                    .file_name()?
                    .to_str()?
                    .strip_suffix(".json")?
                    .to_owned();
                let record = self.view_record(&id)?;
                let draft = record.draft.filter(|_| record.source == source)?;
                self.previews
                    .join(format!("{id}.json"))
                    .is_file()
                    .then_some(SavedView {
                        id,
                        title: draft.title,
                        saved_at: record.saved_at,
                    })
            })
            .collect();
        views.sort_by_key(|view| std::cmp::Reverse(view.saved_at));
        views
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::Selection;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("slink-local-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("previews")).unwrap();
        dir
    }

    #[test]
    fn a_draft_is_saved_one_revision_at_a_time() {
        let home = scratch("drafts");
        let drafts = Drafts::beside(&home.join("previews"));
        let (revision, empty) = drafts.read_draft("source").unwrap();
        assert_eq!(
            (revision, empty.title.as_str(), empty.items.len()),
            (0, "Shared session excerpt", 0)
        );
        let draft = Draft {
            title: "Finding".into(),
            note: "why".into(),
            items: vec![Selection {
                id: "u0-out-0-0".into(),
                start: Some(2),
                end: Some(9),
            }],
            primary: "u0-out-0-0".into(),
        };
        drafts.write_draft("source", 1, &draft).unwrap();
        assert_eq!(drafts.read_draft("source").unwrap(), (1, draft.clone()));
        // The same number again is someone else's save, whatever it holds.
        assert!(matches!(
            drafts.write_draft("source", 1, &Draft::default()),
            Err(SaveError::Taken)
        ));
        drafts.write_draft("source", 2, &Draft::default()).unwrap();
        assert_eq!(drafts.read_draft("source").unwrap(), (2, Draft::default()));
        let left: Vec<_> = fs::read_dir(home.join("drafts/source"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left.len(), 2, "no temporary file is left: {left:?}");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_title_follows_the_session_across_snapshots() {
        let home = scratch("titles");
        let drafts = Drafts::beside(&home.join("previews"));
        let key = session_title_key("claude-code", "0c3b8f0e");
        assert!(key.starts_with("session-") && key.len() == 40);
        assert_eq!(session_title_key("", "x"), "");
        assert_eq!(drafts.title_for("first", &key), "");
        drafts.set_title("first", &key, "Typed").unwrap();
        assert_eq!(drafts.title_for("first", &key), "Typed");
        assert_eq!(
            drafts.title_for("second", &key),
            "Typed",
            "a later snapshot keeps the name"
        );
        assert_eq!(drafts.title_for("second", ""), "");
        drafts.set_title("lone", "", "Only here").unwrap();
        assert_eq!(drafts.title_for("lone", ""), "Only here");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn saved_views_are_those_of_one_session_still_on_disk() {
        let home = scratch("views");
        let drafts = Drafts::beside(&home.join("previews"));
        let draft = |title: &str| Draft {
            title: title.into(),
            ..Draft::default()
        };
        for (id, source, at) in [
            ("a", "one", 1),
            ("b", "one", 3),
            ("c", "two", 2),
            ("gone", "one", 4),
        ] {
            drafts.record_view(id, source, &draft(id), at).unwrap();
            if id != "gone" {
                fs::write(home.join("previews").join(format!("{id}.json")), "{}").unwrap();
            }
        }
        let ids: Vec<String> = drafts
            .saved_views("one")
            .into_iter()
            .map(|view| view.id)
            .collect();
        assert_eq!(ids, ["b", "a"]);
        assert_eq!(drafts.edit_source("c").as_deref(), Some("two"));
        assert_eq!(drafts.edit_source("../c"), None);
        assert_eq!(drafts.edit_source("absent"), None);
        fs::remove_dir_all(home).unwrap();
    }
}
