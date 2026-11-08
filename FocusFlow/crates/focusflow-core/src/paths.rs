use std::path::{Path, PathBuf};

use crate::{CoreError, CoreResult, ErrorCode};

pub fn project_dir(root: &Path, id: &str) -> CoreResult<PathBuf> {
    validate_id(id, "projectId")?;

    let root = existing_dir(root, "projectId")?;
    let project = existing_dir(&root.join(id), "projectId")?;
    if project.parent() != Some(root.as_path()) || project.file_name() != Some(std::ffi::OsStr::new(id)) {
        return Err(path_error(
            ErrorCode::InvalidPath,
            "Project must be a direct child with the requested ID in the recordings directory",
            "projectId",
        ));
    }
    Ok(project)
}

pub fn asset_path(project_dir: &Path, relative: &str) -> CoreResult<PathBuf> {
    // Use forward slashes on every host; reject drive prefixes and Windows alternate streams.
    if relative.contains(['\\', ':', '\0'])
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(path_error(
            ErrorCode::InvalidPath,
            "Asset must be a relative path with normal components and forward slashes",
            "path",
        ));
    }

    let project = existing_dir(project_dir, "path")?;
    let asset = resolve_existing(&project.join(relative), "path")?;
    if !asset.starts_with(&project) || !asset.is_file() {
        return Err(path_error(
            ErrorCode::InvalidPath,
            "Asset must be a regular file inside its project directory",
            "path",
        ));
    }
    Ok(asset)
}

fn existing_dir(path: &Path, field: &str) -> CoreResult<PathBuf> {
    let canonical = resolve_existing(path, field)?;
    if !canonical.is_dir() {
        return Err(path_error(
            ErrorCode::InvalidPath,
            "Expected a directory",
            field,
        ));
    }
    Ok(canonical)
}

fn resolve_existing(path: &Path, field: &str) -> CoreResult<PathBuf> {
    path.canonicalize().map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::NotFound {
            ErrorCode::NotFound
        } else {
            ErrorCode::Io
        };
        path_error(
            code,
            format!("Could not resolve {}: {error}", path.display()),
            field,
        )
    })
}

fn path_error(code: ErrorCode, message: impl Into<String>, field: &str) -> CoreError {
    CoreError {
        code,
        message: message.into(),
        field: Some(field.to_string()),
    }
}

pub(crate) fn validate_id(id: &str, field: &str) -> CoreResult<()> {
    if id.is_empty()
        || id.len() > 128
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(path_error(
            ErrorCode::InvalidId,
            "ID must match [A-Za-z0-9][A-Za-z0-9_-]{0,127}",
            field,
        ));
    }
    Ok(())
}

/// Derived jobs have their own real directories; aliases must not merge identities or escape exports.
pub fn job_dir(project: &Path, job_id: &str) -> CoreResult<PathBuf> {
    validate_id(job_id, "jobId")?;
    let exports = exports_dir(project)?;
    real_child_directory(&exports, job_id, false)
}

/// Resolve the real exports child before discovering candidate job IDs.
pub fn exports_dir(project: &Path) -> CoreResult<PathBuf> {
    real_child_directory(project, "exports", false)
}

pub(crate) fn real_child_directory(parent: &Path, name: &str, create: bool) -> CoreResult<PathBuf> {
    let path = parent.join(name);
    if create {
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(path_error(ErrorCode::Io, format!("Could not create directory: {error}"), "path")),
        }
    }
    let canonical = existing_dir(&path, "path")?;
    if canonical != path || std::fs::symlink_metadata(&path).map_err(|error| path_error(ErrorCode::Io, error.to_string(), "path"))?.file_type().is_symlink() {
        return Err(path_error(ErrorCode::InvalidPath, "Export directories must be real directories inside their project", "path"));
    }
    Ok(canonical)
}

pub(crate) fn job_file(dir: &Path, name: &str) -> CoreResult<PathBuf> {
    let path = dir.join(name);
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| path_error(
        if error.kind() == std::io::ErrorKind::NotFound { ErrorCode::NotFound } else { ErrorCode::Io }, error.to_string(), "path"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || resolve_existing(&path, "path")? != path {
        return Err(path_error(ErrorCode::InvalidPath, "Job files must be regular files in their reserved directory", "path"));
    }
    Ok(path)
}

/// One portable filename only; callers never supply an output path or FFmpeg expression.
pub fn validate_export_basename(name: &str) -> CoreResult<()> {
    if name.eq_ignore_ascii_case("output.partial.mp4") {
        return Err(path_error(ErrorCode::InvalidPath, "output.partial.mp4 is reserved for the encoder temporary output", "basename"));
    }
    let stem = name.strip_suffix(".mp4").unwrap_or("");
    let device = stem.split('.').next().unwrap_or("").trim_end_matches([' ', '.']).to_uppercase();
    let reserved = matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$")
        || ["COM", "LPT"].iter().any(|prefix| device.strip_prefix(prefix).is_some_and(|n| matches!(n, "1"|"2"|"3"|"4"|"5"|"6"|"7"|"8"|"9"|"¹"|"²"|"³")));
    if name.len() > 128 || stem.is_empty() || stem.ends_with([' ', '.']) || stem == "." || stem == ".." || reserved
        || name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')) {
        return Err(path_error(ErrorCode::InvalidPath, "Output name must be one safe MP4 basename of at most 128 bytes, without paths or Windows device names", "basename"));
    }
    Ok(())
}
