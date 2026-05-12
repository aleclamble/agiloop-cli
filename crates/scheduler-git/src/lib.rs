use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("path is not a git repository: {0}")]
    NotRepository(String),
    #[error("base ref not found: {0}")]
    BaseRefNotFound(String),
    #[error("git command failed: {0}")]
    Command(String),
    #[error("filesystem error: {0}")]
    Filesystem(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub repo_path: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: String,
}

pub fn canonicalize_repo(path: &Path) -> Result<PathBuf, GitError> {
    let canonical = path
        .canonicalize()
        .map_err(|error| GitError::Filesystem(error.to_string()))?;
    if !is_git_repo(&canonical) {
        return Err(GitError::NotRepository(canonical.display().to_string()));
    }
    Ok(canonical)
}

pub fn is_git_repo(path: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub fn fetch_repo(repo_path: &Path) -> Result<(), GitError> {
    let remotes = git_stdout(repo_path, &["remote"])?;
    if remotes.trim().is_empty() {
        return Ok(());
    }
    git_stdout(repo_path, &["fetch", "--all", "--prune", "--tags"])?;
    Ok(())
}

pub fn resolve_base_ref(repo_path: &Path, base_ref: &str) -> Result<String, GitError> {
    let base_ref = base_ref.trim();
    let candidates = if base_ref.contains('/') || base_ref == "HEAD" {
        vec![base_ref.to_string()]
    } else {
        vec![base_ref.to_string(), format!("origin/{base_ref}")]
    };

    for candidate in candidates {
        if let Ok(resolved) = git_stdout(
            repo_path,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{candidate}^{{commit}}"),
            ],
        ) {
            let resolved = resolved.trim();
            if !resolved.is_empty() {
                return Ok(resolved.to_string());
            }
        }
    }

    Err(GitError::BaseRefNotFound(base_ref.to_string()))
}

pub fn create_worktree(
    repo_path: &Path,
    worktree_path: &Path,
    branch: &str,
    base_ref: &str,
) -> Result<WorktreeInfo, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("worktree")
        .arg("add")
        .arg("-b")
        .arg(branch)
        .arg(worktree_path)
        .arg(base_ref)
        .output()
        .map_err(|error| GitError::Command(error.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GitError::Command(stderr.trim().to_string()));
    }

    Ok(WorktreeInfo {
        repo_path: repo_path.to_path_buf(),
        worktree_path: worktree_path.to_path_buf(),
        branch: branch.to_string(),
    })
}

fn git_stdout(repo_path: &Path, args: &[&str]) -> Result<String, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .output()
        .map_err(|error| GitError::Command(error.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GitError::Command(stderr.trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    #[test]
    fn detects_git_repository() {
        let temp = tempfile::tempdir().unwrap();
        let status = Command::new("git")
            .arg("init")
            .arg(temp.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());

        assert!(is_git_repo(temp.path()));
        assert_eq!(
            canonicalize_repo(temp.path()).unwrap(),
            temp.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn rejects_non_git_repository() {
        let temp = tempfile::tempdir().unwrap();

        assert!(!is_git_repo(temp.path()));
        assert!(matches!(
            canonicalize_repo(temp.path()),
            Err(GitError::NotRepository(_))
        ));
    }

    #[test]
    fn resolves_existing_base_ref_to_commit() {
        let temp = tempfile::tempdir().unwrap();
        init_repo_with_commit(temp.path());

        let resolved = resolve_base_ref(temp.path(), "HEAD").unwrap();

        assert_eq!(resolved.len(), 40);
        assert!(resolved.chars().all(|value| value.is_ascii_hexdigit()));
    }

    #[test]
    fn missing_base_ref_fails_clearly() {
        let temp = tempfile::tempdir().unwrap();
        init_repo_with_commit(temp.path());

        assert!(matches!(
            resolve_base_ref(temp.path(), "missing-branch"),
            Err(GitError::BaseRefNotFound(value)) if value == "missing-branch"
        ));
    }

    #[test]
    fn fetch_repo_skips_repositories_without_remotes() {
        let temp = tempfile::tempdir().unwrap();
        init_repo_with_commit(temp.path());

        fetch_repo(temp.path()).unwrap();
    }

    fn init_repo_with_commit(path: &Path) {
        let status = Command::new("git")
            .arg("init")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        fs::write(path.join("README.md"), "test\n").unwrap();
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("add")
            .arg("README.md")
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("-c")
            .arg("user.name=Scheduler Test")
            .arg("-c")
            .arg("user.email=scheduler@example.invalid")
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
}
