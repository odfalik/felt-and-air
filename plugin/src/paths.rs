//! Per-user files are resolved at run time, never relative to the build checkout or CWD.
use std::path::PathBuf;

pub(crate) fn support_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join("Library/Application Support/Felt & Air"))
}
