import test from "node:test";
import assert from "node:assert/strict";
import { ruleLensInfos, rulesUpTo } from "../../core/ruleIndex";

const DOC = `[workflow]
name = "demo"

[env_groups.rnaseq]
environment = { conda = "envs/rnaseq.yaml" }

[[rules]]
name = "fastqc"
input = ["data/{sample}.fastq.gz"]
shell = "fastqc {input[0]}"

[[rules]]
name = "trim"
depends_on = ["fastqc"]

[[rules]]
name = "multiqc"
depends_on = ["trim"]
`;

test("ruleLensInfos lists named rules in file order", () => {
  const infos = ruleLensInfos(DOC);
  assert.deepEqual(
    infos.map((r) => r.name),
    ["fastqc", "trim", "multiqc"]
  );
});

test("ruleLensInfos anchors headerLine at [[rules]] and nameLine at name =", () => {
  const infos = ruleLensInfos(DOC);
  const lines = DOC.split("\n");
  assert.match(lines[infos[0].headerLine], /^\[\[rules\]\]$/);
  assert.match(lines[infos[0].nameLine], /^\s*name = "fastqc"$/);
  assert.ok(infos[0].headerLine < infos[0].nameLine);
});

test("ruleLensInfos skips name keys outside [[rules]] blocks", () => {
  const doc = '[workflow]\nname = "not-a-rule"\n\n[[rules]]\nname = "real"\n';
  const infos = ruleLensInfos(doc);
  assert.deepEqual(infos.map((r) => r.name), ["real"]);
  assert.equal(infos[0].nameLine, 4);
});

test("rulesUpTo includes every rule at or before the named one", () => {
  const infos = ruleLensInfos(DOC);
  assert.deepEqual(rulesUpTo(infos, "fastqc"), ["fastqc"]);
  assert.deepEqual(rulesUpTo(infos, "trim"), ["fastqc", "trim"]);
  assert.deepEqual(rulesUpTo(infos, "multiqc"), ["fastqc", "trim", "multiqc"]);
  assert.deepEqual(rulesUpTo(infos, "nope"), []);
});

test("rules with unnamed blocks or duplicate names stay deterministic", () => {
  const doc = '[[rules]]\nshell = "x"\n[[rules]]\nname = "a"\n[[rules]]\nname = "a"\n';
  const infos = ruleLensInfos(doc);
  // Unnamed blocks produce no lens; duplicate names keep the first anchor.
  assert.deepEqual(infos.map((r) => r.nameLine), [3]);
  assert.deepEqual(rulesUpTo(infos, "a"), ["a"]);
});
