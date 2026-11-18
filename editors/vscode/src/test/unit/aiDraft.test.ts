import test from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import * as path from "node:path";
import {
  generateValidatedDraft,
  validateDraftContent,
  type DraftChatTurn,
  type DraftRunner,
} from "../../core/aiDraft";
import type { CliResult } from "../../core/exec";

const VALID = '[workflow]\nname = "generated"\n\n[[rules]]\nname = "r1"\nshell = "true"\n';
const REPAIRED = '[workflow]\nname = "repaired"\n\n[[rules]]\nname = "r1"\nshell = "true"\n';

function cli(exitCode: number, stderr = "", stdout = ""): CliResult {
  return { stdout, stderr, exitCode };
}

/** Chat stub that replays canned model responses in order. */
function scriptedChat(responses: string[]): {
  chat: (turns: readonly DraftChatTurn[]) => Promise<string>;
  calls: DraftChatTurn[][];
} {
  const calls: DraftChatTurn[][] = [];
  return {
    calls,
    chat: async (turns) => {
      calls.push([...turns]);
      const next = responses.shift();
      if (next === undefined) throw new Error("scripted chat ran out of responses");
      return next;
    },
  };
}

async function tmpDir(): Promise<string> {
  return mkdtemp("/tmp/oxo-ai-draft-test-");
}

test("a passing first draft is validated on a snapshot BEFORE apply", async () => {
  const dir = await tmpDir();
  const targetPath = path.join(dir, "out.oxoflow");
  const applied: string[] = [];
  let snapshotAtRun = "";
  let contentAtRun = "";

  const run: DraftRunner = async (_executable, args) => {
    snapshotAtRun = args[1];
    // The snapshot — not the destination — is what reaches the CLI.
    assert.notEqual(snapshotAtRun, targetPath);
    assert.equal(path.dirname(snapshotAtRun), dir);
    assert.ok(snapshotAtRun.endsWith(".oxoflow"));
    assert.equal(existsSync(targetPath), false, "target must not exist before validation passes");
    contentAtRun = await readFile(snapshotAtRun, "utf8");
    return cli(0, "", '{"command":"validate","valid":true}');
  };

  const { chat, calls } = scriptedChat(["```toml\n" + VALID + "```"]);
  const outcome = await generateValidatedDraft("make a demo", {
    targetPath,
    executable: "oxo-flow",
    cwd: dir,
    chat,
    run,
    apply: async (toml) => {
      applied.push(toml);
    },
  });

  assert.deepEqual(outcome, { status: "applied", toml: VALID.trim(), attempts: 1 });
  assert.equal(calls.length, 1);
  assert.equal(contentAtRun, VALID.trim(), "CLI must receive the draft content");
  assert.deepEqual(applied, [VALID.trim()]);
  assert.equal(existsSync(snapshotAtRun), false, "validation snapshot must be removed");
  assert.equal(existsSync(targetPath), false, "core never writes the target; apply owns that");
  await rm(dir, { recursive: true, force: true });
});

test("an invalid draft is rejected and an invalid repair is never applied", async () => {
  const dir = await tmpDir();
  const targetPath = path.join(dir, "out.oxoflow");
  const applied: string[] = [];
  const seen: string[] = [];

  const run: DraftRunner = async (_exe, args) => {
    seen.push(await readFile(args[1], "utf8"));
    return cli(1, seen.length === 1 ? "ERROR E007: unknown rule ghost" : "ERROR E002: missing shell");
  };

  const { chat, calls } = scriptedChat([
    "```toml\n" + VALID + "```",
    "```toml\n" + REPAIRED + "```",
  ]);
  const outcome = await generateValidatedDraft("make a demo", {
    targetPath,
    executable: "oxo-flow",
    cwd: dir,
    chat,
    run,
    apply: async (toml) => {
      applied.push(toml);
    },
  });

  assert.equal(outcome.status, "rejected");
  assert.equal(outcome.status === "rejected" && outcome.attempts, 2);
  assert.equal(applied.length, 0, "invalid output must never be applied");
  assert.deepEqual(seen, [VALID.trim(), REPAIRED.trim()]);
  assert.ok(outcome.status === "rejected" && outcome.detail.includes("E002"), "detail carries the final findings");
  assert.equal(calls.length, 2, "exactly one repair round");
  assert.ok(
    calls[1].some((t) => t.role === "user" && t.text.includes("E007")),
    "repair prompt feeds the first findings back to the model"
  );
  assert.equal(existsSync(targetPath), false, "no destination file on rejection");
  await rm(dir, { recursive: true, force: true });
});

test("a repaired, re-validated draft is the one applied", async () => {
  const dir = await tmpDir();
  const targetPath = path.join(dir, "out.oxoflow");
  const applied: string[] = [];
  let runs = 0;

  const run: DraftRunner = async (_exe, args) => {
    runs += 1;
    const content = await readFile(args[1], "utf8");
    return content === REPAIRED.trim() ? cli(0, "", "{}") : cli(1, "ERROR E007: unknown rule ghost");
  };

  const { chat } = scriptedChat(["```toml\n" + VALID + "```", "```toml\n" + REPAIRED + "```"]);
  const outcome = await generateValidatedDraft("make a demo", {
    targetPath,
    executable: "oxo-flow",
    cwd: dir,
    chat,
    run,
    apply: async (toml) => {
      applied.push(toml);
    },
  });

  assert.deepEqual(outcome, { status: "applied", toml: REPAIRED.trim(), attempts: 2 });
  assert.equal(runs, 2, "the repaired draft must be re-validated");
  assert.deepEqual(applied, [REPAIRED.trim()]);
  await rm(dir, { recursive: true, force: true });
});

test("a repair round with no pipeline is rejected, not applied", async () => {
  const dir = await tmpDir();
  const targetPath = path.join(dir, "out.oxoflow");
  const applied: string[] = [];
  let runs = 0;

  const run: DraftRunner = async () => {
    runs += 1;
    return cli(1, "ERROR E007: unknown rule ghost");
  };

  const { chat } = scriptedChat(["```toml\n" + VALID + "```", "sorry, I cannot fix that"]);
  const outcome = await generateValidatedDraft("make a demo", {
    targetPath,
    executable: "oxo-flow",
    cwd: dir,
    chat,
    run,
    apply: async (toml) => {
      applied.push(toml);
    },
  });

  assert.equal(outcome.status, "rejected");
  assert.equal(runs, 1);
  assert.equal(applied.length, 0);
  assert.ok(outcome.status === "rejected" && /repair round returned no pipeline/.test(outcome.detail));
  await rm(dir, { recursive: true, force: true });
});

test("a response without a pipeline is surfaced without validation or apply", async () => {
  const dir = await tmpDir();
  const targetPath = path.join(dir, "out.oxoflow");
  let runs = 0;
  let applies = 0;

  const outcome = await generateValidatedDraft("make a demo", {
    targetPath,
    executable: "oxo-flow",
    cwd: dir,
    chat: scriptedChat(["no pipeline here"]).chat,
    run: async () => {
      runs += 1;
      return cli(0);
    },
    apply: async () => {
      applies += 1;
    },
  });

  assert.equal(outcome.status, "no-pipeline");
  assert.equal(runs, 0);
  assert.equal(applies, 0);
  await rm(dir, { recursive: true, force: true });
});

test("a spawn failure rejects without applying", async () => {
  const dir = await tmpDir();
  const targetPath = path.join(dir, "out.oxoflow");
  let applied = 0;

  const outcome = await generateValidatedDraft("make a demo", {
    targetPath,
    executable: "missing-oxo-flow",
    cwd: dir,
    chat: scriptedChat(["```toml\n" + VALID + "```", "```toml\n" + VALID + "```"]).chat,
    run: async () => ({ stdout: "", stderr: "", exitCode: null, spawnError: "ENOENT" }),
    apply: async () => {
      applied += 1;
    },
  });

  assert.equal(outcome.status, "rejected");
  assert.ok(outcome.status === "rejected" && outcome.detail.includes("ENOENT"));
  assert.equal(applied, 0);
  await rm(dir, { recursive: true, force: true });
});

test("validateDraftContent reports the CLI findings for a failing draft", async () => {
  const dir = await tmpDir();
  const verdict = await validateDraftContent(
    {
      targetPath: path.join(dir, "out.oxoflow"),
      executable: "oxo-flow",
      cwd: dir,
      run: async () => cli(1, "", '{"command":"validate","valid":false,"errors":[{"code":"E007"}]}'),
    },
    VALID
  );
  assert.equal(verdict.ok, false);
  assert.ok(verdict.detail.includes("E007"), verdict.detail);
  await rm(dir, { recursive: true, force: true });
});
