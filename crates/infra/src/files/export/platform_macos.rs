//! macOS publication through a held descriptor chain, a private temporary file and a no-replace
//! `renameatx_np`.
//!
//! macOS has no `O_TMPFILE`, so the temporary name is briefly visible in the held parent, as on
//! Windows. Every system call goes through the rustix safe API (`openat`, `fstat`, `statat`,
//! `unlinkat`, `renameat_with` with `RENAME_EXCL`, `fcntl_fullfsync`, `geteuid`), so this module
//! contains no `unsafe` code and needs no allow attribute. The shared orchestration (destination
//! syntax, protected roots, outcome mapping) is in the parent `export` module.
//!
//! `RENAME_EXCL` fails with `EEXIST` when the name exists, and the comparison follows the volume's
//! case rules, so on a case-insensitive APFS volume a name that differs only by case is refused
//! as well (`<sys/stdio.h>`, `RENAME_EXCL`, in the macOS SDK).

use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::Path;

use jobniac_core::artifacts::{DocumentArtifact, validate_artifact};
use rustix::fs::{
    AtFlags, FileType, Mode, OFlags, RenameFlags, Stat, fcntl_fullfsync, fstat, fsync, open,
    openat, renameat_with, statat, unlinkat,
};
use rustix::process::geteuid;
use uuid::Uuid;

use super::{AtomicExporter, Failure, TEMP_PREFIX, inside_protected, protected_snapshot};
use crate::files::no_follow::is_fork_component;
use crate::platform_macos_acl::has_extended_acl;

/// The held directory chain: every component is an open descriptor, and `names` gives each
/// component below the root in order.
struct Held {
    files: Vec<File>,
    names: Vec<String>,
}

/// The flags of every directory open: a directory, never a followed final link, close-on-exec.
fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

/// Publish `artifact` as `name` under `parent`.
pub(super) fn publish(
    exporter: &AtomicExporter,
    artifact: &DocumentArtifact,
    parent: &str,
    name: &str,
    published: &mut bool,
) -> Result<String, Failure> {
    let parent_path = Path::new(parent);
    if is_fork_component(name) {
        return Err(Failure::Refused);
    }
    let snapshot = protected_snapshot(&exporter.protected_roots);
    let held = open_chain(parent_path).map_err(|_| Failure::Refused)?;
    check_held(&held)?;
    check_protected(parent_path, &held, exporter, &snapshot)?;
    let parent_dir = held.files.last().ok_or(Failure::Refused)?;
    // An existing name, a link and a case variant on a case-insensitive volume all answer here.
    if statat(parent_dir, name, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
        return Err(Failure::Refused);
    }
    let temporary = format!("{TEMP_PREFIX}{}", Uuid::new_v4().simple());
    let outcome = write_and_link(
        &held,
        &temporary,
        name,
        artifact,
        &|| check_protected(parent_path, &held, exporter, &snapshot),
        published,
    );
    outcome.map(|()| name.to_owned())
}

/// Stages the temporary file and links it in as `name`; on any failure before the rename the
/// temporary name is removed again. `published` is set as soon as the rename succeeded.
fn write_and_link(
    held: &Held,
    temporary: &str,
    name: &str,
    artifact: &DocumentArtifact,
    recheck_protected: &dyn Fn() -> Result<(), Failure>,
    published: &mut bool,
) -> Result<(), Failure> {
    let outcome = stage_and_link(
        held,
        temporary,
        name,
        artifact,
        recheck_protected,
        published,
    );
    if !*published && let Some(parent_dir) = held.files.last() {
        let _ = unlinkat(parent_dir, temporary, AtFlags::empty());
    }
    outcome
}

/// Creates the temporary file, fills and syncs it, re-checks the chain and links it in as `name`
/// with a no-replace rename.
fn stage_and_link(
    held: &Held,
    temporary: &str,
    name: &str,
    artifact: &DocumentArtifact,
    recheck_protected: &dyn Fn() -> Result<(), Failure>,
    published: &mut bool,
) -> Result<(), Failure> {
    let parent_dir = held.files.last().ok_or(Failure::Refused)?;
    let created = openat(
        parent_dir,
        temporary,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|_| Failure::Refused)?;
    let mut file = File::from(created);
    // The creation mode is filtered by the umask, which can only remove bits; set it exactly.
    rustix::fs::fchmod(&file, Mode::from_raw_mode(0o600)).map_err(|_| Failure::Refused)?;
    file.write_all(&artifact.content)
        .map_err(|_| Failure::Refused)?;
    flush(&file).map_err(|_| Failure::Refused)?;
    validate_artifact(artifact.clone()).map_err(|_| Failure::Refused)?;
    check_held(held)?;
    recheck_protected()?;
    let inode = fstat(&file).map_err(|_| Failure::Refused)?;
    let by_name =
        statat(parent_dir, temporary, AtFlags::SYMLINK_NOFOLLOW).map_err(|_| Failure::Refused)?;
    if !is_exact(&inode, artifact.content.len())
        || inode.st_nlink != 1
        || !same_object(&inode, &by_name)
    {
        return Err(Failure::Refused);
    }
    // `EEXIST` is a refusal, and there is no fallback.
    renameat_with(
        parent_dir,
        temporary,
        parent_dir,
        name,
        RenameFlags::NOREPLACE,
    )
    .map_err(|_| Failure::Refused)?;
    *published = true;
    let current =
        statat(parent_dir, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|_| Failure::Published)?;
    if !same_object(&current, &inode)
        || current.st_nlink != 1
        || !is_exact(&current, artifact.content.len())
    {
        return Err(Failure::Published);
    }
    flush(parent_dir).map_err(|_| Failure::Published)?;
    Ok(())
}

/// Flushes to stable storage: `F_FULLFSYNC` asks the drive to empty its cache, which `fsync` on
/// macOS does not. A volume that does not support it falls back to `fsync`.
fn flush(file: &File) -> std::io::Result<()> {
    if fcntl_fullfsync(file).is_ok() {
        return Ok(());
    }
    fsync(file).map_err(std::io::Error::from)
}

/// A regular, owner-only file of the effective user with exactly `length` bytes.
fn is_exact(stat: &Stat, length: usize) -> bool {
    FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
        && stat.st_uid == geteuid().as_raw()
        && u32::from(stat.st_mode) & 0o7777 == 0o600
        && u64::try_from(stat.st_size).is_ok_and(|size| size == length as u64)
}

fn same_object(left: &Stat, right: &Stat) -> bool {
    (left.st_dev, left.st_ino) == (right.st_dev, right.st_ino)
}

fn is_directory(stat: &Stat) -> bool {
    FileType::from_raw_mode(stat.st_mode) == FileType::Directory
}

/// Opens every component of an absolute path, each with no-follow and directory flags.
fn open_chain(path: &Path) -> std::io::Result<Held> {
    let mut files = vec![File::from(open("/", directory_flags(), Mode::empty())?)];
    let mut names = Vec::new();
    for component in path.components().skip(1) {
        let text = component.as_os_str().to_string_lossy().into_owned();
        if is_fork_component(&text) {
            return Err(std::io::Error::other("fork component"));
        }
        let parent = files
            .last()
            .ok_or_else(|| std::io::Error::other("empty directory chain"))?;
        let opened = openat(parent, text.as_str(), directory_flags(), Mode::empty())?;
        files.push(File::from(opened));
        names.push(text);
    }
    Ok(Held { files, names })
}

/// Every held component still matches its name, and the parent is private. Descriptors do not
/// stop a rename on macOS, so each name is compared with its held directory.
fn check_held(held: &Held) -> Result<(), Failure> {
    let root =
        statat(rustix::fs::CWD, "/", AtFlags::SYMLINK_NOFOLLOW).map_err(|_| Failure::Refused)?;
    let root_held = fstat(&held.files[0]).map_err(|_| Failure::Refused)?;
    if !same_object(&root, &root_held) {
        return Err(Failure::Refused);
    }
    for (index, name) in held.names.iter().enumerate() {
        let current = statat(&held.files[index], name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| Failure::Refused)?;
        let held_file = fstat(&held.files[index + 1]).map_err(|_| Failure::Refused)?;
        if !is_directory(&current) || !same_object(&current, &held_file) {
            return Err(Failure::Refused);
        }
    }
    private_parent(held.files.last().ok_or(Failure::Refused)?)
}

/// A directory owned by the effective user with no group or other write bits and no extended ACL.
/// The ACL rule is stricter than Linux, which ignores POSIX ACLs: an `add_file` or `delete_child`
/// entry for another user would defeat the protection of the visible temporary name.
fn private_parent(directory: &File) -> Result<(), Failure> {
    let info = fstat(directory).map_err(|_| Failure::Refused)?;
    if !is_directory(&info)
        || info.st_uid != geteuid().as_raw()
        || u32::from(info.st_mode) & 0o022 != 0
    {
        return Err(Failure::Refused);
    }
    match has_extended_acl(directory.as_raw_fd()) {
        Ok(false) => Ok(()),
        Ok(true) | Err(_) => Err(Failure::Refused),
    }
}

/// The protected-root rules: the snapshot must be stable, and neither the parent nor any held
/// ancestor may be a protected root.
fn check_protected(
    path: &Path,
    held: &Held,
    exporter: &AtomicExporter,
    snapshot: &[String],
) -> Result<(), Failure> {
    if protected_snapshot(&exporter.protected_roots) != snapshot {
        return Err(Failure::Refused);
    }
    if inside_protected(path, &exporter.protected_roots, snapshot) {
        return Err(Failure::Refused);
    }
    let held_ids: Vec<_> = held
        .files
        .iter()
        .filter_map(|file| fstat(file).ok())
        .map(|info| (info.st_dev, info.st_ino))
        .collect();
    for root in &exporter.protected_roots {
        let identity = statat(rustix::fs::CWD, root.as_str(), AtFlags::empty())
            .ok()
            .map(|info| (info.st_dev, info.st_ino));
        if identity.is_some_and(|id| held_ids.contains(&id)) {
            return Err(Failure::Refused);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobniac_core::clock::format_instant;
    use jobniac_core::documents::{DocumentMetadata, sha256_hex};
    use time::macros::datetime;

    fn artifact() -> DocumentArtifact {
        let mut content = b"%PDF-1.4\n".to_vec();
        content.resize(64, b'a');
        DocumentArtifact {
            document: DocumentMetadata {
                id: Uuid::from_u128(0xAA),
                original_file_name: "cv.pdf".to_owned(),
                media_type: "application/pdf".to_owned(),
                byte_size: 64,
                sha256: sha256_hex(&content),
                created_at: format_instant(datetime!(2026-01-02 03:04:05 UTC)),
                updated_at: format_instant(datetime!(2026-01-02 03:04:06 UTC)),
            },
            content,
        }
    }

    /// A rename that fails after the temporary file was written (the name appeared in between)
    /// removes the temporary name and leaves the other file untouched.
    #[test]
    fn a_failed_rename_leaves_no_temporary_name() {
        let scratch = tempfile::tempdir().expect("scratch");
        let dir = std::fs::canonicalize(scratch.path()).expect("canonical scratch");
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .expect("private scratch");
        std::fs::write(dir.join("taken.pdf"), b"original").expect("existing file");
        let held = open_chain(&dir).expect("chain");
        let temporary = format!("{TEMP_PREFIX}test");
        let mut published = false;
        let outcome = write_and_link(
            &held,
            &temporary,
            "taken.pdf",
            &artifact(),
            &|| Ok(()),
            &mut published,
        );
        assert!(matches!(outcome, Err(Failure::Refused)));
        assert!(!published);
        let names: Vec<_> = std::fs::read_dir(&dir)
            .expect("list")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names, ["taken.pdf"]);
        assert_eq!(
            std::fs::read(dir.join("taken.pdf")).expect("read"),
            b"original"
        );
    }
}
