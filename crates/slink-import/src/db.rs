//! Reading a harness's SQLite database: opened read-only, and each row as
//! the JSON its importer wants, so a row here is the row the JavaScript and
//! Go importers saw.

use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Map, Value, json};

/// Opens the database read-only. A harness that is not installed has none,
/// which is an ordinary answer and not a failure to report.
pub(crate) fn open(path: &Path) -> Result<Connection, String> {
    if !path.is_file() {
        return Err(format!("{}: no such file", path.display()));
    }
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| error.to_string())
}

/// Runs `query` and returns each row as an object of its columns. Integers
/// and reals become numbers, text and blobs become strings.
pub(crate) fn rows(
    db: &Connection,
    query: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<Value>, String> {
    let failed = |error: rusqlite::Error| error.to_string();
    let mut statement = db.prepare(query).map_err(failed)?;
    let columns: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut found = statement.query(args).map_err(failed)?;
    let mut out = Vec::new();
    while let Some(row) = found.next().map_err(failed)? {
        let mut object = Map::new();
        for (index, column) in columns.iter().enumerate() {
            let value = match row.get_ref(index).map_err(failed)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(number) => json!(number),
                ValueRef::Real(number) => json!(number),
                ValueRef::Text(text) | ValueRef::Blob(text) => json!(String::from_utf8_lossy(text)),
            };
            object.insert(column.clone(), value);
        }
        out.push(Value::Object(object));
    }
    Ok(out)
}

/// The names of a table's columns: harnesses add columns between versions,
/// and a query may only name the ones that are there.
pub(crate) fn columns(db: &Connection, table: &str) -> Vec<String> {
    // The table's name comes from this crate, never from a session.
    rows(db, &format!("PRAGMA table_info({table})"), &[])
        .unwrap_or_default()
        .iter()
        .filter_map(|row| row["name"].as_str().map(str::to_owned))
        .collect()
}

/// A column that holds JSON as text, parsed. Text that is not JSON reads as
/// an empty object, and a value that is not text stays as it is.
pub(crate) fn parsed(value: &Value) -> Value {
    match value.as_str() {
        Some(text) => slink_format::parse(text).unwrap_or_else(|_| json!({})),
        None => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_is_an_object_of_its_columns() {
        let dir = std::env::temp_dir().join(format!("slink-import-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rows.db");
        let _ = std::fs::remove_file(&path);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "create table part (id text, n integer, x real, data blob, gone text);
             insert into part values ('a', 7, 1.5, x'7b7d', null);",
        )
        .unwrap();
        drop(db);
        let db = open(&path).unwrap();
        assert_eq!(
            rows(&db, "select * from part where id = ?1", &[&"a"]).unwrap(),
            [json!({"id": "a", "n": 7, "x": 1.5, "data": "{}", "gone": null})]
        );
        assert_eq!(columns(&db, "part"), ["id", "n", "x", "data", "gone"]);
        assert!(
            db.execute("insert into part (id) values ('b')", [])
                .is_err(),
            "opened read-only"
        );
        assert_eq!(parsed(&json!("{\"a\":1}")), json!({"a": 1}));
        assert_eq!(parsed(&json!("not json")), json!({}));
        assert_eq!(parsed(&json!(7)), json!(7));
        assert!(open(&dir.join("absent.db")).is_err());
        // Windows does not delete a file that is open.
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
