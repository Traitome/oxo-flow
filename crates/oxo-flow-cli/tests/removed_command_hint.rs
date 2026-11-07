//! Integration tests for the removed-command migration hints (issue #142 H2):
//! `history`, `package`, `profile`, and `watch` were removed in v0.13, but the
//! binary must keep answering them with the actual migration path (instead of
//! clap's bare "unrecognized subcommand") AND exit non-zero so shell scripts
//! and CI pipelines fail loudly instead of silently continuing.

use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// Each removed command with the replacement text its hint must carry.
const REMOVED_COMMANDS: &[(&str, &str)] = &[
    ("history", "oxo-flow status --timing"),
    ("package", "oxo-flow export"),
    ("profile", "run --profile <NAME>"),
    ("watch", "oxo-flow status"),
];

#[test]
fn removed_commands_exit_non_zero_with_migration_hint() {
    for (cmd, replacement) in REMOVED_COMMANDS {
        let out = oxo().arg(cmd).output().unwrap();

        let code = out.status.code();
        assert_eq!(
            code,
            Some(1),
            "'oxo-flow {cmd}' must exit 1 so scripts fail loudly; stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(cmd) && stderr.contains("removed in v0.13"),
            "'oxo-flow {cmd}' must name the removal in its hint; stderr:\n{stderr}"
        );
        assert!(
            stderr.contains(replacement),
            "'oxo-flow {cmd}' hint must point at '{replacement}'; stderr:\n{stderr}"
        );
        assert!(
            stderr.contains("oxo-flow --help"),
            "'oxo-flow {cmd}' hint must point at --help; stderr:\n{stderr}"
        );

        // The migration hint replaces clap's parser, so no
        // "unrecognized subcommand" noise may leak through.
        assert!(
            !stderr.contains("unrecognized subcommand"),
            "'oxo-flow {cmd}' must not fall through to clap's bare error; stderr:\n{stderr}"
        );
    }
}

#[test]
fn removed_command_hint_survives_global_flags_before_it() {
    // Global boolean flags before the subcommand must not shift the
    // detected subcommand position (issue #142 H2 scan contract).
    for (cmd, _) in REMOVED_COMMANDS {
        let out = oxo().arg("--json").arg(cmd).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(1),
            "'oxo-flow --json {cmd}' must still exit 1; stderr:\n{stderr}"
        );
        assert!(
            stderr.contains("removed in v0.13"),
            "'oxo-flow --json {cmd}' must still hint; stderr:\n{stderr}"
        );
    }
}
