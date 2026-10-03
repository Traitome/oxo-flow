import test from "node:test";
import assert from "node:assert/strict";
import { resolveEditContext } from "../../core/editContext";

const DOC = [
  "# comment",
  "[workflow]",
  "name = ",
  "",
  "[[rules]]",
  "name = \"qc\"",
  "depends_on = [",
  "    \"trim\",",
  "]",
  "environment = { conda ",
  "shell = '''",
  "# not a comment",
  "'''",
].join("\n");

test("comment lines disable completion", () => {
  const { ctx } = resolveEditContext(DOC, 0, 3);
  assert.equal(ctx.kind, "comment");
});

test("top-level header typing", () => {
  const { ctx } = resolveEditContext("[work", 0, 5);
  assert.equal(ctx.kind, "header");
  if (ctx.kind === "header") assert.equal(ctx.typed, "work");
});

test("value context inside [workflow] carries the section", () => {
  const { ctx } = resolveEditContext(DOC, 2, 8); // after `name = `
  assert.equal(ctx.kind, "value");
  if (ctx.kind === "value") {
    assert.equal(ctx.key, "name");
    assert.equal(ctx.context, "workflow");
  }
});

test("key context before = inside [[rules]]", () => {
  const line = DOC.split("\n").findIndex((l) => l === 'name = "qc"');
  const { ctx } = resolveEditContext(DOC, line, 0);
  assert.equal(ctx.kind, "key");
  if (ctx.kind === "key") assert.equal(ctx.context, "rules");
});

test("multi-line array continuation keeps the key", () => {
  const { ctx } = resolveEditContext(DOC, 7, 10); // the "trim", line
  assert.equal(ctx.kind, "value");
  if (ctx.kind === "value") assert.equal(ctx.key, "depends_on");
});

test("inline table open on the line completes table keys", () => {
  const line = DOC.split("\n").findIndex((l) => l.includes("environment = { conda "));
  const { ctx } = resolveEditContext(DOC, line, DOC.split("\n")[line].length);
  assert.equal(ctx.kind, "inline-table-key");
  if (ctx.kind === "inline-table-key") {
    assert.equal(ctx.tableKey, "environment");
    assert.deepEqual(ctx.usedKeys, []);
  }
});

test("multi-line strings hide completion", () => {
  const line = DOC.split("\n").findIndex((l) => l === "# not a comment");
  const { ctx } = resolveEditContext(DOC, line, 5);
  assert.equal(ctx.kind, "multiline-string");
});
