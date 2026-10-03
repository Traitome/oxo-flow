/**
 * Parsers for the machine-readable (`--json`) reports of `oxo-flow validate`
 * and `oxo-flow lint`. The shapes mirror the serde_json documents produced by
 * `crates/oxo-flow-cli/src/commands/quality.rs` — keep in sync with that file
 * (the unit tests pin the field names).
 */

export type Severity = "error" | "warning" | "info";

/** {code, message, rule, suggestion} — validate errors and lint diagnostics. */
export interface RawDiagnostic {
  code: string;
  message: string;
  rule: string | null;
  suggestion: string | null;
}

export interface ValidateReport {
  command: "validate";
  workflow: string;
  valid: boolean;
  rules: number;
  dependencies: number;
  errors: RawDiagnostic[];
  missing_inputs: string[];
}

export interface LintReport {
  command: "lint";
  workflow: string;
  strict: boolean;
  diagnostics: (RawDiagnostic & { severity: Severity })[];
  error_count: number;
  warning_count: number;
  info_count: number;
  passed: boolean;
}

export type Report = ValidateReport | LintReport;

/** Thrown when the CLI output is not the expected JSON document. */
export class ReportParseError extends Error {
  constructor(message: string, public override readonly cause?: unknown) {
    super(message);
    this.name = "ReportParseError";
  }
}
export function parseReport(stdout: string): Report {
  const start = stdout.indexOf("{");
  if (start < 0) {
    throw new ReportParseError("CLI produced no JSON document on stdout");
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(stdout.slice(start));
  } catch (e) {
    throw new ReportParseError("CLI output is not valid JSON", e);
  }
  if (typeof parsed !== "object" || parsed === null) {
    throw new ReportParseError("CLI JSON is not an object");
  }
  const command = (parsed as { command?: unknown }).command;
  if (command === "validate") return parsed as ValidateReport;
  if (command === "lint") return parsed as LintReport;
  throw new ReportParseError(`unexpected CLI report command: ${String(command)}`);
}

/** Normalized finding positioned later by the diagnostics provider. */
export interface Finding {
  severity: Severity;
  code: string;
  message: string;
  rule: string | null;
  suggestion: string | null;
  source: "validate" | "lint";
}

export function findingsFromValidate(r: ValidateReport): Finding[] {
  const findings: Finding[] = r.errors.map((e) => ({
    severity: "error" as const,
    code: e.code,
    message: e.message,
    rule: e.rule ?? null,
    suggestion: e.suggestion ?? null,
    source: "validate" as const,
  }));
  for (const input of r.missing_inputs ?? []) {
    findings.push({
      severity: "warning",
      code: "MISSING_INPUT",
      message: `input file does not exist: ${input}`,
      rule: null,
      suggestion: "Create the file or fix the path (paths resolve relative to the pipeline's directory).",
      source: "validate",
    });
  }
  return findings;
}

export function findingsFromLint(r: LintReport): Finding[] {
  return r.diagnostics.map((d) => ({
    severity: d.severity,
    code: d.code,
    message: d.message,
    rule: d.rule ?? null,
    suggestion: d.suggestion ?? null,
    source: "lint" as const,
  }));
}

/**
 * Validate and lint intentionally overlap (lint re-runs the format checks), so
 * running both produces duplicates. Two findings are the same when code,
 * message and rule agree — the marketplace cannot merge them, so we do.
 */
export function dedupeFindings(findings: Finding[]): Finding[] {
  const seen = new Set<string>();
  const out: Finding[] = [];
  for (const f of findings) {
    const key = `${f.source}\u0000${f.code}\u0000${f.message}\u0000${f.rule ?? ""}`;
    if (!seen.has(key)) {
      seen.add(key);
      out.push(f);
    }
  }
  return out;
}
