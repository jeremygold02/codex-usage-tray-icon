//! The Claude CLI's credentials file: reading its OAuth login and saving
//! rotated tokens back without disturbing anything else the CLI keeps there.

use std::io::{self, Write};
use std::path::PathBuf;

use serde_json::{Map, Value};

use super::files::{self, TemporaryFile};
use super::MAX_BYTES;

/// A JSON object whose every member is kept, so fields this app does not know
/// about survive a rewrite.
pub(super) type Object = Map<String, Value>;

const OAUTH_KEY: &str = "claudeAiOauth";
/// The members a token rotation changes; everything else belongs to the CLI.
const ROTATED_KEYS: [&str; 3] = ["accessToken", "refreshToken", "expiresAt"];

pub(super) struct CredentialFile {
    path: PathBuf,
}

impl CredentialFile {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Where the CLI keeps its credentials: under `CLAUDE_CONFIG_DIR` when
    /// that is set, otherwise under `.claude` in the user's profile. Empty,
    /// and so never readable, when neither location is known.
    pub(super) fn default_path() -> PathBuf {
        std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|value| !value.to_string_lossy().trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".claude"))
            })
            .map(|directory| directory.join(".credentials.json"))
            .unwrap_or_default()
    }

    /// The `claudeAiOauth` object, or `None` when the file has none. Fails
    /// when the file is missing, too large or not JSON.
    pub(super) fn load_oauth(&self) -> io::Result<Option<Object>> {
        Ok(oauth_of(&self.load()?).cloned())
    }

    /// Saves `replacement`'s tokens in place of `original`'s and returns the
    /// login that is in the file afterwards. If the CLI has stored a different
    /// login in the meantime, the file is left alone and that login is
    /// returned instead.
    pub(super) fn persist_rotation(
        &self,
        original: &Object,
        replacement: &Object,
    ) -> io::Result<Option<Object>> {
        let mut root = self.load()?;
        let mut merged = match oauth_of(&root) {
            Some(latest) if same_tokens(Some(latest), Some(original)) => latest.clone(),
            latest => return Ok(latest.cloned()),
        };
        // Preserve other credential fields changed by the CLI while the request ran.
        for key in ROTATED_KEYS {
            match replacement.get(key) {
                Some(value) => merged.insert(key.to_string(), value.clone()),
                None => merged.remove(key),
            };
        }
        root.insert(OAUTH_KEY.to_string(), Value::Object(merged.clone()));

        let temporary = TemporaryFile::beside(&self.path);
        {
            let mut file = files::create_like(&self.path, temporary.path())?;
            file.write_all(&serde_json::to_vec(&root)?)?;
            file.sync_all()?;
        }
        // Re-read before replacing: the CLI may have signed in again while
        // the new file was being written.
        let latest = self.load_oauth()?;
        if !same_tokens(latest.as_ref(), Some(original)) {
            return Ok(latest);
        }
        if let Err(error) = files::replace_file(&self.path, temporary.path()) {
            // A failed swap can remove the old file and leave the new one
            // under its temporary name. That copy is then the only login
            // left, so it is moved into place rather than deleted.
            let old_file_is_gone = matches!(self.path.try_exists(), Ok(false));
            if old_file_is_gone && std::fs::rename(temporary.path(), &self.path).is_ok() {
                return Ok(Some(merged));
            }
            return Err(error);
        }
        Ok(Some(merged))
    }

    /// The file's top-level object. Any other JSON value holds no login, just
    /// like an empty object.
    fn load(&self) -> io::Result<Object> {
        let bytes = files::read_limited(&self.path, MAX_BYTES)?;
        Ok(match serde_json::from_slice(&bytes)? {
            Value::Object(root) => root,
            _ => Object::new(),
        })
    }
}

fn oauth_of(root: &Object) -> Option<&Object> {
    root.get(OAUTH_KEY)?.as_object()
}

/// A string member, or `None` when it is missing or not a string.
pub(super) fn text<'a>(source: &'a Object, key: &str) -> Option<&'a str> {
    source.get(key)?.as_str()
}

/// True when both logins exist and belong to the same token generation.
pub(super) fn same_tokens(left: Option<&Object>, right: Option<&Object>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => ["accessToken", "refreshToken"]
            .iter()
            .all(|key| text(left, key) == text(right, key)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use super::super::files::tests::{create_with_dacl, dacl_of, TestDirectory, PROTECTED_DACL};

    const ORIGINAL: &str = r#"{"unrelatedSetting":"keep-root","claudeAiOauth":{
        "accessToken":"original","refreshToken":"refresh","expiresAt":1,
        "subscriptionType":"max","scopes":["user:profile","user:inference"]}}"#;

    fn object(json: &str) -> Object {
        match serde_json::from_str(json).unwrap() {
            Value::Object(object) => object,
            other => panic!("expected an object, got {other}"),
        }
    }

    fn rotated(original: &Object) -> Object {
        let mut replacement = original.clone();
        replacement.insert("accessToken".to_string(), "rotated-access".into());
        replacement.insert("refreshToken".to_string(), "rotated-refresh".into());
        replacement.remove("expiresAt");
        replacement
    }

    #[test]
    fn a_saved_rotation_keeps_other_fields_and_the_access_control_list() {
        let directory = TestDirectory::new("ClaudeCredentialTests");
        let path = directory.file("credentials.json");
        create_with_dacl(&path, PROTECTED_DACL);
        fs::write(&path, ORIGINAL).unwrap();
        let access_control = dacl_of(&path);
        assert!(access_control.starts_with("D:P"));

        let credentials = CredentialFile::new(path.clone());
        let original = credentials.load_oauth().unwrap().unwrap();
        let saved = credentials
            .persist_rotation(&original, &rotated(&original))
            .unwrap()
            .unwrap();

        let root = object(&fs::read_to_string(&path).unwrap());
        assert_eq!(root["unrelatedSetting"], "keep-root");
        assert_eq!(oauth_of(&root), Some(&saved));
        assert_eq!(text(&saved, "accessToken"), Some("rotated-access"));
        assert_eq!(text(&saved, "refreshToken"), Some("rotated-refresh"));
        assert_eq!(text(&saved, "subscriptionType"), Some("max"));
        assert_eq!(saved["scopes"].as_array().map(Vec::len), Some(2));
        assert!(!saved.contains_key("expiresAt"));
        assert_eq!(dacl_of(&path), access_control);
        assert!(directory.leftovers().is_empty());
    }

    #[test]
    fn a_login_replaced_by_the_cli_is_left_alone() {
        let directory = TestDirectory::new("ClaudeCredentialTests");
        let path = directory.file("credentials.json");
        fs::write(&path, ORIGINAL).unwrap();
        let credentials = CredentialFile::new(path.clone());
        let original = credentials.load_oauth().unwrap().unwrap();

        let from_cli = ORIGINAL.replace("original", "cli-access");
        fs::write(&path, &from_cli).unwrap();
        let latest = credentials
            .persist_rotation(&original, &rotated(&original))
            .unwrap()
            .unwrap();
        assert_eq!(text(&latest, "accessToken"), Some("cli-access"));
        assert_eq!(fs::read_to_string(&path).unwrap(), from_cli);
        assert!(directory.leftovers().is_empty());
    }

    #[test]
    fn unusable_files_hold_no_login() {
        let directory = TestDirectory::new("ClaudeCredentialTests");
        let path = directory.file("credentials.json");
        let credentials = CredentialFile::new(path.clone());
        assert!(credentials.load_oauth().is_err());

        for (json, readable) in [
            ("not json", false),
            ("[]", true),
            ("{}", true),
            (r#"{"claudeAiOauth":"text"}"#, true),
        ] {
            fs::write(&path, json).unwrap();
            let loaded = credentials.load_oauth();
            assert_eq!(loaded.is_ok(), readable, "{json}");
            assert!(loaded.ok().flatten().is_none(), "{json}");
        }

        fs::write(
            &path,
            format!("{{\"padding\":\"{}\"}}", "x".repeat(MAX_BYTES)),
        )
        .unwrap();
        assert!(credentials.load_oauth().is_err());
        assert!(CredentialFile::new(PathBuf::new()).load_oauth().is_err());
    }

    #[test]
    fn token_generations_are_compared_by_both_tokens() {
        let login = |access: &str, refresh: &str| {
            object(&format!(
                r#"{{"accessToken":"{access}","refreshToken":"{refresh}","expiresAt":5}}"#
            ))
        };
        let original = login("a", "r");
        assert!(same_tokens(Some(&original), Some(&login("a", "r"))));
        assert!(!same_tokens(Some(&original), Some(&login("b", "r"))));
        assert!(!same_tokens(Some(&original), Some(&login("a", "s"))));
        assert!(!same_tokens(Some(&original), None));
        assert!(!same_tokens(None, Some(&original)));
        assert!(!same_tokens(None, None));
    }
}
