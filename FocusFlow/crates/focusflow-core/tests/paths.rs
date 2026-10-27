mod common;

use common::TestDir;
use focusflow_core::{paths::{asset_path, project_dir}, ErrorCode};
use std::{fs, path::Path};

// Catches missing ID syntax checks, root aliases and non-direct-child resolution.
#[test]
fn rejects_root_aliases_and_escapes() {
    let temp = TestDir::new();
    let root = temp.path().join("Recordings");
    fs::create_dir(&root).unwrap();
    let direct = root.join("session_01");
    fs::create_dir(&direct).unwrap();
    assert_eq!(project_dir(&root, "session_01").unwrap(), direct.canonicalize().unwrap());
    for id in [".", "..", "", "a/b", "a\\b", "/outside", "C:\\x", "has space", "-prefix", "ക"] {
        assert_eq!(project_dir(&root, id).unwrap_err().code, ErrorCode::InvalidId, "{id}");
    }
    assert_eq!(project_dir(&root, &"a".repeat(129)).unwrap_err().code, ErrorCode::InvalidId);
    fs::create_dir(root.join("a".repeat(128))).unwrap();
    assert!(project_dir(&root, &"a".repeat(128)).is_ok());
    fs::write(root.join("file"), b"synthetic").unwrap();
    assert_eq!(project_dir(&root, "file").unwrap_err().code, ErrorCode::InvalidPath);
    assert_eq!(project_dir(&root, "missing").unwrap_err().code, ErrorCode::NotFound);
    assert_eq!(project_dir(&root.join("file"), "session_01").unwrap_err().code, ErrorCode::InvalidPath);
}

// Catches over-restrictive ASCII-only assets and missing relative/file checks.
#[test]
fn accepts_unicode_space_asset_names() {
    let temp = TestDir::new();
    let media = temp.path().join("media");
    fs::create_dir(&media).unwrap();
    let expected = media.join("hello world-കേരളം.mp4");
    fs::write(&expected, b"synthetic video").unwrap();
    assert_eq!(asset_path(temp.path(), "media/hello world-കേരളം.mp4").unwrap(), expected.canonicalize().unwrap());
    for relative in ["../x", "/x", r"C:\x", r"\\host\share", r"C:x", r"media\file.mp4", "", ".", "media/../x", "media/./x", "media//x", "media/file:stream", "media\0file"] {
        assert_eq!(asset_path(temp.path(), relative).unwrap_err().code, ErrorCode::InvalidPath, "{relative:?}");
    }
    assert_eq!(asset_path(temp.path(), "media").unwrap_err().code, ErrorCode::InvalidPath);
    assert_eq!(asset_path(temp.path(), "missing.mp4").unwrap_err().code, ErrorCode::NotFound);
    assert_eq!(asset_path(&expected, "x").unwrap_err().code, ErrorCode::InvalidPath);
}

#[cfg(any(unix, windows))]
fn symlink_dir(target: &Path, link: &Path) {
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_dir(target, link);
    result.expect("Directory symlink coverage requires symlink privileges; test was not skipped");
}

// Catches canonical containment bypasses through root/outside/nested symlinks.
#[cfg(any(unix, windows))]
#[test]
fn rejects_project_and_asset_symlink_escapes() {
    let temp = TestDir::new();
    let root = temp.path().join("Recordings");
    let project = root.join("session_01");
    let nested = project.join("media");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("clip.mp4"), b"synthetic outside").unwrap();
    symlink_dir(&root, &root.join("root_alias"));
    symlink_dir(&outside, &root.join("outside_alias"));
    symlink_dir(&nested, &root.join("nested_alias"));
    for id in ["root_alias", "outside_alias", "nested_alias"] {
        assert_eq!(project_dir(&root, id).unwrap_err().code, ErrorCode::InvalidPath, "{id}");
    }
    symlink_dir(&outside, &nested.join("escape"));
    assert_eq!(asset_path(&project, "media/escape/clip.mp4").unwrap_err().code, ErrorCode::InvalidPath);
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.join("clip.mp4"), project.join("escape.mp4")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(outside.join("clip.mp4"), project.join("escape.mp4"))
        .expect("File symlink coverage requires symlink privileges; test was not skipped");
    assert_eq!(asset_path(&project, "escape.mp4").unwrap_err().code, ErrorCode::InvalidPath);
    fs::write(nested.join("inside.mp4"), b"synthetic inside").unwrap();
    symlink_dir(&nested, &project.join("inside_alias"));
    assert_eq!(asset_path(&project, "inside_alias/inside.mp4").unwrap(), nested.join("inside.mp4").canonicalize().unwrap());
}

// Different ID-keyed locks must never refer to the same physical project.
#[cfg(any(unix, windows))]
#[test]
fn rejects_sibling_project_alias() {
    let temp = TestDir::new();
    std::fs::create_dir(temp.path().join("real")).unwrap();
    symlink_dir(&temp.path().join("real"), &temp.path().join("alias"));
    assert_eq!(project_dir(temp.path(), "alias").unwrap_err().code, ErrorCode::InvalidPath);
    assert!(project_dir(temp.path(), "real").is_ok());
}

// Discovery must reject an aliased exports parent before enumerating its children.
#[cfg(any(unix, windows))]
#[test]
fn exports_discovery_requires_real_parent_and_job_directories() {
    use focusflow_core::paths::{exports_dir, job_dir};
    let temp = TestDir::new();
    let project = temp.path().join("project");
    let outside = temp.path().join("outside");
    fs::create_dir(&project).unwrap();
    fs::create_dir(&outside).unwrap();
    assert_eq!(exports_dir(&project).unwrap_err().code, ErrorCode::NotFound);
    symlink_dir(&outside, &project.join("exports"));
    assert_eq!(exports_dir(&project).unwrap_err().code, ErrorCode::InvalidPath);
    fs::remove_dir(project.join("exports")).or_else(|_| fs::remove_file(project.join("exports"))).unwrap();
    fs::create_dir(project.join("exports")).unwrap();
    assert_eq!(exports_dir(&project).unwrap(), project.join("exports").canonicalize().unwrap());
    symlink_dir(&outside, &project.join("exports/alias"));
    assert_eq!(job_dir(&project, "alias").unwrap_err().code, ErrorCode::InvalidPath);
    fs::create_dir(project.join("exports/job_1")).unwrap();
    assert!(job_dir(&project, "job_1").is_ok());
    assert_eq!(job_dir(&project, "../outside").unwrap_err().code, ErrorCode::InvalidId);
}
