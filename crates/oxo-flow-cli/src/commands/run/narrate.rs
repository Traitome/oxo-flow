//! Run-log narration plumbing: the shared run-log slot, tee helpers, console/diagnostic emitters, ANSI stripping, execution-event fan-out (#948 P2-5).

use super::*;

/// The active run log, shared so spawned rule tasks can write their own
/// narrative lines — the tee's file side is private to `crate::logging`, and
/// the guard cannot move into a spawned task while the loop still needs it.
pub(crate) type SharedRunLog = Arc<std::sync::Mutex<Option<crate::logging::RunLogGuard>>>;

/// Append one plain-text line to the active run log (the shared guard slot).
/// Failures degrade to a debug trace — the log is best-effort and the console
/// copy has already gone out.
pub(crate) fn write_run_log_line(run_log: &SharedRunLog, text: &str) {
    let mut slot = run_log
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(log) = slot.as_mut() {
        let line = format!("{}\n", text.trim_start());
        if let Err(e) = log.write_all(line.as_bytes()) {
            tracing::debug!(error = %e, "narrative line could not be written to the run log");
        }
    }
}

/// Write one console line WITHOUT panicking on failure. The historical
/// `eprintln!` aborts its task on any write error — with stderr piped to a
/// reader that exits early (`| head`, `| grep -q`, a closed CI fd) that is
/// "failed printing to stderr: Broken pipe (os error 32)", and inside a
/// spawned rule task the panic surfaced through `JoinSet::join_next` as an
/// engine fault: the HEALTHY rule got recorded as Failed in the checkpoint
/// with the panic text as its stderr tail. The console surface is
/// best-effort — a failed write is traced and dropped; the run-log copy
/// still lands because [`write_run_log_line`] has always been tolerant.
pub(crate) fn narrate_line_to<W: std::io::Write>(sink: &mut W, msg: std::fmt::Arguments<'_>) {
    if let Err(e) = writeln!(sink, "{msg}") {
        tracing::trace!(error = %e, "console narrate line dropped (stderr unavailable)");
    }
}

/// Emit one progress narrative line exactly TWICE in total — once on stderr
/// (the console surface) and once in the archived run log (issue #194 B1).
/// The historical form routed the log copy through the tracing stream, whose
/// writer also targets stderr, so every line printed twice and a leading
/// blank line in the message surfaced as an empty INFO row (audit P4-11).
/// Colors live in the console form; the log side gets plain text.
///
/// Background runs hold no run log (their stderr already lands in it), so
/// only the console copy is written.
pub(crate) fn progress_narrate(msg: std::fmt::Arguments<'_>, run_log: &SharedRunLog) {
    // --quiet silences the console copy only: the run log is an artifact the
    // report/web UI reads, not terminal noise.
    if !crate::commands::is_quiet() {
        narrate_console_line(msg);
    }
    write_run_log_line(run_log, &strip_ansi(&msg.to_string()));
}

/// Console-side half of the narrate helpers: targets the process stderr via
/// the tolerant writer, so a closed/early-exiting pipe reader can never panic
/// a rule task mid-flight.
pub(crate) fn narrate_console_line(msg: std::fmt::Arguments<'_>) {
    narrate_line_to(&mut std::io::stderr(), msg);
}

/// Emit one audit-trail diagnostic line to stderr AND the run log: adoption
/// notes, config-change summaries, and invalidation cascades explain WHY
/// checkpoint credit was lost, and nohup/CI users only keep the run-log file
/// (issue #484). Unlike [`progress_narrate`] the console copy is NOT gated on
/// --quiet — these lines print unconditionally today and operators parse them.
/// Callers outside the run path (dry-run, status) pass `None` and keep the
/// historic stderr-only behavior; background runs also pass a file-less slot
/// (their stderr already lands in the log), so nothing is written twice.
pub(crate) fn diagnostic_narrate(msg: std::fmt::Arguments<'_>, run_log: Option<&SharedRunLog>) {
    narrate_console_line(msg);
    if let Some(run_log) = run_log {
        write_run_log_line(run_log, &strip_ansi(&msg.to_string()));
    }
}

/// Narrate the #837 workdir source-linking pass: what was linked, what was
/// converted from a duplicate copy, and which references need operator
/// attention (conflicts, undeclared shell paths, filesystem failures).
pub(crate) fn narrate_source_link_report(
    report: &crate::commands::workdir_sources::SourceLinkReport,
    run_log: Option<&SharedRunLog>,
) {
    if let Some(note) = &report.skipped_note {
        diagnostic_narrate(format_args!("workdir source linking: {}", note), run_log);
        return;
    }
    if report.is_empty() {
        return;
    }
    if !report.linked.is_empty() {
        diagnostic_narrate(
            format_args!(
                "workdir source linking: created {} into the workdir (repo stays the single source of truth): {}",
                oxo_flow_core::plural(report.linked.len(), "symlink", "symlinks"),
                report.linked.join(", ")
            ),
            run_log,
        );
    }
    if !report.sidecars.is_empty() {
        diagnostic_narrate(
            format_args!(
                "workdir source linking: linked {} next to their linked files (bwa/samtools/picard auto-discovery — not spelled out in any shell command): {}",
                oxo_flow_core::plural(
                    report.sidecars.len(),
                    "tool-convention index sidecar",
                    "tool-convention index sidecars"
                ),
                report.sidecars.join(", ")
            ),
            run_log,
        );
    }
    if !report.converted.is_empty() {
        diagnostic_narrate(
            format_args!(
                "workdir source linking: replaced {} byte-identical workdir duplicates with {}: {}",
                report.converted.len(),
                oxo_flow_core::plural(report.converted.len(), "a symlink", "symlinks"),
                report.converted.join(", ")
            ),
            run_log,
        );
    }
    for path in &report.conflicts {
        diagnostic_narrate(
            format_args!(
                "{} workdir source linking: '{}' differs from the workflow-repo file — keeping the workdir copy for this run (remove the copy or update the repo to link it)",
                "Warning:".yellow(),
                path
            ),
            run_log,
        );
    }
    if !report.undeclared.is_empty() {
        diagnostic_narrate(
            format_args!(
                "{} workdir source linking: shell commands reference workflow-repo files that no rule declares as inputs — they were NOT linked (declaring the path as a rule input links it automatically):\n  {}",
                "Note:".yellow(),
                report.undeclared.join("\n  ")
            ),
            run_log,
        );
    }
    for err in &report.errors {
        diagnostic_narrate(
            format_args!("{} workdir source linking: {}", "Warning:".yellow(), err),
            run_log,
        );
    }
}

/// Strip ANSI escape sequences: run-log files are plain text. Mirrors the
/// file-side filter of `crate::logging`, which exposes no public writer.
pub(crate) fn strip_ansi(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut plain = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                // CSI sequence: ESC '[' parameters final-byte.
                i += 2;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            } else {
                // Stray escape.
                i += 1;
            }
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != 0x1b {
                i += 1;
            }
            // 0x1b never appears inside a multi-byte UTF-8 sequence.
            plain.push_str(&text[start..i]);
        }
    }
    plain
}

/// Emit a structured
/// [`ExecutionEvent`](oxo_flow_core::executor::ExecutionEvent) as one JSON
/// line in the tracing
/// stream (issue #194 B3): the event schema stops being dead code and the
/// run log gains a machine-readable execution timeline alongside the prose.
/// Background runs go through the same tracing call: ANSI is already off
/// under the stderr redirect (`main.rs` folds
/// `OXO_FLOW_STDERR_ALREADY_REDIRECTED` into its color-disabled set) and the
/// run-log tee stays unarmed there, so the line reaches the redirected
/// stderr exactly once and lands in the run log via the redirect.
pub(crate) fn emit_execution_event(event: oxo_flow_core::executor::ExecutionEvent) {
    tracing::info!(target: "execution_event", "{}", event.to_json_log());
}

#[cfg(test)]
mod sigpipe_narrate_tests {
    //! Regression coverage for the A#1 persona finding: a rule task printing
    //! progress to stderr whose reader exited early must NOT panic (broken
    //! pipe) — the panic surfaced via `JoinSet` and poisoned the checkpoint
    //! with a Failed entry for a healthy rule. The tolerant writer is the
    //! whole fix; these tests pin it in place.

    use super::{diagnostic_narrate, narrate_line_to, progress_narrate, write_run_log_line};

    /// A sink that is always full/dead: every write fails with EPIPE.
    struct DeadSink;

    impl std::io::Write for DeadSink {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(32))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn slot() -> super::SharedRunLog {
        std::sync::Arc::new(std::sync::Mutex::new(None))
    }

    #[test]
    fn narrate_line_to_survives_dead_sink() {
        let mut sink = DeadSink;
        // Must not panic — the write error is traced and swallowed.
        narrate_line_to(&mut sink, format_args!("  ✓ greet (0.1s)"));
        narrate_line_to(&mut sink, format_args!("\nexit code: 1"));
        narrate_line_to(&mut sink, format_args!(""));
    }

    #[test]
    fn narrate_helpers_survive_dead_console_with_log() {
        let run_log = slot();
        progress_narrate(format_args!("  ✓ greet (0.1s)"), &run_log);
        diagnostic_narrate(format_args!("  ⚠ adoption note"), Some(&run_log));
        diagnostic_narrate(format_args!("  ⚠ operator hint"), None);
        write_run_log_line(&run_log, "plain line");
    }
}
