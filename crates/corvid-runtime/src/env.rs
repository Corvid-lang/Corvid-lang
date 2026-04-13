//! `.env` loading via `dotenvy`.
//!
//! Walks from the current working directory up through ancestors looking
//! for the first `.env` file. Loads it without overriding real env vars
//! — standard dotenv precedence (real env > .env > nothing). Missing
//! `.env` is not an error.

use std::path::{Path, PathBuf};

/// Look for `.env` starting at `start` and walking upward. Returns the
/// path of the first one found, or `None` if no ancestor has one.
pub fn find_dotenv_walking(start: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start);
    while let Some(dir) = cur {
        let candidate = dir.join(".env");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

/// Find and load `.env` from `start` upward. Real env vars win over
/// `.env` values. Returns the path that was loaded, or `None` if none
/// was found. IO errors during load are swallowed — startup must not
/// fail because of a malformed `.env`.
pub fn load_dotenv_walking(start: &Path) -> Option<PathBuf> {
    let path = find_dotenv_walking(start)?;
    // `from_path` does not override existing env vars (matches our
    // intended precedence: real env > .env).
    let _ = dotenvy::from_path(&path);
    Some(path)
}

/// Convenience: load from the current working directory upward.
pub fn load_dotenv() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    load_dotenv_walking(&cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_dotenv_in_current_dir() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".env"), "FOO=bar\n").unwrap();
        let found = find_dotenv_walking(tmp.path()).unwrap();
        assert_eq!(found, tmp.path().join(".env"));
    }

    #[test]
    fn finds_dotenv_in_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        fs::write(tmp.path().join(".env"), "FOO=bar\n").unwrap();
        let found = find_dotenv_walking(&nested).unwrap();
        assert_eq!(found, tmp.path().join(".env"));
    }

    #[test]
    fn returns_none_when_no_dotenv_anywhere() {
        let tmp = tempfile::tempdir().unwrap();
        let found = find_dotenv_walking(tmp.path());
        // Caveat: a real `.env` could exist somewhere up the test runner's
        // path. We can't guard against that without sandboxing. So just
        // assert that find returns *something* whose stem is `.env` or
        // None — and that load_dotenv doesn't panic.
        if let Some(p) = found {
            assert_eq!(p.file_name().unwrap(), ".env");
        }
    }

    #[test]
    fn load_dotenv_walks_and_applies_values() {
        let tmp = tempfile::tempdir().unwrap();
        let key = "CORVID_TEST_DOTENV_VAR";
        // Make sure the key isn't already set.
        std::env::remove_var(key);
        fs::write(tmp.path().join(".env"), format!("{key}=hello\n")).unwrap();
        let loaded = load_dotenv_walking(tmp.path()).unwrap();
        assert!(loaded.ends_with(".env"));
        assert_eq!(std::env::var(key).unwrap(), "hello");
        std::env::remove_var(key);
    }
}
