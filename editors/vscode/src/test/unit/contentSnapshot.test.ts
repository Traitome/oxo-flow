import test from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import * as path from "node:path";
import { snapshotPathFor, withContentSnapshot } from "../../core/contentSnapshot";

const ANCHOR = path.join("/tmp", "project", "pipeline.oxoflow");

test("snapshotPathFor stays beside the anchor with an .oxoflow suffix", () => {
  const p = snapshotPathFor(ANCHOR, "unit");
  assert.equal(path.dirname(p), path.dirname(ANCHOR));
  assert.ok(p.endsWith(".oxoflow"), p);
  assert.ok(path.basename(p).startsWith(".oxo-flow-snapshot-"), p);
});

test("snapshotPathFor yields distinct paths when no token is supplied", () => {
  assert.notEqual(snapshotPathFor(ANCHOR), snapshotPathFor(ANCHOR));
});

test("withContentSnapshot exposes the exact buffer content and cleans up", async () => {
  const dir = await mkdtemp("/tmp/oxo-snapshot-test-");
  const anchor = path.join(dir, "demo.oxoflow");
  const content = '[workflow]\nname = "unsaved-buffer"\n';
  let seenPath = "";
  let seen = "";

  await withContentSnapshot(anchor, content, async (snapshot) => {
    seenPath = snapshot;
    assert.equal(path.dirname(snapshot), dir);
    assert.ok(existsSync(snapshot));
    seen = await readFile(snapshot, "utf8");
  });

  assert.equal(seen, content);
  assert.equal(existsSync(seenPath), false, "snapshot must be deleted");
  await rm(dir, { recursive: true, force: true });
});

test("withContentSnapshot deletes the snapshot when the body throws", async () => {
  const dir = await mkdtemp("/tmp/oxo-snapshot-test-");
  const anchor = path.join(dir, "demo.oxoflow");
  let seenPath = "";

  await assert.rejects(
    withContentSnapshot(anchor, "[workflow]\n", async (snapshot) => {
      seenPath = snapshot;
      throw new Error("boom");
    }),
    /boom/
  );
  assert.equal(existsSync(seenPath), false, "snapshot must be deleted on failure");
  await rm(dir, { recursive: true, force: true });
});
