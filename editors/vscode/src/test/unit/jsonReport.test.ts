import test from "node:test";
import assert from "node:assert/strict";
import {
  dedupeFindings,
  findingsFromLint,
  findingsFromValidate,
  parseReport,
  ReportParseError,
  type LintReport,
  type ValidateReport,
} from "../../core/jsonReport";

// Shapes mirror crates/oxo-flow-cli/src/commands/quality.rs — if the CLI
// changes these documents, the unit test failure names the drift.
const VALIDATE_JSON = JSON.stringify({
  command: "validate",
  workflow: "p.oxoflow",
  valid: false,
  rules: 3,
  dependencies: 2,
  errors: [
    { code: "E001", message: "unknown key 'environmen'", rule: "align", suggestion: "did you mean 'environment'?" },
    { code: "PARSE", message: "TOML parse error at line 4", rule: null, suggestion: null },
  ],
  missing_inputs: ["data/missing.fastq.gz"],
} satisfies ValidateReport);

const LINT_JSON = JSON.stringify({
  command: "lint",
  workflow: "p.oxoflow",
  strict: false,
  diagnostics: [
    { severity: "error", code: "E002", message: "rule has no output", rule: "qc", suggestion: null },
    { severity: "warning", code: "W001", message: "rule without threads", rule: "qc", suggestion: "set threads" },
    { severity: "info", code: "S001", message: "consider a description", rule: null, suggestion: null },
  ],
  error_count: 1,
  warning_count: 1,
  info_count: 1,
  passed: false,
} satisfies LintReport);

function validateFixture(): ValidateReport {
  const r = parseReport(VALIDATE_JSON);
  if (r.command !== "validate") throw new Error("fixture changed: expected a validate report");
  return r;
}

function lintFixture(): LintReport {
  const r = parseReport(LINT_JSON);
  if (r.command !== "lint") throw new Error("fixture changed: expected a lint report");
  return r;
}

test("parseReport accepts validate documents", () => {
  const r = validateFixture();
  assert.equal(r.valid, false);
  assert.equal(r.errors.length, 2);
});

test("parseReport accepts lint documents", () => {
  const r = lintFixture();
  assert.equal(r.passed, false);
  assert.equal(r.diagnostics.length, 3);
});

test("parseReport tolerates leading non-JSON output (banners, warnings)", () => {
  const r = parseReport(`some banner\nlog line\n${VALIDATE_JSON}`);
  assert.equal(r.command, "validate");
});

test("parseReport rejects garbage and unknown commands", () => {
  assert.throws(() => parseReport("no json here"), ReportParseError);
  assert.throws(() => parseReport('{"command": "mystery"}'), ReportParseError);
  assert.throws(() => parseReport("[1,2,3]"), ReportParseError);
});

test("findingsFromValidate maps errors and missing inputs", () => {
  const findings = findingsFromValidate(validateFixture());
  assert.equal(findings.length, 3);
  assert.deepEqual(
    findings.map((f) => f.severity),
    ["error", "error", "warning"]
  );
  assert.equal(findings[0].source, "validate");
  assert.equal(findings[2].code, "MISSING_INPUT");
});

test("findingsFromLint maps severities 1:1", () => {
  const findings = findingsFromLint(lintFixture());
  assert.deepEqual(
    findings.map((f) => f.severity),
    ["error", "warning", "info"]
  );
  assert.equal(findings[1].rule, "qc");
});

test("dedupeFindings keeps validate/lint overlap once", () => {
  const validate = findingsFromValidate(validateFixture());
  const lint = findingsFromLint(lintFixture());
  const merged = dedupeFindings([...validate, ...lint, ...lint]);
  assert.equal(merged.length, validate.length + lint.length);
});
