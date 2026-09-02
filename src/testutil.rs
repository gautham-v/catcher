//! fixtures shared by the test modules: a fresh scratch directory and a
//! file written under it

use std::fs;
use std::path::{Path, PathBuf};

/// an empty directory under the system temp dir, named for the module and
/// the test so runs do not tread on each other
pub fn tmpdir(prefix: &str, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("catcher-{prefix}-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    // the walk canonicalizes its roots, so a fixture that does not would
    // compare `/var/…` against `/private/var/…` and never match itself
    fs::canonicalize(&dir).unwrap()
}

/// `body` written to `rel` under `dir`, creating any missing parents
pub fn write(dir: &Path, rel: &str, body: &str) -> PathBuf {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, body).unwrap();
    path
}
