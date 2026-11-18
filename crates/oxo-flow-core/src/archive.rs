//! Safe, complete extraction of remote tar archives (issue #1157, audit CL-04).
//!
//! Both archive consumers in the engine — `oxo-flow pull` bundle
//! verification and the web server's remote result pull — used to trust the
//! extractor:
//!
//! - [`tar::Archive::unpack`] silently **skips** members whose path contains
//!   `..`, silently **rewrites** absolute member paths relative to the
//!   destination, and **creates symlinks verbatim** even when their target
//!   points outside the destination. All of those report `Ok(())`, so a
//!   crafted archive is indistinguishable from a clean one.
//! - Shelling out to the platform `tar` inherits whatever the installed tar
//!   does; rejection is not proven and differs between bsdtar and GNU tar.
//! - Neither path verifies the archive afterwards, so a truncated transfer
//!   that stops on a member boundary leaves missing members behind while the
//!   caller claims success.
//!
//! [`extract_tar`] closes all three holes with a pure-Rust policy applied to
//! every member while it is extracted:
//!
//! 1. **Path pre-check** — an absolute member path, any `..` component, or an
//!    empty path fails extraction with the offending member named. Symlink
//!    and hard-link targets must resolve (lexically) inside the destination
//!    root; an escaping target is rejected before the link is created.
//! 2. **Extraction** — each entry goes through [`tar::Entry::unpack_in`],
//!    which re-checks the destination defensively; a refusal is an error, not
//!    a silent skip.
//! 3. **Post-extraction completeness** — every recorded member must exist on
//!    disk with the expected type, and callers can require named members
//!    (for example the remote run's `execution.log`) that must be present in
//!    the archive. A missing member fails with its path named.
//!
//! The helper returns the member list so callers can verify their own
//! expectations against exactly what the archive carried.

use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// What a validated archive member is expected to become on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveMemberKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link (target stored in [`ArchiveMember::link_target`]).
    Symlink,
    /// A hard link (target stored in [`ArchiveMember::link_target`]).
    HardLink,
}

/// One archive member that passed the member-level pre-check and was
/// extracted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveMember {
    /// Destination-relative path, normalized (no `.`/`..`/root components).
    pub path: PathBuf,
    /// What the member became on disk.
    pub kind: ArchiveMemberKind,
    /// Link target as recorded in the archive (symlink/hard-link members).
    pub link_target: Option<PathBuf>,
}

/// The members an archive contributed to the destination.
#[derive(Debug, Default)]
pub struct ExtractedArchive {
    members: Vec<ArchiveMember>,
}

impl ExtractedArchive {
    /// Every extracted member, in archive order.
    pub fn members(&self) -> &[ArchiveMember] {
        &self.members
    }

    /// Whether the archive carried a member at the given relative path.
    ///
    /// The query is normalized the same way member paths are (`./` and
    /// repeated separators are dropped), so a caller's `./data/x` matches the
    /// extracted `data/x`; an absolute or `..` query never matches.
    pub fn contains(&self, path: impl AsRef<Path>) -> bool {
        let Ok(wanted) = normalize_member_path(path.as_ref()) else {
            return false;
        };
        !wanted.as_os_str().is_empty() && self.members.iter().any(|m| m.path == wanted)
    }

    /// Number of extracted members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the archive contained no extractable members.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// Extract every member of a tar stream into `dest`, rejecting unsafe members
/// loudly and verifying completeness afterwards.
///
/// `required` lists destination-relative members the caller expects the
/// archive to carry (e.g. a remote run's `execution.log`). Every required
/// member missing from the archive fails with all missing paths named — a
/// partial archive can never look like a successful extraction.
pub fn extract_tar_requiring<R: Read>(
    reader: R,
    dest: &Path,
    required: &[&str],
) -> io::Result<ExtractedArchive> {
    std::fs::create_dir_all(dest).map_err(|e| {
        io::Error::other(format!(
            "failed to create extraction destination {}: {e}",
            dest.display()
        ))
    })?;

    let mut archive = tar::Archive::new(reader);
    // The remote result pull concatenates a second, caller-verified tar
    // segment (the run-specific completion record) after the result tree.
    // Reading past the first archive's zero terminator is what makes that
    // trailing record visible — and its absence proof of a truncated stream.
    archive.set_ignore_zeros(true);
    let entries = archive
        .entries()
        .map_err(|e| io::Error::other(format!("failed to read tar archive: {e}")))?;

    let mut members: Vec<ArchiveMember> = Vec::new();
    for entry in entries {
        let mut entry = entry
            .map_err(|e| io::Error::other(format!("failed to read tar archive entry: {e}")))?;
        let entry_type = entry.header().entry_type();

        // PAX/GNU extension headers carry metadata for the NEXT entry, not a
        // member of their own — the tar crate applies them transparently when
        // the real entry is read.
        if entry_type.is_pax_global_extensions()
            || entry_type.is_pax_local_extensions()
            || entry_type.is_gnu_longname()
            || entry_type.is_gnu_longlink()
        {
            continue;
        }

        let raw_path = entry
            .path()
            .map_err(|e| io::Error::other(format!("tar archive member has an invalid path: {e}")))?
            .to_path_buf();
        let path = normalize_member_path(&raw_path)?;
        if path.as_os_str().is_empty() {
            // `tar -cf - .` emits the archive root itself as `./` (both GNU
            // tar and bsdtar do): there is nothing to write — the
            // destination already exists. A non-directory member without a
            // name is meaningless and must not slip through.
            if entry_type.is_dir() {
                continue;
            }
            return Err(io::Error::other(
                "archive member has an empty path — refusing to extract",
            ));
        }

        let (kind, link_target) = if entry_type.is_dir() {
            (ArchiveMemberKind::Directory, None)
        } else if entry_type.is_file() || entry_type.is_contiguous() {
            (ArchiveMemberKind::File, None)
        } else if entry_type.is_symlink() {
            let target = link_target_of(&entry, &path, "symlink")?;
            ensure_link_stays_inside(&path, &target, "symlink")?;
            (ArchiveMemberKind::Symlink, Some(target))
        } else if entry_type.is_hard_link() {
            let target = link_target_of(&entry, &path, "hard link")?;
            ensure_link_stays_inside(&path, &target, "hard link")?;
            (ArchiveMemberKind::HardLink, Some(target))
        } else {
            // FIFOs, character/block devices, GNU sparse headers, or an
            // unknown typeflag: no reason for a bundle or a result tree to
            // carry them, and creating device nodes from an archive is a
            // privilege escalation primitive. Fail closed.
            return Err(member_error(
                &path,
                format!("unsupported member type '{}'", entry_type.as_byte() as char),
            ));
        };

        let unpacked = entry
            .unpack_in(dest)
            .map_err(|e| member_error(&path, format!("failed to extract: {}", error_chain(&e))))?;
        if !unpacked {
            // unpack_in only refuses paths that we already rejected; never
            // let a silent skip pass as success.
            return Err(member_error(
                &path,
                "refused by the tar extractor (it would escape the destination)",
            ));
        }

        members.push(ArchiveMember {
            path,
            kind,
            link_target,
        });
    }

    // Completeness: every member the archive listed must actually be on
    // disk. This catches extraction paths that silently dropped an entry
    // (the old behavior) and any write that failed without propagating.
    for member in &members {
        let full = dest.join(&member.path);
        let meta = std::fs::symlink_metadata(&full)
            .map_err(|e| member_error(&member.path, format!("missing after extraction: {e}")))?;
        let file_type = meta.file_type();
        let matches_kind = match member.kind {
            ArchiveMemberKind::File => file_type.is_file(),
            ArchiveMemberKind::Directory => file_type.is_dir(),
            ArchiveMemberKind::Symlink => file_type.is_symlink(),
            // A hard link lands as the linked file (or a link to it); only a
            // directory would prove the entry did not land as expected.
            ArchiveMemberKind::HardLink => !file_type.is_dir(),
        };
        if !matches_kind {
            return Err(member_error(
                &member.path,
                "has an unexpected type after extraction",
            ));
        }
    }

    // Caller expectations: the archive must carry the members the caller
    // needs. Report every missing path at once so one run surfaces the whole
    // gap instead of failing one member per retry.
    let mut missing: Vec<String> = Vec::new();
    for required in required {
        let expected = normalize_member_path(Path::new(required))
            .map_err(|e| io::Error::other(format!("invalid required member '{required}': {e}")))?;
        if expected.as_os_str().is_empty() {
            return Err(io::Error::other(format!(
                "invalid required member '{required}': empty path"
            )));
        }
        if !members.iter().any(|m| m.path == expected) {
            missing.push(required.to_string());
        }
    }
    if !missing.is_empty() {
        return Err(io::Error::other(format!(
            "archive is missing required member(s): {}",
            missing.join(", ")
        )));
    }

    Ok(ExtractedArchive { members })
}

/// Extract a tar stream with no caller-declared required members.
///
/// Every member is still validated and verified; see [`extract_tar_requiring`].
pub fn extract_tar<R: Read>(reader: R, dest: &Path) -> io::Result<ExtractedArchive> {
    extract_tar_requiring(reader, dest, &[])
}

/// Normalize an archive member path, rejecting anything that could leave the
/// destination root.
fn normalize_member_path(raw: &Path) -> io::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in raw.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(io::Error::other(format!(
                    "archive member '{}' is an absolute path — refusing to extract",
                    raw.display()
                )));
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(io::Error::other(format!(
                    "archive member '{}' contains '..' — refusing to extract",
                    raw.display()
                )));
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

/// Read and validate the link target of a symlink/hard-link entry.
fn link_target_of<R: Read>(
    entry: &tar::Entry<'_, R>,
    member: &Path,
    kind: &str,
) -> io::Result<PathBuf> {
    match entry.link_name() {
        Ok(Some(target)) => {
            if target.as_os_str().is_empty() {
                return Err(member_error(member, format!("{kind} has an empty target")));
            }
            Ok(target.to_path_buf())
        }
        Ok(None) => Err(member_error(member, format!("{kind} has no target"))),
        Err(e) => Err(member_error(member, format!("invalid {kind} target: {e}"))),
    }
}

/// Require a link target to be relative and free of `..` components.
///
/// Lexical containment is deliberately NOT accepted: a target component can
/// itself be a symlink, so `x -> .` followed by `y -> x/..` resolves (through
/// `x`) to the destination PARENT even though the `..` "cancels" against `x`
/// on paper. Any `..` in a symlink or hard-link target is therefore rejected;
/// in-tree targets without `..` (for example `sub -> data.txt` or
/// `sub -> .`) remain allowed. Absolute targets are rejected outright.
fn ensure_link_stays_inside(member: &Path, target: &Path, kind: &str) -> io::Result<()> {
    if target.is_absolute()
        || target
            .components()
            .any(|c| matches!(c, Component::RootDir | Component::Prefix(_)))
    {
        return Err(member_error(
            member,
            format!(
                "{kind} points at absolute target '{}' — refusing to create it",
                target.display()
            ),
        ));
    }
    if target.components().any(|c| c == Component::ParentDir) {
        return Err(member_error(
            member,
            format!(
                "{kind} target '{}' contains '..' — refusing to create it (it can resolve \
                 outside the extraction root through a symlinked component)",
                target.display()
            ),
        ));
    }
    Ok(())
}

/// Error naming the offending archive member.
fn member_error(member: &Path, message: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("archive member '{}' {message}", member.display()))
}

/// Render an error plus its source chain — the tar crate's wrapper message
/// ("failed to unpack `path`") hides the underlying errno otherwise.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
        source = cause.source();
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal raw-tar writer so tests control member names, typeflags, and
    /// link targets byte-for-byte — `tar::Builder` sanitizes some of the
    /// hostile shapes these tests must exercise.
    struct RawTar {
        bytes: Vec<u8>,
    }

    impl RawTar {
        fn new() -> Self {
            Self { bytes: Vec::new() }
        }

        fn add(&mut self, name: &str, typeflag: u8, linkname: &str, data: &[u8]) -> &mut Self {
            let mut header = [0u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            // Directories need the execute bit or files cannot be created
            // inside them.
            let mode = if typeflag == b'5' {
                b"0000755"
            } else {
                b"0000644"
            };
            header[100..107].copy_from_slice(mode);
            header[108..115].copy_from_slice(b"0000000");
            header[116..123].copy_from_slice(b"0000000");
            let size = format!("{:011o}", data.len());
            header[124..135].copy_from_slice(size.as_bytes());
            header[136..147].copy_from_slice(b"00000000000");
            header[148..156].copy_from_slice(b"        ");
            header[156] = typeflag;
            if !linkname.is_empty() {
                header[157..157 + linkname.len()].copy_from_slice(linkname.as_bytes());
            }
            header[257..263].copy_from_slice(b"ustar\0");
            header[263..265].copy_from_slice(b"00");
            let checksum: u32 = header.iter().map(|b| u32::from(*b)).sum();
            let checksum = format!("{checksum:06o}\0 ");
            header[148..156].copy_from_slice(checksum.as_bytes());
            self.bytes.extend_from_slice(&header);
            self.bytes.extend_from_slice(data);
            let pad = (512 - data.len() % 512) % 512;
            self.bytes.extend(std::iter::repeat_n(0u8, pad));
            self
        }

        fn finish(&mut self) -> Vec<u8> {
            self.bytes.extend(std::iter::repeat_n(0u8, 1024));
            std::mem::take(&mut self.bytes)
        }
    }

    #[test]
    fn clean_archive_extracts_every_member_and_accepts_internal_symlink() {
        let mut tar = RawTar::new();
        tar.add("./dir/", b'5', "", b"");
        tar.add("./dir/data.txt", b'0', "", b"payload");
        tar.add("./dir/link", b'2', "data.txt", b"");
        let bytes = tar.finish();

        let dest = tempfile::tempdir().unwrap();
        let extracted = extract_tar_requiring(bytes.as_slice(), dest.path(), &["dir/data.txt"])
            .expect("clean archive must extract");

        assert_eq!(extracted.members().len(), 3);
        assert!(extracted.contains("dir/data.txt"));
        assert_eq!(
            std::fs::read(dest.path().join("dir/data.txt")).unwrap(),
            b"payload"
        );
        assert_eq!(
            std::fs::read_link(dest.path().join("dir/link")).unwrap(),
            PathBuf::from("data.txt")
        );
    }

    #[test]
    fn contains_normalizes_leading_curdir_components() {
        let mut tar = RawTar::new();
        tar.add("./dir/data.txt", b'0', "", b"payload");
        let bytes = tar.finish();

        let dest = tempfile::tempdir().unwrap();
        let extracted = extract_tar(bytes.as_slice(), dest.path()).unwrap();

        assert!(extracted.contains("dir/data.txt"));
        assert!(
            extracted.contains("./dir/data.txt"),
            "a manifest-style `./` path must match the normalized member"
        );
        assert!(!extracted.contains("../dir/data.txt"));
        assert!(!extracted.contains("/dir/data.txt"));
        assert!(!extracted.contains("./"));
    }

    #[test]
    fn concatenated_archives_are_read_to_the_end() {
        // The remote pull appends a second tar segment (the completion
        // record) after the result tree; `ignore_zeros` must carry the
        // reader through the first segment's terminator.
        let mut tree = RawTar::new();
        tree.add("./execution.log", b'0', "", b"log");
        tree.add("./outputs/out.txt", b'0', "", b"result");
        let mut tail = RawTar::new();
        tail.add("./.oxo-flow-pull-complete", b'0', "", b"run-42");
        let mut bytes = tree.finish();
        bytes.extend_from_slice(&tail.finish());

        let dest = tempfile::tempdir().unwrap();
        let extracted =
            extract_tar_requiring(bytes.as_slice(), dest.path(), &[".oxo-flow-pull-complete"])
                .expect("the trailing completion segment must be read");

        assert_eq!(extracted.members().len(), 3);
        assert_eq!(
            std::fs::read(dest.path().join(".oxo-flow-pull-complete")).unwrap(),
            b"run-42"
        );
    }

    #[test]
    fn archive_root_entry_is_ignored_not_rejected() {
        // `tar -cf - .` (GNU tar and bsdtar alike) names the archive root
        // itself `./`; a remote result pull must not trip over that.
        let mut tar = RawTar::new();
        tar.add("./", b'5', "", b"");
        tar.add("./outputs/", b'5', "", b"");
        tar.add("./outputs/result.txt", b'0', "", b"ok");
        let bytes = tar.finish();

        let dest = tempfile::tempdir().unwrap();
        let extracted = extract_tar(bytes.as_slice(), dest.path())
            .expect("the archive root entry must be ignored");
        assert_eq!(extracted.members().len(), 2);
        assert!(extracted.contains("outputs/result.txt"));
    }

    #[test]
    fn missing_required_member_fails_naming_every_missing_member() {
        let mut tar = RawTar::new();
        tar.add("./execution.log", b'0', "", b"log");
        let bytes = tar.finish();

        let dest = tempfile::tempdir().unwrap();
        let err = extract_tar_requiring(
            bytes.as_slice(),
            dest.path(),
            &["execution.log", ".exit-code"],
        )
        .expect_err("a missing required member must fail the extraction");

        let message = err.to_string();
        assert!(
            message.contains(".exit-code"),
            "missing member must be named: {message}"
        );
        assert!(
            !message.contains("execution.log"),
            "present members must not be reported missing: {message}"
        );
    }

    #[test]
    fn parent_dir_member_is_rejected_and_nothing_is_written_outside() {
        let mut tar = RawTar::new();
        tar.add("./ok.txt", b'0', "", b"ok");
        tar.add("../escape.txt", b'0', "", b"pwned");
        let bytes = tar.finish();

        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("dest");
        let err = extract_tar(bytes.as_slice(), &dest).expect_err("a '..' member must be rejected");

        assert!(
            err.to_string().contains("../escape.txt"),
            "offending member must be named: {err}"
        );
        assert!(
            !parent.path().join("escape.txt").exists(),
            "nothing may be written outside the destination"
        );
    }

    #[test]
    fn absolute_member_is_rejected_instead_of_rewritten() {
        let mut tar = RawTar::new();
        tar.add("/abs/evil.txt", b'0', "", b"pwned");
        let bytes = tar.finish();

        let dest = tempfile::tempdir().unwrap();
        let err = extract_tar(bytes.as_slice(), dest.path())
            .expect_err("an absolute member must be rejected");

        assert!(
            err.to_string().contains("/abs/evil.txt"),
            "offending member must be named: {err}"
        );
        assert!(
            !dest.path().join("abs").exists(),
            "an absolute member must not be rewritten under the destination"
        );
    }

    #[test]
    fn escaping_symlink_is_rejected_and_never_created() {
        for target in ["../../escape-target", "/etc/passwd"] {
            let mut tar = RawTar::new();
            tar.add("./ok.txt", b'0', "", b"ok");
            tar.add("./link", b'2', target, b"");
            let bytes = tar.finish();

            let dest = tempfile::tempdir().unwrap();
            let err = extract_tar(bytes.as_slice(), dest.path())
                .expect_err("an escaping symlink must be rejected");

            assert!(
                err.to_string().contains("link") && err.to_string().contains(target),
                "error must name the member and target: {err}"
            );
            assert!(
                std::fs::symlink_metadata(dest.path().join("link")).is_err(),
                "the escaping symlink must not exist (target {target})"
            );
        }
    }

    #[test]
    fn symlink_chain_through_dotdot_is_rejected() {
        // The bypass the lexical check missed: `x -> .` makes `x/..` resolve
        // to the destination PARENT even though `x` and `..` cancel on paper.
        let mut tar = RawTar::new();
        tar.add("./x", b'2', ".", b"");
        tar.add("./y", b'2', "x/..", b"");
        let bytes = tar.finish();

        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("dest");
        let err = extract_tar(bytes.as_slice(), &dest)
            .expect_err("a `..` in a link target must be rejected");

        assert!(
            err.to_string().contains("'y'") && err.to_string().contains(".."),
            "error must name the member and the offending target: {err}"
        );
        assert!(
            std::fs::symlink_metadata(dest.join("y")).is_err(),
            "the escaping chain link must not be created"
        );
    }

    #[test]
    fn chained_symlink_through_dotdot_is_rejected() {
        // Longer chain: a3 -> a2/.. resolves through a2 -> a1 -> `.` to the
        // destination parent.
        let mut tar = RawTar::new();
        tar.add("./a1", b'2', ".", b"");
        tar.add("./a2", b'2', "a1", b"");
        tar.add("./a3", b'2', "a2/..", b"");
        let bytes = tar.finish();

        let dest = tempfile::tempdir().unwrap();
        let err = extract_tar(bytes.as_slice(), dest.path())
            .expect_err("a chained `..` link target must be rejected");

        assert!(err.to_string().contains("a2/.."), "got: {err}");
        assert!(std::fs::symlink_metadata(dest.path().join("a3")).is_err());
    }

    #[test]
    fn link_target_with_dotdot_is_rejected_even_when_lexically_inside() {
        // Policy: `..` is never accepted in a link target, even when written
        // form would resolve inside (`sub/up -> ../data.txt`).
        let mut tar = RawTar::new();
        tar.add("./sub/", b'5', "", b"");
        tar.add("./sub/data.txt", b'0', "", b"payload");
        tar.add("./sub/up", b'2', "../data.txt", b"");
        let bytes = tar.finish();

        let dest = tempfile::tempdir().unwrap();
        let err = extract_tar(bytes.as_slice(), dest.path())
            .expect_err("a `..` link target must be rejected unconditionally");
        assert!(err.to_string().contains("sub/up"), "got: {err}");
    }

    #[test]
    fn escaping_hard_link_is_rejected() {
        let mut tar = RawTar::new();
        tar.add("./link", b'1', "../outside", b"");
        let bytes = tar.finish();

        let dest = tempfile::tempdir().unwrap();
        let err = extract_tar(bytes.as_slice(), dest.path())
            .expect_err("an escaping hard link must be rejected");
        assert!(err.to_string().contains("../outside"), "got: {err}");
    }

    #[test]
    fn symlink_through_which_a_later_member_writes_never_escapes() {
        // `sub` is a valid in-tree symlink to `.`; the write through it stays
        // inside and is allowed, while an escaping link is rejected above.
        let mut tar = RawTar::new();
        tar.add("./sub", b'2', ".", b"");
        tar.add("./sub/inside.txt", b'0', "", b"inside");
        let bytes = tar.finish();

        let dest = tempfile::tempdir().unwrap();
        let extracted =
            extract_tar(bytes.as_slice(), dest.path()).expect("in-tree symlink chain is safe");
        assert!(extracted.contains("sub/inside.txt"));
        assert_eq!(
            std::fs::read(dest.path().join("sub/inside.txt")).unwrap(),
            b"inside"
        );
    }

    #[test]
    fn unsupported_member_type_fails_closed() {
        let mut tar = RawTar::new();
        tar.add("./fifo", b'6', "", b"");
        let bytes = tar.finish();

        let dest = tempfile::tempdir().unwrap();
        let err =
            extract_tar(bytes.as_slice(), dest.path()).expect_err("a FIFO member must be rejected");
        assert!(
            err.to_string().contains("unsupported member type"),
            "got: {err}"
        );
    }
}
