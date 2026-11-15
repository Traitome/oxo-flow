//! Execution-time placeholder gate (issue #1078 F2).
//!
//! Engine placeholders are a closed vocabulary: every one of them resolves
//! to a value for an executing rule. A token that survives rendering
//! therefore means the rule never got a value for it, and the shell is
//! about to receive the literal brace text — live: an unexpanded per-sample
//! rule ran TRUST4 with `{meta.bam_genomic}` as its BAM path, and the tool
//! failed on a file that never existed.
//!
//! The gate is deliberately conservative about what counts as a
//! placeholder:
//!
//! - `${...}` is shell parameter syntax, not engine syntax, and is left
//!   alone (F7: rule shells legitimately use `${#parts[@]}`,
//!   `${ch##*.clones_}`, `{ ...; }` groups).
//! - `\{...}` is an escaped literal brace.
//! - Bare tokens must match the engine's fan-out vocabulary
//!   ([`FAN_OUT_TOKENS`]); brace expansion and awk programs never match.
//! - Namespaced tokens must use one of the engine namespaces
//!   (`meta.`, `config.`, `params.`, `input`/`output`, `wildcard.`,
//!   `values.`).
//!
//! Callers strip shell comments before scanning (comments are prose, not
//! executed code).

/// Bare engine placeholders that must have a binding for every executing
/// rule. One list feeds both the render-time warning (so `dry-run`/`debug`
/// still surface the finding) and the execution-time gate; it cannot drift
/// the way hand-copied pairs did.
pub const FAN_OUT_TOKENS: &[&str] = &[
    "sample",
    "group",
    "pair_id",
    "experiment",
    "control",
    "tumor",
    "normal",
    "log",
    "threads",
    "memory",
    "effective_threads",
    "effective_memory_mb",
    "chunks",
];

/// Which engine namespace a surviving token belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceholderClass {
    /// A per-instance binding (`{sample}`, `{group}`, `{input…}`,
    /// `{wildcard.*}`, `{values.*}`, `{log}`, `{threads}`, …). A survivor
    /// means the rule never expanded or the token was never declared.
    FanOut,
    /// `{meta.<column>}` with no metadata row bound. A one-row metadata
    /// table resolves before this point (the supported constant read); a
    /// multi-row table with no sample-like binding leaves the token here.
    Meta,
    /// `{input…}` / `{output…}` namespace token left unsubstituted.
    Io,
    /// `{config.<key>}` whose key has no value.
    Config,
    /// `{params.<key>}` the rule does not declare.
    Params,
}

impl PlaceholderClass {
    /// Stable label for diagnostics and tests.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FanOut => "fan-out",
            Self::Meta => "meta",
            Self::Io => "input/output",
            Self::Config => "config",
            Self::Params => "params",
        }
    }
}

/// One engine placeholder that reached the rendered command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StalePlaceholder {
    pub class: PlaceholderClass,
    /// The token exactly as the shell would receive it, braces included
    /// (e.g. `{meta.bam_genomic}`).
    pub token: String,
}

/// Scan one rendered command text for surviving engine placeholders.
///
/// The input must already have shell comments stripped
/// ([`crate::wildcard::strip_shell_comments`]). First-appearance order is
/// preserved and duplicate tokens are reported once.
#[must_use]
pub fn stale_placeholders(text: &str) -> Vec<StalePlaceholder> {
    let mut found: Vec<StalePlaceholder> = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = text[cursor..].find('{') {
        let start = cursor + offset;
        // `${...}` is shell parameter syntax; `\{...}` is an escaped
        // literal. Skip the whole brace group so its interior is not
        // scanned as engine syntax.
        if let Some(prev) = text[..start].chars().next_back()
            && matches!(prev, '$' | '\\')
        {
            cursor = match text[start + 1..].find('}') {
                Some(end) => start + 1 + end + 1,
                None => break,
            };
            continue;
        }
        let Some(end) = text[start + 1..].find('}') else {
            break;
        };
        let inner = &text[start + 1..start + 1 + end];
        if let Some(class) = classify(inner) {
            let token = format!("{{{inner}}}");
            if !found.iter().any(|p| p.token == token) {
                found.push(StalePlaceholder { class, token });
            }
        } else if inner.contains('{') && !inner.starts_with('{') {
            // A literal/outer brace with another opening brace before the
            // next `}` (a quoted `'{'`, a nested group): do not swallow the
            // nested token — advance one char and keep scanning (issue
            // #1078 R6). A doubled `{{...}}` is the host-language escape
            // (Python f-string `{{sample}}`), so it stays untouched.
            cursor = start + 1;
            continue;
        }
        cursor = start + 1 + end + 1;
    }
    found
}

/// Classify one brace body, or `None` when it is not engine syntax.
fn classify(inner: &str) -> Option<PlaceholderClass> {
    if FAN_OUT_TOKENS.contains(&inner) {
        return Some(PlaceholderClass::FanOut);
    }
    // The log renderer substitutes the snakemake-style indexed form too.
    if inner.starts_with("log[") {
        return Some(PlaceholderClass::FanOut);
    }
    // Namespaced forms. `input_group` must be checked before `input`.
    let (namespace, key) = inner.split_once(['.', '['])?;
    if key.is_empty() && !inner.contains('[') {
        return None;
    }
    match namespace {
        "input_group" => Some(PlaceholderClass::Io),
        "input" | "output" => Some(PlaceholderClass::Io),
        "config" => Some(PlaceholderClass::Config),
        "params" => Some(PlaceholderClass::Params),
        "meta" => Some(PlaceholderClass::Meta),
        "wildcard" | "values" => Some(PlaceholderClass::FanOut),
        _ => None,
    }
}

/// The gate's verdict for one rule.
#[derive(Debug, Default)]
pub struct GateOutcome {
    /// Every surviving token, first-appearance order.
    pub survivors: Vec<StalePlaceholder>,
    /// Survivors that must fail the rule.
    pub fatal: Vec<StalePlaceholder>,
}

impl GateOutcome {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.survivors.is_empty()
    }

    /// `{token}, {token}` — the rendered list used by the error/warn text.
    #[must_use]
    pub fn tokens(&self, list: &[StalePlaceholder]) -> String {
        list.iter()
            .map(|p| p.token.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Evaluate the gate over the rendered command texts of one rule.
///
/// `enabled` comes from [`crate::config::resolve_placeholder_gate`]; when
/// disabled every survivor is a warning (the historical behavior, emitted
/// by the render-time scan). When enabled, a survivor of ANY class fails
/// the rule — `singleton` only softens the static W041 diagnosis and opts
/// out of meta-implied fan-out; it never lets a literal engine token reach
/// a shell (issue #1078 F2/H1). A one-row metadata table resolves
/// `{meta.<column>}` before this point, which is the supported constant
/// read.
#[must_use]
pub fn evaluate<'a>(texts: impl IntoIterator<Item = &'a str>, enabled: bool) -> GateOutcome {
    let mut outcome = GateOutcome::default();
    for text in texts {
        for stale in stale_placeholders(text) {
            if !outcome.survivors.iter().any(|p| p.token == stale.token) {
                outcome.survivors.push(stale);
            }
        }
    }
    if enabled {
        outcome.fatal = outcome.survivors.clone();
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(text: &str) -> Vec<(PlaceholderClass, String)> {
        stale_placeholders(text)
            .into_iter()
            .map(|p| (p.class, p.token))
            .collect()
    }

    #[test]
    fn flags_engine_fan_out_tokens() {
        assert_eq!(
            classes("tool --sample {sample} --threads {threads} {input[0]}"),
            vec![
                (PlaceholderClass::FanOut, "{sample}".to_string()),
                (PlaceholderClass::FanOut, "{threads}".to_string()),
                (PlaceholderClass::Io, "{input[0]}".to_string()),
            ]
        );
    }

    #[test]
    fn flags_namespaced_placeholders() {
        assert_eq!(
            classes("x={meta.batch} y={config.threads} z={params.q} v={wildcard.read}"),
            vec![
                (PlaceholderClass::Meta, "{meta.batch}".to_string()),
                (PlaceholderClass::Config, "{config.threads}".to_string()),
                (PlaceholderClass::Params, "{params.q}".to_string()),
                (PlaceholderClass::FanOut, "{wildcard.read}".to_string()),
            ]
        );
    }

    #[test]
    fn flags_the_indexed_log_form() {
        // The renderer substitutes `{log[0]}` too; a survivor must not slip
        // through the classifier (issue #1078 L2).
        assert_eq!(
            classes("cmd 2> {log[0]}"),
            vec![(PlaceholderClass::FanOut, "{log[0]}".to_string())]
        );
    }

    #[test]
    fn shell_parameter_expansion_is_not_a_placeholder() {
        // F7: `${...}` is the shell's own grammar. The gate must never
        // turn a working bash construct into a run-killing error. The body
        // is shell text by contract, even when it contains a brace group
        // (`${x:-{sample}}` is a default value, not an engine token).
        let text = "echo ${#parts[@]} ${ch##*.clones_} ${sample} ${meta.id} \
                    ${config[x]} ${x:-{sample}}";
        assert!(classes(text).is_empty(), "{:?}", classes(text));
    }

    #[test]
    fn escaped_and_braced_literals_are_not_placeholders() {
        assert!(classes(r"echo \{sample\}").is_empty());
        assert!(classes("bs={1..10}; echo {a,b}; awk '{print $1}'").is_empty());
        assert!(classes("echo {{meta.x}}").is_empty());
    }

    #[test]
    fn brace_groups_and_shell_blocks_are_not_placeholders() {
        assert!(classes("{ echo hi; } { sample; }").is_empty());
    }

    #[test]
    fn dedupes_repeated_tokens() {
        assert_eq!(
            classes("{sample} {sample} {meta.x} {meta.x}").len(),
            2,
            "each distinct token is reported once"
        );
    }

    #[test]
    fn literal_brace_before_a_token_does_not_hide_it() {
        // A quoted `'{'` (or a nested group) must not swallow the next
        // engine token (issue #1078 R6).
        assert_eq!(
            classes("echo '{' ; echo {sample}"),
            vec![(PlaceholderClass::FanOut, "{sample}".to_string())]
        );
        assert_eq!(
            classes("{ echo {meta.x}; }"),
            vec![(PlaceholderClass::Meta, "{meta.x}".to_string())]
        );
    }

    #[test]
    fn unterminated_brace_does_not_panic() {
        assert!(classes("echo {sample").is_empty());
    }

    #[test]
    fn enabled_gate_makes_every_survivor_fatal() {
        let outcome = evaluate(["echo {meta.x} {sample}"], true);
        assert_eq!(outcome.survivors.len(), 2);
        assert_eq!(
            outcome.fatal.len(),
            2,
            "singleton only softens the static diagnosis, never the runtime gate"
        );
    }

    #[test]
    fn disabled_gate_reports_no_fatal_survivors() {
        let outcome = evaluate(["echo {sample}"], false);
        assert_eq!(outcome.survivors.len(), 1);
        assert!(outcome.fatal.is_empty());
    }
}
