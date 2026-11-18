//! Bundle archive extraction safety (issue #1157, audit CL-04 extraction).
//!
//! These tests drive the real CLI: a crafted `.tar.gz` bundle is handed to
//! `run --bundle`, whose verification/extraction step must reject hostile
//! members instead of accepting them the way `tar::Archive::unpack` does.
//!
//! Every traversal fixture fails the test on the pre-#1157 extractor: `..`
//! members were silently skipped, absolute members silently rewritten under
//! the destination, and escaping symlinks created verbatim — each case then
//! "verified" and ran the bundled workflow successfully.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Minimal raw-tar writer (the `tar` crate's builder refuses `..` and
/// absolute paths, so hostile archives must be assembled byte-for-byte).
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
        header[100..107].copy_from_slice(b"0000644");
        header[108..115].copy_from_slice(b"0000000");
        header[116..123].copy_from_slice(b"0000000");
        header[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
        header[136..147].copy_from_slice(b"00000000000");
        header[148..156].copy_from_slice(b"        ");
        header[156] = typeflag;
        if !linkname.is_empty() {
            header[157..157 + linkname.len()].copy_from_slice(linkname.as_bytes());
        }
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u32 = header.iter().map(|b| u32::from(*b)).sum();
        header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
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

const WORKFLOW: &str = "[workflow]\nname = \"archive\"\nversion = \"1.0.0\"\n\n\
[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo ok > {output[0]}\"\n";

/// A workflow that references a NESTED data file, so a legitimately published
/// bundle carries a `data/meta.tsv` member whose manifest path is relative and
/// nested. The manifest-path guard must keep such paths verifying.
const NESTED_WORKFLOW: &str = "[workflow]\nname = \"nested\"\nversion = \"1.0.0\"\n\
metadata_file = \"data/meta.tsv\"\n\n\
[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo ok > {output[0]}\"\n";

fn manifest(files: &str) -> String {
    format!(
        "{{\n  \"format\": \"oxoflow-bundle-v1\",\n  \"name\": \"archive\",\n  \
         \"version\": \"1.0.0\",\n  \"entrypoint\": \"wf.oxoflow\",\n  \
         \"files\": [{files}],\n  \"signatures\": []\n}}\n"
    )
}

/// Write a gzip-compressed bundle containing `manifest.json`, `wf.oxoflow`,
/// and the caller's extra entries.
fn write_bundle(dir: &Path, name: &str, files: &str, extra: &mut RawTar) -> PathBuf {
    let mut tar = RawTar::new();
    let manifest = manifest(files);
    tar.add("./manifest.json", b'0', "", manifest.as_bytes());
    tar.add("./wf.oxoflow", b'0', "", WORKFLOW.as_bytes());
    // Copy the extra members in after the standard ones.
    let extra_bytes = extra.finish();
    // `finish` appends a 1024-byte zero terminator; strip it so the members
    // follow the workflow entry inside one archive.
    let body = &extra_bytes[..extra_bytes.len() - 1024];
    tar.bytes.extend_from_slice(body);

    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    let mut gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    gz.write_all(&tar.finish()).unwrap();
    gz.finish().unwrap();
    path
}

fn oxo_flow() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

fn run_bundle(bundle: &Path, workdir: &Path) -> std::process::Output {
    std::fs::create_dir_all(workdir).unwrap();
    oxo_flow()
        .args([
            "run",
            "--bundle",
            bundle.to_str().unwrap(),
            "--yes",
            "--workdir",
        ])
        .arg(workdir)
        .output()
        .unwrap()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn bundle_with_parent_dir_member_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut extra = RawTar::new();
    extra.add("../escape.txt", b'0', "", b"pwned");
    let bundle = write_bundle(dir.path(), "escape.tar.gz", "", &mut extra);

    let workdir = dir.path().join("work");
    let output = run_bundle(&bundle, &workdir);

    assert!(
        !output.status.success(),
        "a '..' member must fail the bundle, stderr: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("../escape.txt"),
        "the offending member must be named, stderr: {}",
        stderr(&output)
    );
    assert!(
        !dir.path().join("escape.txt").exists(),
        "nothing may be written outside the extraction root"
    );
}

#[test]
fn bundle_with_absolute_member_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut extra = RawTar::new();
    extra.add("/abs/evil.txt", b'0', "", b"pwned");
    let bundle = write_bundle(dir.path(), "absolute.tar.gz", "", &mut extra);

    let output = run_bundle(&bundle, &dir.path().join("work"));

    assert!(
        !output.status.success(),
        "an absolute member must fail the bundle, stderr: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("/abs/evil.txt"),
        "the offending member must be named, stderr: {}",
        stderr(&output)
    );
}

#[test]
fn bundle_with_escaping_symlink_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut extra = RawTar::new();
    extra.add("./link", b'2', "../../escape-target", b"");
    let bundle = write_bundle(dir.path(), "symlink.tar.gz", "", &mut extra);

    let output = run_bundle(&bundle, &dir.path().join("work"));

    assert!(
        !output.status.success(),
        "an escaping symlink must fail the bundle, stderr: {}",
        stderr(&output)
    );
    let message = stderr(&output);
    assert!(
        message.contains("link") && message.contains("../../escape-target"),
        "the offending symlink and target must be named, stderr: {message}"
    );
}

#[test]
fn bundle_missing_a_declared_member_fails_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    // Manifest declares a file the archive does not carry.
    let files = r#"{"path": "data/missing.txt", "sha256": "0000000000000000000000000000000000000000000000000000000000000000"}"#;
    let bundle = write_bundle(dir.path(), "missing.tar.gz", files, &mut RawTar::new());

    let output = run_bundle(&bundle, &dir.path().join("work"));

    assert!(
        !output.status.success(),
        "a missing declared member must fail the bundle, stderr: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("data/missing.txt"),
        "the missing member must be named, stderr: {}",
        stderr(&output)
    );
}

#[test]
fn clean_bundle_still_extracts_and_runs() {
    let dir = tempfile::tempdir().unwrap();

    // Publish a real bundle so checksums are authentic, then run it.
    let wf = dir.path().join("clean.oxoflow");
    std::fs::write(&wf, WORKFLOW).unwrap();
    let publish = oxo_flow()
        .args(["publish", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        publish.status.success(),
        "publish failed: {}",
        String::from_utf8_lossy(&publish.stderr)
    );
    let bundle = dir.path().join("clean-bundle.tar.zst");
    assert!(bundle.exists(), "publish must produce the bundle");

    let workdir = dir.path().join("work");
    let output = run_bundle(&bundle, &workdir);

    assert!(
        output.status.success(),
        "clean bundle must still run, stderr: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("Bundle verified"),
        "verification must still succeed, stderr: {}",
        stderr(&output)
    );
    assert!(workdir.join("out.txt").is_file(), "the rule must run");
}

/// Publish a bundle, read its `manifest.json` back out of the archive, and
/// return `(bundle_path, manifest_json_string)`.
///
/// `rewrite` may rewrite the manifest before it is re-packed, so a test can
/// hand the real CLI a manifest whose checksummed `files[].path` is hostile
/// while every byte the publisher produced stays authentic.
fn published_bundle_with_manifest(
    dir: &Path,
    workflow: &str,
    rewrite: impl FnOnce(&str) -> String,
) -> PathBuf {
    let wf = dir.join("wf.oxoflow");
    std::fs::write(&wf, workflow).unwrap();
    let publish = oxo_flow()
        .args(["publish", wf.to_str().unwrap()])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        publish.status.success(),
        "publish failed: {}",
        String::from_utf8_lossy(&publish.stderr)
    );
    let bundle = dir.join("wf-bundle.tar.zst");
    assert!(bundle.exists(), "publish must produce the bundle");

    // Read the manifest back out of the archive the publisher produced.
    let reader = zstd::stream::read::Decoder::new(std::fs::File::open(&bundle).unwrap()).unwrap();
    let mut archive = tar::Archive::new(reader);
    let mut manifest_json = String::new();
    let mut members: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if path.ends_with("manifest.json") {
            manifest_json = String::from_utf8(data.clone()).unwrap();
        }
        members.push((path, data));
    }
    let tampered = rewrite(&manifest_json);
    let mut out = tar::Builder::new(Vec::new());
    for (path, data) in &members {
        let body = if path.ends_with("manifest.json") {
            tampered.as_bytes()
        } else {
            data.as_slice()
        };
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        out.append_data(&mut header, path, body).unwrap();
    }
    let unpacked = out.into_inner().unwrap();

    let file = std::fs::File::create(&bundle).unwrap();
    let mut zst = zstd::stream::write::Encoder::new(file, 0).unwrap();
    zst.write_all(&unpacked).unwrap();
    zst.finish().unwrap();
    bundle
}

/// The manifest `files[].path` traversal guard (#1157, audit CL-04): a
/// crafted manifest declaring `files[].path = "../../escape.txt"` must be
/// rejected by name, and verification must never read a file OUTSIDE the
/// verified extraction.
#[test]
fn manifest_declaring_a_parent_dir_file_path_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    // A real file at the escape location, so a `join` (pre-#1157) would read
    // it and "verify" against far-fetched checksum-outsider semantics.
    std::fs::write(dir.path().join("escape.txt"), b"pwned").unwrap();
    let bundle = published_bundle_with_manifest(dir.path(), WORKFLOW, |json| {
        json.replace(r#""path": "wf.oxoflow""#, r#""path": "../../escape.txt""#)
    });

    let output = run_bundle(&bundle, &dir.path().join("work"));

    assert!(
        !output.status.success(),
        "a manifest '..' file path must fail the bundle, stderr: {}",
        stderr(&output)
    );
    let message = stderr(&output);
    assert!(
        message.contains("../../escape.txt") && message.contains("contains '..'"),
        "the offending manifest path must be named with the escape reason, stderr: {message}"
    );
}

/// The absolute-path half of the same guard.
#[test]
fn manifest_declaring_an_absolute_file_path_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = published_bundle_with_manifest(dir.path(), WORKFLOW, |json| {
        json.replace(r#""path": "wf.oxoflow""#, r#""path": "/etc/hostname""#)
    });

    let output = run_bundle(&bundle, &dir.path().join("work"));

    assert!(
        !output.status.success(),
        "an absolute manifest file path must fail the bundle, stderr: {}",
        stderr(&output)
    );
    let message = stderr(&output);
    assert!(
        message.contains("/etc/hostname") && message.contains("absolute"),
        "the offending manifest path must be named with the absolute reason, stderr: {message}"
    );
}

/// A legitimately nested relative path must still verify — the guard rejects
/// escapes, not depth.
#[test]
fn manifest_nested_relative_file_path_still_verifies() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(dir.path().join("data/meta.tsv"), b"sample\tgroup\n").unwrap();

    let wf = dir.path().join("nested.oxoflow");
    std::fs::write(&wf, NESTED_WORKFLOW).unwrap();
    let publish = oxo_flow()
        .args(["publish", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        publish.status.success(),
        "publish failed: {}",
        String::from_utf8_lossy(&publish.stderr)
    );
    let bundle = dir.path().join("nested-bundle.tar.zst");
    assert!(bundle.exists(), "publish must produce the bundle");

    let workdir = dir.path().join("work");
    let output = run_bundle(&bundle, &workdir);

    assert!(
        output.status.success(),
        "a nested relative manifest path must still verify, stderr: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("Bundle verified"),
        "verification must still succeed, stderr: {}",
        stderr(&output)
    );
}
