//! Docs ↔ CLI drift gate. The README's command-category table and the
//! "N subcommands" prose across the docs hand-mirror the clap definition;
//! this gate derives the truth from the built binary's `--help` and fails
//! on any divergence (missing/extra table entries, stale counts). The
//! GitHub Pages landing page's command cards (`docs/index.html`) hand-write
//! each subcommand's name, category badge, and description — the same gate
//! keeps that page in lockstep, including its "N CLI subcommands" prose
//! (English and Chinese), which is only checked when it is adjacent to the
//! word "subcommand"/"子命令", so unrelated numbers never trip the gate.

mod common;

use common::workspace_bin;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// Docs that quote the subcommand count in prose. Every `\d+ subcommands?`
/// (and `\d+ CLI subcommands`) occurrence in these files must match the
/// count clap reports.
const COUNT_MENTIONS: &[&str] = &[
    "README.md",
    "AGENTS.md",
    "CONTRIBUTING.md",
    "docs/guide/src/index.md",
    "docs/guide/src/reference/architecture.md",
    "docs/superpowers/plans/linux-server-validation-protocol.md",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// (subcommands, aliases) parsed from `--help`. Alias lines read
/// `dry-run      ... (alias: plan)`.
fn clap_subcommands() -> (Vec<String>, BTreeSet<String>) {
    let output = std::process::Command::new(workspace_bin("oxo-flow"))
        .arg("--help")
        .output()
        .expect("oxo-flow binary runs — build it before this gate");
    let help = String::from_utf8(output.stdout).expect("help output is UTF-8");
    let mut commands = Vec::new();
    let mut aliases = std::collections::BTreeSet::new();
    let mut in_commands = false;
    for line in help.lines() {
        if line.starts_with("Commands:") {
            in_commands = true;
            continue;
        }
        if !in_commands {
            continue;
        }
        // Sections are separated by a blank line or the next heading
        // ("Options:"); entries are indented "  name  description".
        if line.trim().is_empty() || !line.starts_with(char::is_whitespace) {
            break;
        }
        let name = line.split_whitespace().next().unwrap_or_default();
        if !name.is_empty() && name != "help" {
            commands.push(name.to_string());
            if let Some(alias) = line
                .split("(alias: ")
                .nth(1)
                .and_then(|rest| rest.split(')').next())
            {
                aliases.insert(alias.trim().to_string());
            }
        }
    }
    (commands, aliases)
}

/// Backticked names inside the README's command-category table rows
/// (`| **Category** | \`a\`, \`b\` (alias \`x\`) |`). Alias annotations
/// are stripped before extraction — clap hides aliases from `--help`, and
/// an invalid alias fails loudly at invocation anyway.
fn readme_table_commands(readme: &str) -> Vec<String> {
    let mut commands = Vec::new();
    // Scope to the command table: the slice between the subcommand-count
    // paragraph and the next top-level heading. Other `| **...**` tables
    // elsewhere in the README backtick flags and config keys, not commands.
    let start = readme
        .find("provides **")
        .expect("README subcommand-count paragraph present");
    let end = readme[start..]
        .find("\n## ")
        .map(|i| start + i)
        .unwrap_or(readme.len());
    for line in readme[start..end].lines().filter(|l| l.starts_with("| **")) {
        // Remove only the `(alias ...)` span, not the rest of the row.
        let line: String = match (line.find("(alias"), line.find(')')) {
            (Some(start), Some(end)) if end > start => {
                format!("{}{}", &line[..start], &line[end + 1..])
            }
            _ => line.to_string(),
        };
        for span in line.split('`').skip(1).step_by(2) {
            let name = span.trim();
            if !name.is_empty() {
                commands.push(name.to_string());
            }
        }
    }
    commands
}

/// `cli-cmd-name` anchor texts on the landing page, in document order.
fn landing_page_cards(html: &str) -> Vec<String> {
    let mut cards = Vec::new();
    let mut tail = html;
    while let Some(pos) = tail.find("cli-cmd-name\">") {
        let rest = &tail[pos + "cli-cmd-name\">".len()..];
        let name: String = rest.chars().take_while(|c| *c != '<').collect();
        if !name.is_empty() {
            cards.push(name);
        }
        tail = rest;
    }
    cards
}

/// Trim the characters that may separate a count from the keyword it
/// qualifies: whitespace, Markdown emphasis (`**34** subcommands`), and the
/// backticks/underscores used around inline code.
fn trim_count_separators(s: &str) -> &str {
    s.trim_end_matches(|c: char| c.is_whitespace() || c == '*' || c == '`' || c == '_')
}

/// Digits ending `s`, with nothing else trailing.
fn trailing_digits(s: &str) -> Option<String> {
    let count = s.bytes().rev().take_while(|b| b.is_ascii_digit()).count();
    let digits = &s[s.len() - count..];
    (!digits.is_empty()).then(|| digits.to_string())
}

/// `N subcommands`, `N CLI subcommands`, `N CLI SUBCOMMANDS` — the count
/// must sit immediately before the keyword (optionally through the `CLI`
/// qualifier), so ports, versions, and "100+ APIs" can never match.
fn digits_before_english_subcommand(prefix: &str) -> Option<String> {
    let mut tail = trim_count_separators(prefix);
    if tail.to_ascii_lowercase().ends_with("cli") {
        tail = trim_count_separators(&tail[..tail.len() - "cli".len()]);
    }
    trailing_digits(tail)
}

/// Chinese counterpart: `N 个 (CLI) 子命令`.
fn digits_before_chinese_subcommand(prefix: &str) -> Option<String> {
    let mut tail = trim_count_separators(prefix);
    if tail.to_ascii_lowercase().ends_with("cli") {
        tail = trim_count_separators(&tail[..tail.len() - "cli".len()]);
    }
    let tail = trim_count_separators(tail.strip_suffix('个')?);
    trailing_digits(tail)
}

fn line_number(text: &str, pos: usize) -> usize {
    text[..pos].matches('\n').count() + 1
}

/// Every `<N> subcommands` / `<N> CLI subcommands` / `<N> 个 (CLI) 子命令`
/// mention in `text`, as `(line, digits)`.
fn prose_subcommand_counts(text: &str) -> Vec<(usize, String)> {
    // ASCII lowercasing preserves byte offsets, so positions found in the
    // lowercase copy are valid in `text`.
    let lower = text.to_ascii_lowercase();
    let mut mentions = Vec::new();
    for (needle, parse) in [
        (
            "subcommand",
            digits_before_english_subcommand as fn(&str) -> Option<String>,
        ),
        ("子命令", digits_before_chinese_subcommand),
    ] {
        let mut search_from = 0;
        while let Some(offset) = lower[search_from..].find(needle) {
            let pos = search_from + offset;
            if let Some(digits) = parse(&lower[..pos]) {
                mentions.push((line_number(text, pos), digits));
            }
            search_from = pos + needle.len();
        }
    }
    mentions.sort();
    mentions
}

#[test]
fn readme_command_table_and_count_mentions_match_the_clap_definition() {
    let (mut cli, aliases) = clap_subcommands();
    cli.sort();
    assert!(
        cli.len() >= 10,
        "--help parsing broke: only {} subcommands found",
        cli.len()
    );

    let readme = fs::read_to_string(repo_root().join("README.md")).expect("README.md readable");
    let mut table = readme_table_commands(&readme);
    table.sort();

    let missing_in_table: Vec<_> = cli.iter().filter(|c| !table.contains(*c)).collect();
    let unknown_to_cli: Vec<_> = table
        .iter()
        .filter(|c| !cli.contains(*c) && !aliases.contains(*c))
        .collect();
    assert!(
        missing_in_table.is_empty() && unknown_to_cli.is_empty(),
        "README command table drifted from clap:\n  missing from the table: {missing_in_table:?}\n  not a subcommand: {unknown_to_cli:?}"
    );

    let mut failures = Vec::new();
    for file in COUNT_MENTIONS {
        let text = fs::read_to_string(repo_root().join(file))
            .unwrap_or_else(|e| panic!("cannot read {file}: {e}"));
        let tail = &mut text.as_str();
        while let Some(pos) = tail.find("subcommand") {
            let prefix = &tail[..pos];
            // Walk back over the count phrase's separating characters, then
            // restore forward order before keeping digits.
            let window: String = prefix
                .chars()
                .rev()
                .take_while(|c| {
                    c.is_ascii_digit()
                        || *c == ' '
                        || *c == '\n'
                        || *c == '*'
                        || *c == 'I'
                        || *c == 'L'
                        || *c == 'C'
                })
                .collect();
            let digits: String = window
                .chars()
                .rev()
                .filter(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() && digits != cli.len().to_string() {
                failures.push(format!(
                    "{file}: says \"{digits} subcommands\", clap reports {}",
                    cli.len()
                ));
            }
            *tail = &tail[pos + "subcommand".len()..];
        }
    }
    assert!(
        failures.is_empty(),
        "stale subcommand counts in the docs:\n{}",
        failures.join("\n")
    );
}

#[test]
fn landing_page_command_cards_match_the_clap_definition() {
    let (mut cli, _aliases) = clap_subcommands();
    cli.sort();
    assert!(
        cli.len() >= 10,
        "--help parsing broke: only {} subcommands found",
        cli.len()
    );

    let html =
        fs::read_to_string(repo_root().join("docs/index.html")).expect("docs/index.html readable");
    let mut cards = landing_page_cards(&html);
    cards.sort();

    // Strict set equality — a missing card hides a subcommand from the
    // page's entry grid, an extra card links to nothing real, and a
    // duplicate double-links one command. Alias cards count as extras:
    // the canonical name's card already covers the command.
    assert_eq!(
        cards, cli,
        "landing-page CLI cards drifted from clap (missing = subcommands without a card, extra = cards that are not subcommands — aliases included)"
    );
}

/// Regression gate for the stale "33 subcommands" prose the release audit
/// found on the landing page: every count mention in README.md and
/// docs/index.html must equal the clap-derived count, and each file must
/// keep at least one mention so a prose rewrite cannot silently disable the
/// check.
#[test]
fn prose_subcommand_counts_in_readme_and_landing_page_match_the_clap_definition() {
    let (cli, _aliases) = clap_subcommands();
    assert!(
        cli.len() >= 10,
        "--help parsing broke: only {} subcommands found",
        cli.len()
    );

    let mut failures = Vec::new();
    for file in ["README.md", "docs/index.html"] {
        let text = fs::read_to_string(repo_root().join(file))
            .unwrap_or_else(|e| panic!("cannot read {file}: {e}"));
        let mentions = prose_subcommand_counts(&text);
        assert!(
            !mentions.is_empty(),
            "{file}: no \"<N> subcommands\" prose found — update this gate if the wording changed"
        );
        for (line, digits) in mentions {
            if digits != cli.len().to_string() {
                failures.push(format!(
                    "{file}:{line}: says \"{digits} subcommands\", clap reports {}",
                    cli.len()
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "stale subcommand counts in prose:\n{}",
        failures.join("\n")
    );
}
