//! Reading `testdata/protocol`, for the tests that hold this crate to it,
//! and the generator the tests that choose their own keys and damage share.

#![allow(dead_code)]

use std::fs;
use std::path::Path;

use serde_json::Value;
use slink_protocol::Signed;

pub fn read(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/protocol")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    serde_json::from_str(&text).expect("the case file is JSON")
}

/// One verdict for every implementation, or one for each.
pub fn expected(case: &Value) -> &str {
    case["expect"]
        .as_str()
        .or_else(|| case["expect"]["rust"].as_str())
        .expect("the case has a verdict for rust")
}

pub fn text<'a>(case: &'a Value, field: &str) -> &'a str {
    case[field]
        .as_str()
        .unwrap_or_else(|| panic!("{}: no {field}", case["name"]))
}

/// A statement written out in a case.
pub fn signed(value: &Value) -> Signed {
    Signed {
        payload: text(value, "payload").to_owned(),
        signature: text(value, "signature").to_owned(),
    }
}

pub fn report(failures: Vec<String>) {
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// xorshift64: enough to choose keys nobody will use and where to damage
/// something, the same on every run.
pub struct Random(pub u64);

impl Random {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit.max(1) as u64) as usize
    }

    pub fn bytes(&mut self, length: usize) -> Vec<u8> {
        (0..length).map(|_| self.next() as u8).collect()
    }

    pub fn array<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0; N];
        out.iter_mut().for_each(|byte| *byte = self.next() as u8);
        out
    }

    /// Text of up to `limit` characters, some of them outside ASCII and some
    /// that JSON escapes.
    pub fn text(&mut self, limit: usize) -> String {
        const SOME: &[char] = &[
            'a',
            'Z',
            '0',
            '/',
            ' ',
            '"',
            '\\',
            '<',
            '&',
            '\n',
            '\u{0}',
            '\u{7f}',
            '\u{e9}',
            '\u{2028}',
            '\u{1f4bb}',
        ];
        (0..self.below(limit + 1))
            .map(|_| SOME[self.below(SOME.len())])
            .collect()
    }
}
