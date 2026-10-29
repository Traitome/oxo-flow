# verify-data

Verify a data tree against a vendor md5 sidecar (`md5sum` text format) and
report mismatched, missing, and extra files in one pass. Turns post-incident
audits and post-recovery verification into a single command (issue #935 P2).

## Usage

```bash
oxo-flow verify-data <TREE> --from <MD5_FILE> [OPTIONS]
```

## Description

Deliveries commonly ship `md5.txt` sidecars — the #926 incident tree did, and
it is the only reason the corrupted files' original hashes were known.
`verify-data` walks `<TREE>` and compares every file listed in the sidecar
against its recorded md5, then also scans the tree for files the sidecar does
not list (extras).

- Sidecar paths resolve relative to `<TREE>`. Both standard `md5sum` text
  forms are accepted: `d41d8cd9…42  path` (two spaces) and
  `d41d8cd9…42 *path` (space + asterisk, binary mode). A leading `./` on
  sidecar paths is stripped so sidecars written from inside the tree agree
  with trees passed with a trailing slash.
- Blank lines and `#` comments in the sidecar are skipped.
- A malformed sidecar line (digest not 32 hex chars, empty path, duplicate
  path, or a path that escapes the tree via `..` or an absolute path) is a
  hard error naming the line number. Silently skipping a line would report a
  corrupted delivery as clean — the exact failure this command exists to
  catch.
- The engine's own `.oxo-flow/` artifact directory inside the tree is skipped
  during the extras walk; it is bookkeeping, never delivery content. If the
  sidecar itself lives inside the tree, it is excluded from the extras scan
  (hashing it against itself would always mismatch).
- FIFOs and other non-regular files are never opened for checksumming (same
  stat-first gate as the run-time checksum helpers — `File::open` on a
  writer-less FIFO would block forever).
- Symlinks are never followed. A symlink the sidecar does not list is not
  reported as an extra (the walk neither follows nor hashes it); a symlink
  the sidecar does list resolves through its target — the target's content
  is what gets hashed, so a retargeted or corrupted target still mismatches
  and fails the audit. Deliveries that must contain links should audit them
  out of band; `verify-data`'s verdict covers regular files only.
- A subdirectory that cannot be read (e.g. lost execute permission) does not
  silently shrink the audit: the walk reports the traversal failure as an
  extra entry with a `tree walk failed` note, and the summary's `extra`
  count includes it, so the audit exits `1` instead of passing a tree it
  could not fully inspect. This is deliberately fail-closed even for
  engine-internal (`.oxo-flow/`) or ignored paths — an unreadable directory
  means part of the tree was never seen, which invalidates a clean verdict.

## Options

| Option | Description |
|--------|-------------|
| `<TREE>` | Directory to verify (positional) |
| `--from <MD5_FILE>` | md5 sidecar to verify against, in `md5sum` text format |
| `--json` | Emit a machine-readable JSON document on stdout; the human report stays on stderr |
| `-v, --verbose` | Enable verbose (debug-level) logging |
| `--quiet` | Suppress non-essential output (errors only) |
| `--no-color` | Disable colored output (also respects `NO_COLOR`) |

## Exit Code

`0` when every listed file matches and nothing is missing or extra; `1` when
any listed file mismatches, is missing, or is unlisted — so the command drops
straight into scripts and post-recovery gates. (An empty sidecar — zero
listed files — reports a note and exits `0` without scanning for extras.)

## Examples

```bash
# Audit a delivery against its sidecar
oxo-flow verify-data raw/ --from raw/md5.txt

# Machine-readable result for scripts
oxo-flow --json verify-data raw/ --from raw/md5.txt

# Gate a recovery: only proceed when the restored tree is byte-clean
oxo-flow verify-data raw/ --from raw/md5.txt && oxo-flow run pipeline.oxoflow
```

## Output

Human report on stderr; exit code carries the verdict:

```text
Verify Data /data/raw against /data/raw/md5.txt

  ✓ raw/S1.fastq 020a9b001f7b28cef19de98df57a904d
  ✓ raw/S2.fastq 8d395b7db458642f77197096769d0ce9
  ✗ raw/S3.fastq (expected: 513464b728fd8dec2039cff5710be0ca, actual: ec8f8521de9a1c8b70d2138a65ee6c50)
  ! raw/notes.txt (not listed in sidecar, md5 7b48666b13c02ffd7122df4275adc002)

Summary: 2 matched, 1 mismatched, 0 missing, 1 extra (not listed)
```

## JSON Output

With `--json`, stdout carries only the document (`entries` is byte-stable:
sidecar entries in sorted path order, then extras in sorted path order):

```json
{
  "command": "verify-data",
  "tree": "/data/raw",
  "from": "/data/raw/md5.txt",
  "matched": 2,
  "mismatched": 1,
  "missing": 0,
  "extra": 1,
  "entries": [
    {
      "file": "raw/S1.fastq",
      "status": "matched",
      "expected": "020a9b001f7b28cef19de98df57a904d",
      "actual": "020a9b001f7b28cef19de98df57a904d"
    },
    {
      "file": "raw/S3.fastq",
      "status": "mismatched",
      "expected": "513464b728fd8dec2039cff5710be0ca",
      "actual": "ec8f8521de9a1c8b70d2138a65ee6c50"
    },
    {
      "file": "raw/notes.txt",
      "status": "extra",
      "actual": "7b48666b13c02ffd7122df4275adc002"
    }
  ]
}
```

Entry statuses: `matched`, `mismatched` (including checksum-read failures,
which carry a `note`), `missing` (listed in the sidecar, absent on disk), and
`extra` (on disk, unlisted — `expected` is absent by definition; this also
covers unreadable-subdirectory traversal failures, which carry a
`tree walk failed` note instead of a checksum).

## Notes

- The command is a pure audit: it never modifies the tree, the sidecar, or
  any checkpoint state.
- md5 is used because that is what vendor sidecars ship; it is a detection
  surface, not a security boundary. For tamper-evidence the engine's own
  provenance checksums (sha256) remain the source of truth — see
  [provenance](provenance.md).
- To detect drift continuously during a run (not just as a one-shot audit),
  declare the tree under `[engine] protected_data_trees` — see
  [run](run.md#protected-data-trees-protected_data_trees).

## See Also

- [provenance](provenance.md) — verify run outputs against the checkpoint's stored checksums
- [run](run.md#protected-data-trees-protected_data_trees) — whole-tree drift detection during runs
- [workflow format reference](../reference/workflow-format.md) — `[engine]` section
- [web API reference](../reference/web-api.md) — `POST /api/data/verify` runs this audit over HTTP on a user-workspace tree
