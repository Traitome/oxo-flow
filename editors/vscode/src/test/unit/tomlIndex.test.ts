import test from "node:test";
import assert from "node:assert/strict";
import { indexToml, stripComment } from "../../core/tomlIndex";

const DOC = `# pipeline header comment
[workflow]
name = "demo"
version = "1.0.0"

[config]
[config.mode]
type = "string"
default = "fast"

[wildcard_constraints]
sample = "[A-Za-z0-9_-]+"

[env_groups.rnaseq]
environment = { conda = "envs/rnaseq.yaml" }

[[rules]]
name = "fastqc"
input = ["data/{sample}.fastq.gz"]
shell = "fastqc {input[0]}"

[[rules]]
name = "multiqc"
depends_on = [
    "fastqc",
]
shell = "multiqc ."
`;

test("indexToml finds sections, rule names and domain references", () => {
  const idx = indexToml(DOC);
  assert.deepEqual(
    idx.sections.map((s) => `${s.kind}:${s.name}`),
    [
      "table:workflow",
      "table:config",
      "table:config.mode",
      "table:wildcard_constraints",
      "table:env_groups.rnaseq",
      "array-of-tables:rules",
      "array-of-tables:rules",
    ]
  );
  assert.deepEqual([...idx.ruleNameLines.keys()].sort(), ["fastqc", "multiqc"]);
  assert.equal(idx.lineOfRuleName("fastqc"), 17);
  assert.equal(idx.lineOfRuleName("multiqc"), 22);
  assert.equal(idx.lineOfRuleName("nope"), null);
  assert.deepEqual(idx.envGroupNames, ["rnaseq"]);
  assert.deepEqual(idx.wildcardNames, ["sample"]);
});

test("rule-name anchors point at the name = line (diagnostic anchoring)", () => {
  const idx = indexToml(DOC);
  const line = idx.lineOfRuleName("fastqc")!;
  assert.match(DOC.split("\n")[line], /^\s*name = "fastqc"$/);
});

test("hashes inside multi-line strings are not comments", () => {
  const doc = [
    "[[rules]]",
    'name = "scripted"',
    "script = '''",
    "# this is a comment inside the script",
    "echo done",
    "'''",
  ].join("\n");
  const idx = indexToml(doc);
  assert.equal(idx.lineOfRuleName("scripted"), 1);
  // The script line must stay part of the key segment, not a new section.
  assert.equal(idx.sections.length, 1);
  const seg = idx.segmentAt(3)!;
  assert.equal(seg.key, "script");
  assert.equal(seg.endLine, 5);
});

test("multi-line arrays keep the owning key segment", () => {
  const idx = indexToml(DOC);
  const seg = idx.segmentAt(24); // the "fastqc", element line inside depends_on
  assert.ok(seg, "expected a segment for the array continuation line");
  assert.equal(seg.key, "depends_on");
  assert.equal(seg.endLine, 25); // the closing bracket line
});

test("segmentAt returns null outside any key", () => {
  const idx = indexToml(DOC);
  assert.equal(idx.segmentAt(0), null, "header comment line");
});

test("sectionBefore maps lines to the latest header", () => {
  const idx = indexToml(DOC);
  assert.equal(idx.sectionBefore(0), null);
  assert.equal(idx.sectionBefore(2)?.name, "workflow");
  assert.equal(idx.sectionBefore(18)?.name, "rules");
});

test("stripComment removes trailing comments but not hashes in strings", () => {
  assert.equal(stripComment('shell = "echo #1" # trailing'), 'shell = "echo #1" ');
  assert.equal(stripComment("a = 'lit # eral'"), "a = 'lit # eral'");
  assert.equal(stripComment('basic "with \\" escaped" # c'), 'basic "with \\" escaped" ');
  assert.equal(stripComment("plain # comment"), "plain ");
});

test("duplicate rule names keep the first anchor", () => {
  const doc = '[[rules]]\nname = "a"\n[[rules]]\nname = "a"\n';
  const idx = indexToml(doc);
  assert.equal(idx.lineOfRuleName("a"), 1);
});
