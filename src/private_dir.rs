//! Private working directories under a shared parent such as `$TMPDIR`.
//!
//! `create_dir_all` succeeds when the directory already exists, whoever made
//! it. On a stock Mac `$TMPDIR` is a per-user 0700 directory, so that is
//! harmless there, but a launch setup that points `TMPDIR` at a shared place
//! such as `/tmp` would let another user pre-create the directory and read or
//! plant the files macrdp writes into it. These helpers only hand back a
//! directory that this process created, or one that is provably ours: a real
//! directory (not a symlink), owned by our user, with no group or other access.

use std::io;
use std::path::{Path, PathBuf};

/// Create a fresh directory `parent/<prefix><random>` with mode 0700. Fails
/// rather than reusing anything that already exists.
pub fn create_unique(parent: &Path, prefix: &str) -> io::Result<PathBuf> {
    for _ in 0..8 {
        let mut bytes = [0u8; 8];
        getrandom::getrandom(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
        let suffix: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let path = parent.join(format!("{prefix}{suffix}"));
        match create_0700(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not find a free directory name",
    ))
}

/// Create `path` with mode 0700, or accept it if it already exists and is
/// ours (see the module docs). Used for a per-process directory that several
/// callers share.
pub fn ensure(path: &Path) -> io::Result<()> {
    match create_0700(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => verify_ours(path),
        Err(e) => Err(e),
    }
}

/// Create a new directory `child` directly inside `parent`, refusing a name
/// that would leave `parent` (`..`, a separator) and anything that already
/// exists.
pub fn create_child(parent: &Path, child: &str) -> io::Result<PathBuf> {
    if child.is_empty() || child == "." || child == ".." || child.contains(['/', '\\']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a plain directory name: {child:?}"),
        ));
    }
    let path = parent.join(child);
    if path.parent() != Some(parent) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path escapes its parent",
        ));
    }
    create_0700(&path)?;
    Ok(path)
}

#[cfg(unix)]
fn create_0700(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_0700(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

#[cfg(unix)]
fn verify_ours(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    let ok = meta.file_type().is_dir() && meta.uid() == euid && meta.mode() & 0o077 == 0;
    if ok {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} exists but is not a private directory owned by this user",
                path.display()
            ),
        ))
    }
}

#[cfg(not(unix))]
fn verify_ours(path: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(path)?.file_type().is_dir() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "not a directory",
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn scratch() -> PathBuf {
        create_unique(&std::env::temp_dir(), "macrdp-private-dir-test-").unwrap()
    }

    #[test]
    fn unique_dirs_are_new_and_private() {
        let root = scratch();
        let a = create_unique(&root, "x-").unwrap();
        let b = create_unique(&root, "x-").unwrap();
        assert_ne!(a, b);
        let mode = std::fs::metadata(&a).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ensure_accepts_our_own_and_rejects_open_or_symlinked_dirs() {
        let root = scratch();
        let mine = root.join("mine");
        ensure(&mine).unwrap();
        ensure(&mine).unwrap();

        let open = root.join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            ensure(&open).is_err(),
            "a world-writable dir must not be reused"
        );

        let link = root.join("link");
        std::os::unix::fs::symlink(&mine, &link).unwrap();
        assert!(ensure(&link).is_err(), "a symlink must not be followed");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn create_child_refuses_names_that_leave_the_parent() {
        let root = scratch();
        for bad in ["", ".", "..", "a/b", "..\\x"] {
            assert!(create_child(&root, bad).is_err(), "{bad:?}");
        }
        let ok = create_child(&root, "drive").unwrap();
        assert_eq!(ok, root.join("drive"));
        assert!(
            create_child(&root, "drive").is_err(),
            "existing dir must not be reused"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
