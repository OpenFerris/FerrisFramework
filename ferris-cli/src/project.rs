//! Filesystem helpers shared by the commands that work inside a project.
//!
//! These used to be a private copy in each of six commands, and the copies had
//! drifted: some reported the error without the directory, one checked
//! `config/ferris.toml` as well. `--dir` also went unread: every one of them
//! resolved against the process working directory, which `ferris --dir <elsewhere>
//! build` left at `<elsewhere>`'s caller.

use anyhow::{Context, Result};
use std::path::Path;

/// Fail unless `dir` looks like a Ferris project.
///
/// Run before a command touches the tree, so that a bad `--dir` is an error and
/// not a build that half-succeeds in the wrong place.
pub fn check_project_exists(dir: &Path) -> Result<()> {
    if !dir.join("Cargo.toml").exists() {
        return Err(anyhow::anyhow!(
            "{} is not a Ferris project (no Cargo.toml). Run 'ferris new' first.",
            dir.display()
        ));
    }
    Ok(())
}

/// The directory a file lives in, for creating it.
///
/// `Path::parent` is `None` only for an empty path, and the callers here always
/// join a file name first — so the old `.unwrap()` was safe, and a panic if that
/// ever stopped being true would take `ferris generate` down with it. An empty
/// parent is the current directory, which is what the caller meant anyway.
pub fn parent_dir(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| Path::new("."))
}

/// Copy `src` into `dst`, creating directories as needed.
///
/// A symlink is copied as a symlink rather than followed: following one that
/// points at an ancestor walks the tree forever.
pub fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("could not create {}", dst.display()))?;

    for entry in
        std::fs::read_dir(src).with_context(|| format!("could not read {}", src.display()))?
    {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());

        if entry.file_type()?.is_symlink() {
            let target = std::fs::read_link(&from)?;
            std::os::unix::fs::symlink(&target, &to)
                .with_context(|| format!("could not link {}", to.display()))?;
        } else if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).with_context(|| {
                format!("could not copy {} to {}", from.display(), to.display())
            })?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique scratch directory per test, since `cargo test` runs them in threads.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ferris-project-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_directory_without_a_manifest_is_not_a_project() {
        let dir = scratch("not-a-project");
        let err = check_project_exists(&dir).expect_err("no Cargo.toml in there");
        assert!(err.to_string().contains("is not a Ferris project"), "{err}");
        assert!(
            err.to_string().contains(&dir.display().to_string()),
            "the message must name the directory it looked at: {err}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_with_a_manifest_is_a_project() {
        let dir = scratch("a-project");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").unwrap();
        check_project_exists(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copying_brings_files_and_nested_directories() {
        let root = scratch("copy");
        let src = root.join("src");
        std::fs::create_dir_all(src.join("templates/email")).unwrap();
        std::fs::write(src.join("ferris.toml"), "[server]\n").unwrap();
        std::fs::write(src.join("templates/email/welcome.html"), "<p>hi</p>").unwrap();

        let dst = root.join("dst");
        copy_dir_all(&src, &dst).unwrap();

        assert_eq!(
            std::fs::read_to_string(dst.join("ferris.toml")).unwrap(),
            "[server]\n"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("templates/email/welcome.html")).unwrap(),
            "<p>hi</p>"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_copied_as_a_symlink_not_followed() {
        let root = scratch("symlink");
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        // A link into the tree it is copied from: following it would not end.
        std::fs::write(root.join("outside.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(root.join("outside.txt"), src.join("back.txt")).unwrap();

        let dst = root.join("dst");
        copy_dir_all(&src, &dst).unwrap();

        assert!(
            std::fs::symlink_metadata(dst.join("back.txt"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the copy is a link, not the file it points at"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
