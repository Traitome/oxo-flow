//! Terminal banner rendered at the top of `oxo-flow --help`.

/// Banner for the top-level `-h`/`--help`: ANSI Shadow "oxo-flow" with a
/// per-letter cyan-to-green gradient, then the version and repository URL.
///
/// The banner is embedded verbatim in the clap help template (replacing
/// the default `{name} {version}` header). `main.rs` picks this colored
/// variant only when stdout is a terminal and `NO_COLOR`/`--no-color` is
/// not set; otherwise the plain variant below is used, so piped output
/// and scripts never see escape codes.
pub const HELP_TEMPLATE: &str = concat!(
    "\x1b[38;2;0;206;255m ██████╗  \x1b[0m \x1b[38;2;0;199;227m██╗  ██╗ \x1b[0m \x1b[38;2;0;193;199m ██████╗  \x1b[0m \x1b[38;2;0;186;171m███████╗ \x1b[0m \x1b[38;2;0;180;143m██╗      \x1b[0m \x1b[38;2;0;173;115m ██████╗  \x1b[0m \x1b[38;2;0;167;87m██╗    ██╗\x1b[0m\n\x1b[38;2;0;206;255m██╔═══██╗ \x1b[0m \x1b[38;2;0;199;227m╚██╗██╔╝ \x1b[0m \x1b[38;2;0;193;199m██╔═══██╗ \x1b[0m \x1b[38;2;0;186;171m██╔════╝ \x1b[0m \x1b[38;2;0;180;143m██║      \x1b[0m \x1b[38;2;0;173;115m██╔═══██╗ \x1b[0m \x1b[38;2;0;167;87m██║    ██║\x1b[0m\n\x1b[38;2;0;206;255m██║   ██║ \x1b[0m \x1b[38;2;0;199;227m ╚███╔╝  \x1b[0m \x1b[38;2;0;193;199m██║   ██║ \x1b[0m \x1b[38;2;0;186;171m█████╗   \x1b[0m \x1b[38;2;0;180;143m██║      \x1b[0m \x1b[38;2;0;173;115m██║   ██║ \x1b[0m \x1b[38;2;0;167;87m██║ █╗ ██║\x1b[0m\n\x1b[38;2;0;206;255m██║   ██║ \x1b[0m \x1b[38;2;0;199;227m ██╔██╗  \x1b[0m \x1b[38;2;0;193;199m██║   ██║ \x1b[0m \x1b[38;2;0;186;171m██╔══╝   \x1b[0m \x1b[38;2;0;180;143m██║      \x1b[0m \x1b[38;2;0;173;115m██║   ██║ \x1b[0m \x1b[38;2;0;167;87m██║███╗██║\x1b[0m\n\x1b[38;2;0;206;255m╚██████╔╝ \x1b[0m \x1b[38;2;0;199;227m██╔╝ ██╗ \x1b[0m \x1b[38;2;0;193;199m╚██████╔╝ \x1b[0m \x1b[38;2;0;186;171m██║      \x1b[0m \x1b[38;2;0;180;143m███████╗ \x1b[0m \x1b[38;2;0;173;115m╚██████╔╝ \x1b[0m \x1b[38;2;0;167;87m╚███╔███╔╝\x1b[0m\n\x1b[38;2;0;206;255m ╚═════╝  \x1b[0m \x1b[38;2;0;199;227m╚═╝  ╚═╝ \x1b[0m \x1b[38;2;0;193;199m ╚═════╝  \x1b[0m \x1b[38;2;0;186;171m╚═╝      \x1b[0m \x1b[38;2;0;180;143m╚══════╝ \x1b[0m \x1b[38;2;0;173;115m ╚═════╝  \x1b[0m \x1b[38;2;0;167;87m ╚══╝╚══╝ \x1b[0m",
    "\n\n",
    "\x1b[1;37moxo-flow v",
    env!("CARGO_PKG_VERSION"),
    "\x1b[0m",
    "\x1b[38;5;245m — Rust-native bioinformatics pipeline engine\x1b[0m\n",
    "\x1b[38;2;0;206;255m",
    env!("CARGO_PKG_REPOSITORY"),
    "\x1b[0m\n\n",
    "{author-with-newline}{about-with-newline}\n{usage-heading} {usage}\n\n{all-args}{after-help}",
);

/// Plain-text variant of [`HELP_TEMPLATE`] without ANSI escapes.
pub const HELP_TEMPLATE_PLAIN: &str = concat!(
    " ██████╗   ██╗  ██╗   ██████╗   ███████╗  ██╗        ██████╗   ██╗    ██╗\n██╔═══██╗  ╚██╗██╔╝  ██╔═══██╗  ██╔════╝  ██║       ██╔═══██╗  ██║    ██║\n██║   ██║   ╚███╔╝   ██║   ██║  █████╗    ██║       ██║   ██║  ██║ █╗ ██║\n██║   ██║   ██╔██╗   ██║   ██║  ██╔══╝    ██║       ██║   ██║  ██║███╗██║\n╚██████╔╝  ██╔╝ ██╗  ╚██████╔╝  ██║       ███████╗  ╚██████╔╝  ╚███╔███╔╝\n ╚═════╝   ╚═╝  ╚═╝   ╚═════╝   ╚═╝       ╚══════╝   ╚═════╝    ╚══╝╚══╝ ",
    "\n\n",
    "oxo-flow v",
    env!("CARGO_PKG_VERSION"),
    " — Rust-native bioinformatics pipeline engine\n",
    env!("CARGO_PKG_REPOSITORY"),
    "\n\n",
    "{author-with-newline}{about-with-newline}\n{usage-heading} {usage}\n\n{all-args}{after-help}",
);
#[cfg(test)]
mod tests {
    use super::*;

    /// Remove ANSI CSI escape sequences so tests can assert on the text.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for skip in chars.by_ref() {
                    if skip == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn banner_lines_are_equally_wide() {
        let plain = strip_ansi(HELP_TEMPLATE);
        let lines: Vec<&str> = plain.lines().take(6).collect();
        assert_eq!(lines.len(), 6, "banner art should have 6 rows");
        let width = lines[0].chars().count();
        for line in &lines {
            assert_eq!(
                line.chars().count(),
                width,
                "art row is misaligned: {line:?}"
            );
        }
    }

    #[test]
    fn stripped_colored_equals_plain_variant() {
        assert_eq!(strip_ansi(HELP_TEMPLATE), HELP_TEMPLATE_PLAIN);
    }

    #[test]
    fn banner_carries_version_and_repository() {
        let plain = strip_ansi(HELP_TEMPLATE);
        assert!(plain.contains(env!("CARGO_PKG_VERSION")));
        assert!(plain.contains("https://github.com/Traitome/oxo-flow"));
    }

    #[test]
    fn plain_variant_has_no_ansi_escapes() {
        assert!(!HELP_TEMPLATE_PLAIN.contains("\x1b["));
        assert!(HELP_TEMPLATE.contains("\x1b[38;2;"));
    }
}
