use std::path::PathBuf;

use tempfile::TempDir;

pub struct TempWorkspace {
    pub root: TempDir,
}

impl TempWorkspace {
    pub fn new() -> Self {
        Self {
            root: tempfile::tempdir().expect("create temp workspace"),
        }
    }

    pub fn path(&self, child: &str) -> PathBuf {
        self.root.path().join(child)
    }
}

impl Default for TempWorkspace {
    fn default() -> Self {
        Self::new()
    }
}
