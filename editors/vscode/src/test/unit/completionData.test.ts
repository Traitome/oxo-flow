import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { completionData, contextKeys, contextNames, enumFor } from "../../core/completionData";

const extRoot = join(__dirname, "..", "..", "..");
const repoRoot = join(extRoot, "..", "..");
const schemaPath = join(repoRoot, "docs", "schema", "oxoflow-v1.schema.json");

test("completion data provenance: schema hash matches the canonical copy", () => {
  if (!existsSync(schemaPath)) {
    assert.ok(completionData.schemaSha256, "packaged data still carries a provenance hash");
    return;
  }
  const actual = createHash("sha256").update(readFileSync(schemaPath)).digest("hex");
  assert.equal(completionData.schemaSha256, actual, "generated data is stale — run `npm run generate`");
});

test("core contexts exist and are populated", () => {
  const names = contextNames();
  for (const required of ["root", "workflow", "rules"]) {
    assert.ok(names.includes(required), `missing context ${required}`);
    assert.ok(contextKeys(required).length > 0);
  }
  assert.ok(names.includes("rules.environment"), "environmentSpec sub-table context");
});

test("rule properties cover the essentials", () => {
  const keys = contextKeys("rules").map((k) => k.key);
  for (const key of ["name", "input", "output", "shell", "threads", "memory", "depends_on", "environment"]) {
    assert.ok(keys.includes(key), `rules context missing ${key}`);
  }
});

test("enums resolve from the schema", () => {
  assert.deepEqual(enumFor("rules", "checksum"), ["md5", "sha256"]);
  const backend = enumFor("cluster", "backend");
  assert.ok(backend);
  for (const b of ["slurm", "pbs", "sge", "lsf"]) assert.ok(backend!.includes(b));
  assert.equal(enumFor("rules", "shell"), null, "free-form keys have no enums");
});

test("every context has unique, non-empty keys and string descriptions", () => {
  for (const name of contextNames()) {
    const seen = new Set<string>();
    for (const info of contextKeys(name)) {
      assert.ok(info.key.length > 0, `${name}: empty key`);
      assert.ok(!seen.has(info.key), `${name}: duplicate key ${info.key}`);
      seen.add(info.key);
      assert.equal(typeof info.description, "string");
    }
  }
});
