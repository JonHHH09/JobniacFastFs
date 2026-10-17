//! Linux publication through a held descriptor chain and an anonymous `O_TMPFILE` inode.
//!
//! Every system call goes through the rustix safe API (`open`, `openat`, `linkat`, `geteuid`), so
//! this module contains no `unsafe` code and needs no allow attribute. The shared orchestration
//! (destination syntax, protected roots, outcome mapping) is in the parent `export` module.

use std::fs::{self, File, Permissions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use jobniac_core::artifacts::{DocumentArtifact, validate_artifact};
use rustix::fs::{AtFlags, CWD, Mode, OFlags, linkat, open, openat};
use rustix::process::geteuid;

use super::{AtomicExporter, Failure, inside_protected, protected_snapshot};

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
    let snapshot = protected_snapshot(&exporter.protected_roots);
    let held = open_chain(parent_path).map_err(|_| Failure::Refused)?;
    check_held(&held)?;
    check_protected(parent_path, &held, exporter, &snapshot)?;
    let parent_fd = held_fd(&held);
    let parent_dir = held.files.last().ok_or(Failure::Refused)?;
    let target = format!("/proc/self/fd/{parent_fd}/{name}");
    if fs::symlink_metadata(&target).is_ok() {
        return Err(Failure::Refused);
    }
    let anonymous = File::from(
        open(
            format!("/proc/self/fd/{parent_fd}").as_str(),
            OFlags::RDWR | OFlags::TMPFILE | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| Failure::Refused)?,
    );
    anonymous
        .set_permissions(Permissions::from_mode(0o600))
        .map_err(|_| Failure::Refused)?;
    let mut writer = &anonymous;
    writer
        .write_all(&artifact.content)
        .map_err(|_| Failure::Refused)?;
    anonymous.sync_all().map_err(|_| Failure::Refused)?;
    validate_artifact(artifact.clone()).map_err(|_| Failure::Refused)?;
    check_held(&held)?;
    check_protected(parent_path, &held, exporter, &snapshot)?;
    let inode = anonymous.metadata().map_err(|_| Failure::Refused)?;
    let by_proc = fs::metadata(format!("/proc/self/fd/{}", anonymous.as_raw_fd()))
        .map_err(|_| Failure::Refused)?;
    if !inode.is_file()
        || inode.nlink() != 0
        || inode.uid() != geteuid().as_raw()
        || inode.mode() & 0o7777 != 0o600
        || inode.size() != artifact.content.len() as u64
        || (inode.dev(), inode.ino()) != (by_proc.dev(), by_proc.ino())
    {
        return Err(Failure::Refused);
    }
    link_anonymous(&anonymous, name, parent_dir)?;
    *published = true;
    let current = fs::symlink_metadata(format!("/proc/self/fd/{parent_fd}/{name}"))
        .map_err(|_| Failure::Published)?;
    if (current.dev(), current.ino()) != (inode.dev(), inode.ino())
        || current.nlink() != 1
        || current.size() != artifact.content.len() as u64
    {
        return Err(Failure::Published);
    }
    parent_dir.sync_all().map_err(|_| Failure::Published)?;
    Ok(name.to_owned())
}

/// Opens every component of an absolute path, each with no-follow and directory flags.
fn open_chain(path: &Path) -> std::io::Result<Held> {
    let mut files = vec![File::from(open("/", directory_flags(), Mode::empty())?)];
    let mut names = Vec::new();
    for component in path.components().skip(1) {
        let text = component.as_os_str().to_string_lossy().into_owned();
        let parent = files
            .last()
            .ok_or_else(|| std::io::Error::other("empty directory chain"))?;
        let opened = openat(parent, text.as_str(), directory_flags(), Mode::empty())?;
        files.push(File::from(opened));
        names.push(text);
    }
    Ok(Held { files, names })
}

fn held_fd(held: &Held) -> i32 {
    held.files.last().map_or(-1, |file| file.as_raw_fd())
}

/// Every held component still matches its name, and the parent is private (E-FS-114, E-FS-116).
fn check_held(held: &Held) -> Result<(), Failure> {
    let root = fs::symlink_metadata("/").map_err(|_| Failure::Refused)?;
    let root_held = held.files[0].metadata().map_err(|_| Failure::Refused)?;
    if (root.dev(), root.ino()) != (root_held.dev(), root_held.ino()) {
        return Err(Failure::Refused);
    }
    let mut parent_fd = held.files[0].as_raw_fd();
    for (index, name) in held.names.iter().enumerate() {
        let current = fs::symlink_metadata(format!("/proc/self/fd/{parent_fd}/{name}"))
            .map_err(|_| Failure::Refused)?;
        let held_file = held.files[index + 1]
            .metadata()
            .map_err(|_| Failure::Refused)?;
        if !current.is_dir() || (current.dev(), current.ino()) != (held_file.dev(), held_file.ino())
        {
            return Err(Failure::Refused);
        }
        parent_fd = held.files[index + 1].as_raw_fd();
    }
    private_parent(held.files.last().ok_or(Failure::Refused)?)
}

/// A directory owned by the effective user with no group or other write bits (E-FS-114).
fn private_parent(directory: &File) -> Result<(), Failure> {
    let info = directory.metadata().map_err(|_| Failure::Refused)?;
    if !info.is_dir() || info.uid() != geteuid().as_raw() || info.mode() & 0o022 != 0 {
        return Err(Failure::Refused);
    }
    Ok(())
}

/// The protected-root rules (E-FS-115): the snapshot must be stable, and neither the parent nor
/// any held ancestor may be a protected root.
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
    let held_ids: Vec<(u64, u64)> = held
        .files
        .iter()
        .filter_map(|file| file.metadata().ok())
        .map(|info| (info.dev(), info.ino()))
        .collect();
    for root in &exporter.protected_roots {
        let identity = fs::metadata(root).ok().map(|info| (info.dev(), info.ino()));
        if identity.is_some_and(|id| held_ids.contains(&id)) {
            return Err(Failure::Refused);
        }
    }
    Ok(())
}

/// Links the anonymous inode into the held directory. `EEXIST` is a refusal, and there is no
/// fallback (E-FS-113). `AT_SYMLINK_FOLLOW` makes the kernel follow only the proc link of this
/// process's own anonymous descriptor; the target is resolved inside the held parent.
fn link_anonymous(anonymous: &File, name: &str, parent: &File) -> Result<(), Failure> {
    let source = format!("/proc/self/fd/{}", anonymous.as_raw_fd());
    linkat(CWD, source.as_str(), parent, name, AtFlags::SYMLINK_FOLLOW)
        .map_err(|_| Failure::Refused)
}

// Follow-up (see the parent module): this chain and the geteuid checks duplicate
// `crate::generated::platform::linux`; merge them into one platform layer.
