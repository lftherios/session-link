//! A signed payload's JSON. A payload is read only where the Go CLI and the
//! JavaScript verifiers would read the same thing from it: a field is taken
//! by its exact name and in its exact type, nothing is filled in, and no
//! object says a thing twice. One is written as the Go CLI writes it, byte
//! for byte.

use std::fmt::{self, Write as _};

use serde::de::{Deserialize, Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

/// The largest whole number JavaScript holds exactly. A count above it reads
/// as a different number there.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// The payload as JSON, unless an object in it holds a key twice. Go and
/// JavaScript take the last of two, except that Go keeps the one before when
/// the last is null, so a repeated key can be two statements under one
/// signature.
pub(crate) fn parse(raw: &[u8]) -> Option<Value> {
    serde_json::from_slice::<Once>(raw).ok().map(|once| once.0)
}

/// A JSON value in which every object has each key once.
struct Once(Value);

impl<'de> Deserialize<'de> for Once {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(OnceVisitor)
    }
}

struct OnceVisitor;

impl<'de> Visitor<'de> for OnceVisitor {
    type Value = Once;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Once, E> {
        Ok(Once(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Once, E> {
        Ok(Once(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Once, E> {
        Ok(Once(Value::Number(value.into())))
    }

    fn visit_f64<E: Error>(self, value: f64) -> Result<Once, E> {
        Number::from_f64(value)
            .map(|number| Once(Value::Number(number)))
            .ok_or_else(|| E::custom("a number JSON cannot hold"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Once, E> {
        Ok(Once(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Once, E> {
        Ok(Once(Value::String(value)))
    }

    fn visit_unit<E>(self) -> Result<Once, E> {
        Ok(Once(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Once, A::Error> {
        let mut list = Vec::new();
        while let Some(Once(item)) = items.next_element()? {
            list.push(item);
        }
        Ok(Once(Value::Array(list)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<Once, A::Error> {
        let mut object = Map::new();
        while let Some(key) = entries.next_key::<String>()? {
            let Once(value) = entries.next_value()?;
            if object.insert(key, value).is_some() {
                return Err(A::Error::custom("a key given twice"));
            }
        }
        Ok(Once(Value::Object(object)))
    }
}

pub(crate) struct Fields<'a>(&'a Map<String, Value>);

impl<'a> Fields<'a> {
    /// The fields of a JSON object, unless one of its keys is a field's name
    /// written in another case. Go takes such a key for the field and
    /// JavaScript does not, so the two would read different statements.
    pub(crate) fn of(value: &'a Value, names: &[&str]) -> Option<Fields<'a>> {
        let map = value.as_object()?;
        let misspelt = |key: &str| {
            !names.contains(&key)
                && names
                    .iter()
                    .any(|name| key.chars().map(fold).eq(name.chars()))
        };
        if map.keys().any(|key| misspelt(key)) {
            return None;
        }
        Some(Fields(map))
    }

    pub(crate) fn string(&self, name: &str) -> Option<&'a str> {
        self.0.get(name)?.as_str()
    }

    /// A string that may be left out or null. Empty counts as left out, which
    /// is how every implementation writes "none".
    pub(crate) fn optional_string(&self, name: &str) -> Option<Option<&'a str>> {
        match self.0.get(name) {
            None | Some(Value::Null) => Some(None),
            Some(Value::String(text)) if text.is_empty() => Some(None),
            Some(Value::String(text)) => Some(Some(text)),
            Some(_) => None,
        }
    }

    /// A count written as a whole number: `1`, never `1.0`, `1e0` or `-0`.
    pub(crate) fn integer(&self, name: &str) -> Option<u64> {
        self.0
            .get(name)?
            .as_u64()
            .filter(|&number| number <= MAX_SAFE_INTEGER)
    }

    pub(crate) fn array(&self, name: &str) -> Option<&'a [Value]> {
        self.0.get(name)?.as_array().map(Vec::as_slice)
    }
}

/// A JSON object being written: its fields in the order they are given, with
/// nothing between them, and text escaped as Go's `encoding/json` escapes it.
/// A statement is signed as bytes, so no reader depends on the spelling; it
/// is Go's so that the two clients write one statement the same way.
pub(crate) struct Object(String);

impl Object {
    pub(crate) fn new() -> Object {
        Object(String::from("{"))
    }

    fn name(&mut self, name: &str) {
        if self.0.len() > 1 {
            self.0.push(',');
        }
        write_text(&mut self.0, name);
        self.0.push(':');
    }

    pub(crate) fn text(mut self, name: &str, value: &str) -> Object {
        self.name(name);
        write_text(&mut self.0, value);
        self
    }

    pub(crate) fn count(mut self, name: &str, value: u64) -> Object {
        self.name(name);
        write!(self.0, "{value}").expect("writing to a string");
        self
    }

    /// A field whose value is JSON already: an object, or a list of them.
    pub(crate) fn json(mut self, name: &str, value: &str) -> Object {
        self.name(name);
        self.0.push_str(value);
        self
    }

    pub(crate) fn finish(mut self) -> String {
        self.0.push('}');
        self.0
    }
}

/// A list of values that are JSON already.
pub(crate) fn list(items: impl IntoIterator<Item = String>) -> String {
    let items: Vec<String> = items.into_iter().collect();
    format!("[{}]", items.join(","))
}

/// Text as Go writes a JSON string: the short escapes it has, `\u00xx` for
/// the other control characters, and the same for `<`, `>`, `&` and the two
/// line separators, which Go escapes for the sake of HTML.
fn write_text(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                write!(out, "\\u{:04x}", character as u32).expect("writing to a string");
            }
            control if (control as u32) < 0x20 => {
                write!(out, "\\u{:04x}", control as u32).expect("writing to a string");
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// How Go's `encoding/json` folds a key before matching it to a field: ASCII
/// case, and the two other characters whose simple case folding is an ASCII
/// letter.
fn fold(character: char) -> char {
    match character {
        '\u{17f}' => 's',
        '\u{212a}' => 'k',
        other => other.to_ascii_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NAMES: &[&str] = &["account", "seq", "kind", "recovery_box"];

    #[test]
    fn a_field_is_read_in_its_type_or_not_at_all() {
        let value = json!({"account": "a", "seq": 3, "kind": null, "x": {"y": 1}});
        let fields = Fields::of(&value, NAMES).unwrap();
        assert_eq!(fields.string("account"), Some("a"));
        assert_eq!(fields.string("seq"), None);
        assert_eq!(fields.string("kind"), None);
        assert_eq!(fields.string("recovery_box"), None);
        assert_eq!(fields.integer("seq"), Some(3));
        assert_eq!(fields.integer("account"), None);
        assert_eq!(fields.optional_string("kind"), Some(None));
        assert_eq!(fields.optional_string("recovery_box"), Some(None));
        assert_eq!(fields.optional_string("account"), Some(Some("a")));
        assert_eq!(fields.optional_string("seq"), None);
        assert!(Fields::of(&json!([]), NAMES).is_none());
        assert!(Fields::of(&json!("account"), NAMES).is_none());
    }

    #[test]
    fn a_key_given_twice_refuses_the_payload() {
        for text in [
            r#"{"a":1,"b":[true,null,-2,1.5,"x",{"c":{}}]}"#,
            r#"{"a":1,"A":1}"#,
            r#"[{"a":1},{"a":1}]"#,
            "null",
        ] {
            assert_eq!(
                parse(text.as_bytes()),
                serde_json::from_str(text).ok(),
                "{text}"
            );
        }
        for text in [
            r#"{"a":1,"a":1}"#,
            r#"{"a":1,"a":null}"#,
            r#"{"a":null,"b":2,"a":1}"#,
            r#"{"b":{"a":1,"a":2}}"#,
            r#"[[{"a":1,"a":2}]]"#,
            // The same key, once written with an escape.
            "{\"a\":1,\"\\u0061\":2}",
            r#"{"a":1}x"#,
            "",
        ] {
            assert_eq!(parse(text.as_bytes()), None, "{text}");
        }
        // Not UTF-8, inside a string and outside one.
        assert_eq!(parse(b"{\"a\":\"\xff\"}"), None);
        assert_eq!(parse(b"\xef\xbb\xbf{}"), None);
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert_eq!(parse(deep.as_bytes()), None);
    }

    #[test]
    fn a_count_is_a_whole_number_as_written() {
        for (text, want) in [
            ("1", Some(1)),
            ("0", Some(0)),
            ("9007199254740991", Some(MAX_SAFE_INTEGER)),
            ("9007199254740992", None),
            ("1.0", None),
            ("1e0", None),
            ("-0", None),
            ("-1", None),
            ("\"1\"", None),
            ("true", None),
        ] {
            let value = parse(format!("{{\"seq\":{text}}}").as_bytes()).unwrap();
            let fields = Fields::of(&value, NAMES).unwrap();
            assert_eq!(fields.integer("seq"), want, "{text}");
        }
    }

    #[test]
    fn an_object_is_written_as_go_writes_it() {
        let inner = Object::new().text("id", "a").finish();
        let text = Object::new()
            .text(
                "name",
                "a\"b\\c\u{1}\u{8}\u{c}\n\r\t<>&\u{2028}\u{2029}\u{7f}\u{e9}\u{1f4bb}",
            )
            .count("seq", 0)
            .count("epoch", (1 << 53) - 1)
            .json("devices", &list([inner.clone(), inner]))
            .json("none", &list([]))
            .finish();
        assert_eq!(
            text,
            "{\"name\":\"a\\\"b\\\\c\\u0001\\b\\f\\n\\r\\t\\u003c\\u003e\\u0026\\u2028\\u2029\u{7f}\u{e9}\u{1f4bb}\",\
             \"seq\":0,\"epoch\":9007199254740991,\
             \"devices\":[{\"id\":\"a\"},{\"id\":\"a\"}],\"none\":[]}"
        );
        assert_eq!(Object::new().finish(), "{}");
        // What is written reads back as what was given.
        let value = parse(text.as_bytes()).unwrap();
        assert_eq!(
            value["name"],
            "a\"b\\c\u{1}\u{8}\u{c}\n\r\t<>&\u{2028}\u{2029}\u{7f}\u{e9}\u{1f4bb}"
        );
    }

    #[test]
    fn a_field_name_in_another_case_refuses_the_object() {
        for key in [
            "Account",
            "ACCOUNT",
            "SEQ",
            "\u{17f}eq",
            "\u{212a}ind",
            "Recovery_Box",
        ] {
            let mut map = Map::new();
            map.insert(key.to_owned(), json!(1));
            assert!(Fields::of(&Value::Object(map), NAMES).is_none(), "{key}");
        }
        // Not a field's name in any case, so only an unknown field.
        for key in ["accounts", "recovery-box", "s\u{eb}q", "", "kind "] {
            let mut map = Map::new();
            map.insert(key.to_owned(), json!(1));
            assert!(Fields::of(&Value::Object(map), NAMES).is_some(), "{key}");
        }
    }
}
