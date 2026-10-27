use std::{fs, io, path::{Path, PathBuf}};

pub(super) fn resolve_created(root: &Path, session: &Path) -> io::Result<PathBuf> {
    if !fs::symlink_metadata(session)?.is_dir() {
        return Err(io::Error::other("Created session is not a real directory"));
    }
    let root = root.canonicalize()?;
    let resolved = session.canonicalize()?;
    if resolved.parent() != Some(root.as_path()) || resolved.file_name() != session.file_name() {
        return Err(io::Error::other("Created session resolved outside its recordings directory"));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path=std::env::temp_dir().join(format!("focusflow-session-path-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();Self(path)
        }
    }
    impl Drop for Fixture {fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
    #[test]
    fn created_session_is_canonical_and_retains_its_name() {
        let f=Fixture::new();let dir=f.0.join("20261028_010203_001");fs::create_dir(&dir).unwrap();
        let result=resolve_created(&f.0,&dir).unwrap();assert_eq!(result,dir.canonicalize().unwrap());
        assert_eq!(result.parent().unwrap(),f.0.canonicalize().unwrap());assert_eq!(result.file_name(),dir.file_name());
    }
    #[test]
    fn missing_nonregular_and_outside_children_fail_without_changing_files() {
        let f=Fixture::new();let other=Fixture::new();let file=f.0.join("file");fs::write(&file,b"retained").unwrap();
        assert!(resolve_created(&f.0,&f.0.join("missing")).is_err());assert!(resolve_created(&f.0,&file).is_err());
        let outside=other.0.join("session");fs::create_dir(&outside).unwrap();assert!(resolve_created(&f.0,&outside).is_err());
        assert_eq!(fs::read(file).unwrap(),b"retained");assert!(outside.is_dir());
    }
    #[cfg(unix)]
    #[test]
    fn aliased_root_resolves_but_symlink_session_is_rejected() {
        let f=Fixture::new();let dir=f.0.join("session");fs::create_dir(&dir).unwrap();
        let alias=f.0.join("root-alias");std::os::unix::fs::symlink(&f.0,&alias).unwrap();
        assert_eq!(resolve_created(&alias,&alias.join("session")).unwrap(),dir.canonicalize().unwrap());
        let link=f.0.join("child-link");std::os::unix::fs::symlink(&dir,&link).unwrap();
        assert!(resolve_created(&f.0,&link).is_err());assert!(dir.is_dir());
    }
}
