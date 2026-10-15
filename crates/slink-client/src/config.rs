//! `config.json` in the slink home: the API key `slink login` saved, the
//! server it is for, and who signed in. Only the user can read it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use slink_local::write_private;

/// Where publishing goes when nothing says otherwise.
pub const DEFAULT_SERVER: &str = "https://session.link";

/// The saved sign-in. A field the file does not hold, or holds as anything
/// but text, is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub user_id: String,
    pub api_key: String,
    pub server: String,
    /// The handle the account signed in with, when the browser minted the key.
    pub login: String,
}

pub fn file(home: &Path) -> PathBuf {
    home.join("config.json")
}

/// The saved sign-in, or an empty one when there is none to read.
pub fn read(home: &Path) -> Config {
    let saved = fs::read_to_string(file(home))
        .ok()
        .and_then(|text| slink_format::parse(&text).ok())
        .unwrap_or(Value::Null);
    let text = |name: &str| saved[name].as_str().unwrap_or_default().to_owned();
    Config {
        user_id: text("user_id"),
        api_key: text("api_key"),
        server: text("server"),
        login: text("login"),
    }
}

/// Saves `config` whole, as Go's `json.MarshalIndent` writes it, and says
/// where.
pub fn write(home: &Path, config: &Config) -> io::Result<PathBuf> {
    let text = |value: &str| slink_format::encode(&Value::String(value.to_owned()));
    let mut fields = Vec::new();
    if !config.user_id.is_empty() {
        fields.push(format!("  \"user_id\": {}", text(&config.user_id)));
    }
    fields.push(format!("  \"api_key\": {}", text(&config.api_key)));
    fields.push(format!("  \"server\": {}", text(&config.server)));
    if !config.login.is_empty() {
        fields.push(format!("  \"login\": {}", text(&config.login)));
    }
    let path = file(home);
    write_private(
        &path,
        format!("{{\n{}\n}}\n", fields.join(",\n")).as_bytes(),
    )?;
    Ok(path)
}

/// The server and key a command uses: its flag, then `SLINK_SERVER` or
/// `SLINK_API_KEY`, then the saved sign-in, then the default server and no
/// key.
///
/// The saved key goes only to the server it was issued for. A flag or a
/// `SLINK_SERVER` naming another, say from a project's environment, gets no
/// key and signs in there; Go sends the saved key anyway.
pub fn resolve_target(home: &Path, server_flag: &str, key_flag: &str) -> (String, String) {
    let saved = read(home);
    let variable = |name: &str| std::env::var(name).unwrap_or_default();
    let first = |values: [&str; 3]| {
        values
            .into_iter()
            .find(|value| !value.is_empty())
            .unwrap_or_default()
            .to_owned()
    };
    let server = first([server_flag, &variable("SLINK_SERVER"), &saved.server]);
    let server = if server.is_empty() {
        DEFAULT_SERVER.to_owned()
    } else {
        server
    };
    let issued = if saved.server.is_empty() {
        DEFAULT_SERVER
    } else {
        &saved.server
    };
    let same = |one: &str, other: &str| {
        one.trim_end_matches('/')
            .eq_ignore_ascii_case(other.trim_end_matches('/'))
    };
    let saved_key = if same(issued, &server) {
        saved.api_key.as_str()
    } else {
        ""
    };
    let key = first([key_flag, &variable("SLINK_API_KEY"), saved_key]);
    (server, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_is_written_as_go_writes_it_and_read_back() {
        let home = std::env::temp_dir().join(format!("slink-client-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let config = Config {
            user_id: String::new(),
            api_key: "rk_a<b".to_owned(),
            server: "https://session.link".to_owned(),
            login: "tester".to_owned(),
        };
        let path = write(&home, &config).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\n  \"api_key\": \"rk_a\\u003cb\",\n  \"server\": \"https://session.link\",\n  \"login\": \"tester\"\n}\n"
        );
        assert_eq!(read(&home), config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::write(&path, "{\"api_key\":5,\"server\":\"https://x.test\"}").unwrap();
        assert_eq!(
            read(&home),
            Config {
                server: "https://x.test".to_owned(),
                ..Config::default()
            }
        );
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_saved_key_goes_only_to_the_server_it_was_issued_for() {
        let home = std::env::temp_dir().join(format!("slink-client-target-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let config = Config {
            api_key: "rk_saved".to_owned(),
            server: "https://session.link".to_owned(),
            ..Config::default()
        };
        write(&home, &config).unwrap();
        let target = |server: &str, key: &str| resolve_target(&home, server, key);
        for server in ["", "https://session.link/", "HTTPS://Session.Link"] {
            let expected = if server.is_empty() {
                "https://session.link"
            } else {
                server
            };
            assert_eq!(
                target(server, ""),
                (expected.to_owned(), "rk_saved".to_owned())
            );
        }
        let elsewhere = "https://elsewhere.example";
        assert_eq!(target(elsewhere, ""), (elsewhere.to_owned(), String::new()));
        assert_eq!(
            target(elsewhere, "rk_given"),
            (elsewhere.to_owned(), "rk_given".to_owned())
        );
        fs::remove_dir_all(&home).unwrap();
    }
}
