//! Resolution of the Jinja/Typst template root.
//!
//! A print format may use `{% include %}`/`{% extends %}`. Those names are
//! resolved inside a **host-owned** root: the user can choose a relative
//! subfolder from the `Print Format` row, but never an absolute path, `..`, or
//! a symlink that leaves the root. The same resolved directory is what Typst
//! uses for `read()`, so a Jinja include and a Typst asset live under one
//! boundary.
//!
//! The root comes from `PrintConfig::template_dir` (the host sets it from
//! `FERRIS_PRINT_TEMPLATE_DIR`); `PrintFormatDef::template_dir` is the
//! per-format relative folder.

use std::path::{Component, Path, PathBuf};

use ferris_core::{Error, Result, ValidationError};

use super::super::config::{PrintConfig, PrintFormatDef};

/// Resolve the directory a format is allowed to read from.
///
/// `None` means the host configured no template root; a format that asks for
/// one (or uses an include, which `MiniJinja` will fail on) is then a validation
/// error rather than a silent fallback to the process working directory.
pub fn resolve(config: &PrintConfig, format: &PrintFormatDef) -> Result<Option<PathBuf>> {
    let relative = format
        .template_dir
        .as_deref()
        .map(str::trim)
        .filter(|dir| !dir.is_empty());

    let Some(root) = config
        .template_dir
        .as_deref()
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
    else {
        return relative.map_or_else(
            || Ok(None),
            |dir| {
                Err(invalid(format!(
                    "print format `{}` uses template folder `{dir}`, but the print engine has no template root; set FERRIS_PRINT_TEMPLATE_DIR",
                    format.name
                )))
            },
        );
    };

    let base = std::fs::canonicalize(root).map_err(|error| {
        invalid(format!(
            "print template root `{root}` is not readable: {error}"
        ))
    })?;
    if !base.is_dir() {
        return Err(invalid(format!(
            "print template root `{root}` is not a directory"
        )));
    }

    let Some(relative) = relative else {
        return Ok(Some(base));
    };
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(invalid(format!(
            "print format `{}` has an unsafe template folder `{relative}`; use a relative path without `..`",
            format.name
        )));
    }

    let candidate = base.join(relative_path);
    let resolved = candidate.canonicalize().map_err(|error| {
        invalid(format!(
            "print template folder `{relative}` does not exist under `{root}`: {error}"
        ))
    })?;
    if !resolved.starts_with(&base) || !resolved.is_dir() {
        return Err(invalid(format!(
            "print template folder `{relative}` escapes the print template root"
        )));
    }
    Ok(Some(resolved))
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Validation(ValidationError::Invalid(message.into()))
}
