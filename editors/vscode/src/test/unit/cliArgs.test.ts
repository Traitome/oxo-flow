import test from "node:test";
import assert from "node:assert/strict";
import {
  cleanArgs,
  dryRunArgs,
  formatArgs,
  graphArgs,
  lintArgs,
  resumeArgs,
  runArgs,
  schemaArgs,
  statusArgs,
  templateArgs,
  validateArgs,
} from "../../core/cliArgs";

test("validateArgs appends --json by default and supports --as-include", () => {
  assert.deepEqual(validateArgs("p.oxoflow"), ["validate", "p.oxoflow", "--json"]);
  assert.deepEqual(validateArgs("p.oxoflow", { asInclude: true }), [
    "validate",
    "p.oxoflow",
    "--as-include",
    "--json",
  ]);
  assert.deepEqual(validateArgs("p.oxoflow", { json: false }), ["validate", "p.oxoflow"]);
});

test("lintArgs supports --strict", () => {
  assert.deepEqual(lintArgs("p.oxoflow"), ["lint", "p.oxoflow", "--json"]);
  assert.deepEqual(lintArgs("p.oxoflow", { strict: true }), ["lint", "p.oxoflow", "--strict", "--json"]);
});

test("runArgs maps jobs/keepGoing/targets/extraArgs in stable order", () => {
  assert.deepEqual(runArgs({ file: "p.oxoflow" }), ["run", "p.oxoflow"]);
  assert.deepEqual(
    runArgs({ file: "p.oxoflow", jobs: 8, keepGoing: true, targets: ["results/a"], extraArgs: ["--profile", "slurm"] }),
    ["run", "p.oxoflow", "-j", "8", "-k", "-t", "results/a", "--profile", "slurm"]
  );
});

test("dryRunArgs shares the run surface without --json (terminal view)", () => {
  assert.deepEqual(dryRunArgs({ file: "p.oxoflow", jobs: 2 }), ["dry-run", "p.oxoflow", "-j", "2"]);
});

test("graph/resume/template/format/schema args", () => {
  assert.deepEqual(graphArgs("p.oxoflow"), ["graph", "p.oxoflow"]);
  assert.deepEqual(resumeArgs(".oxo-flow/checkpoint.json"), ["resume", ".oxo-flow/checkpoint.json"]);
  assert.deepEqual(resumeArgs("c.json", ["-k"]), ["resume", "c.json", "-k"]);
  assert.deepEqual(templateArgs("RNA-seq with STAR"), ["template", "RNA-seq with STAR", "--ai"]);
  assert.deepEqual(templateArgs("RNA-seq", "out.oxoflow"), ["template", "RNA-seq", "--ai", "-o", "out.oxoflow"]);
  assert.deepEqual(formatArgs("p.oxoflow"), ["format", "p.oxoflow"]);
  assert.deepEqual(formatArgs("p.oxoflow", "out.oxoflow"), ["format", "p.oxoflow", "-o", "out.oxoflow"]);
  assert.deepEqual(schemaArgs(), ["schema"]);
});

test("cleanArgs/statusArgs", () => {
  assert.deepEqual(cleanArgs("p.oxoflow", { dryRun: true }), ["clean", "p.oxoflow", "-n"]);
  assert.deepEqual(cleanArgs("p.oxoflow", { force: true }), ["clean", "p.oxoflow", "--force"]);
  assert.deepEqual(cleanArgs("p.oxoflow", { force: true, orphans: true }), [
    "clean",
    "p.oxoflow",
    "--force",
    "--orphans",
  ]);
  assert.deepEqual(statusArgs(), ["status"]);
  assert.deepEqual(statusArgs(".oxo-flow/checkpoint.json"), ["status", ".oxo-flow/checkpoint.json"]);
  assert.deepEqual(statusArgs(".oxo-flow/checkpoint.json", { timing: true }), [
    "status",
    ".oxo-flow/checkpoint.json",
    "--timing",
  ]);
});
