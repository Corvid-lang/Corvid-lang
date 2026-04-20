use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildTarget {
    Native,
    Cdylib,
    Staticlib,
}

pub fn object_extension() -> &'static str {
    if cfg!(windows) {
        "obj"
    } else {
        "o"
    }
}

pub fn shared_library_extension() -> &'static str {
    if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(windows) {
        "dll"
    } else {
        "so"
    }
}

pub fn static_library_extension() -> &'static str {
    if cfg!(windows) {
        "lib"
    } else {
        "a"
    }
}

pub fn shared_library_path_for(out_dir: &Path, stem: &str) -> PathBuf {
    if cfg!(windows) {
        out_dir.join(format!("{stem}.{}", shared_library_extension()))
    } else {
        out_dir.join(format!("lib{stem}.{}", shared_library_extension()))
    }
}

pub fn static_library_path_for(out_dir: &Path, stem: &str) -> PathBuf {
    if cfg!(windows) {
        out_dir.join(format!("{stem}.{}", static_library_extension()))
    } else {
        out_dir.join(format!("lib{stem}.{}", static_library_extension()))
    }
}
