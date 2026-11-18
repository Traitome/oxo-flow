import test from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import * as path from "node:path";
import {
  formatFindings,
  generateValidatedDraft,
  validateDraftContent,
  type DraftChatTurn,
  type DraftRunner,
} from "../../core/aiDraft";
import type { CliResult } from "../../core/exec";

const VALID = '[workflow]\nname = "generated"\n\n[[rules]]\nname = "r1"\nshell = "true"\n';
const REPAIRED = '[workflow]\nname = "repaired"\n\n[[rules]]\nname = "r1"\nshell = "true"\n';

/** Real shape of `validate --json` for an invalid workflow: findings live on
 * stdout, while stderr only carries the CLI's bail summary. */
function invalidJson(code: string, message: string, rule: string | null, suggestion: string | null): string {
  return JSON.stringify({
    command: "validate",
    workflow: "p.oxoflow",
    valid: false,
    rules: 1,
    dependencies: 0,
    errors: [{ code, message, rule, suggestion }],
    missing_inputs: [],
    warnings: [],
  });
}

const E007_JSON = invalidJson(
  "E007",
  "depends_on references non-existent rule 'ghost_rule'",
  "r1",
  "ensure rule 'ghost_rule' is defined in the workflow or remove it from depends_on"
);
const E002_JSON = invalidJson("E002", "rule has no output", "r1", "declare output = [\"...\"]");
const BAIL = "Error: validation failed: 1 error in p.oxoflow";

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

test("formatFindings renders code, rule, message and hint", () => {
  const text = formatFindings([
    {
      severity: "error",
      code: "E007",
      message: "depends_on references non-existent rule 'ghost_rule'",
      rule: "r1",
      suggestion: "define the rule",
      source: "validate",
    },
  ]);
  assert.match(text, /E007:/);
  assert.match(text, /ghost_rule/);
  assert.match(text, /\(rule: r1\)/);
  assert.match(text, /hint: define the rule/);
});

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

test("invalid draft findings reach the repair prompt and the user report", async () => {
  const dir = await tmpDir();
  const targetPath = path.join(dir, "out.oxoflow");
  const applied: string[] = [];

  const run: DraftRunner = async () => cli(1, BAIL, E007_JSON);

  const { chat, calls } = scriptedChat(["```toml\n" + VALID + "```", "no fix available"]);
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

  // The CLI's stderr bail summary is NOT the finding: the stdout JSON is.
  const repairTurn = calls[1].find((t) => t.role === "user" && t.text.includes("failed `oxo-flow validate`"));
  assert.ok(repairTurn, "a repair round must be requested");
  assert.ok(repairTurn.text.includes("E007"), repairTurn.text);
  assert.ok(repairTurn.text.includes("ghost_rule"), repairTurn.text);
  assert.ok(repairTurn.text.includes("(rule: r1)"), repairTurn.text);
  assert.ok(repairTurn.text.includes("hint: ensure rule 'ghost_rule' is defined"), repairTurn.text);
  assert.ok(!repairTurn.text.includes("Error: validation failed"), "raw bail summary must not be the finding");

  assert.equal(outcome.status, "rejected");
  assert.ok(outcome.status === "rejected" && outcome.detail.includes("E007"));
  assert.ok(outcome.status === "rejected" && outcome.detail.includes("(rule: r1)"));
  assert.equal(applied.length, 0, "invalid output must never be applied");
  assert.equal(existsSync(targetPath), false, "no destination file on rejection");
  await rm(dir, { recursive: true, force: true });
});

test("an invalid repair is rejected with its own structured findings", async () => {
  const dir = await tmpDir();
  const targetPath = path.join(dir, "out.oxoflow");
  const applied: string[] = [];
  const seen: string[] = [];

  const run: DraftRunner = async (_exe, args) => {
    const content = await readFile(args[1], "utf8");
    seen.push(content);
    return seen.length === 1 ? cli(1, BAIL, E007_JSON) : cli(1, BAIL, E002_JSON);
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
  assert.ok(outcome.status === "rejected" && outcome.detail.includes("E002"));
  assert.ok(outcome.status === "rejected" && outcome.detail.includes("rule has no output"));
  assert.equal(calls.length, 2, "exactly one repair round");
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
    return content === REPAIRED.trim()
      ? cli(0, "", '{"command":"validate","valid":true}')
      : cli(1, BAIL, E007_JSON);
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
    return cli(1, BAIL, E007_JSON);
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
  assert.ok(outcome.status === "rejected" && outcome.detail.includes("E007"));
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

test("a spawn failure is unavailable, names the cause, and burns no repair round", async () => {
  const dir = await tmpDir();
  const targetPath = path.join(dir, "out.oxoflow");
  let applied = 0;

  const { chat, calls } = scriptedChat(["```toml\n" + VALID + "```"]);
  const outcome = await generateValidatedDraft("make a demo", {
    targetPath,
    executable: "missing-oxo-flow",
    cwd: dir,
    chat,
    run: async () => ({ stdout: "", stderr: "", exitCode: null, spawnError: "ENOENT" }),
    apply: async () => {
      applied += 1;
    },
  });

  assert.equal(outcome.status, "unavailable", "infra failure is not a validation verdict");
  assert.ok(outcome.status === "unavailable" && outcome.detail.includes("cannot run missing-oxo-flow"));
  assert.ok(outcome.status === "unavailable" && outcome.detail.includes("ENOENT"));
  assert.equal(calls.length, 1, "no repair round on an unfixable condition");
  assert.equal(applied, 0);
  assert.equal(existsSync(targetPath), false);
  await rm(dir, { recursive: true, force: true });
});

test("a validation timeout is unavailable, not a rejected draft", async () => {
  const dir = await tmpDir();
  const { chat, calls } = scriptedChat(["```toml\n" + VALID + "```"]);
  const outcome = await generateValidatedDraft("make a demo", {
    targetPath: path.join(dir, "out.oxoflow"),
    executable: "oxo-flow",
    cwd: dir,
    chat,
    run: async () => ({ stdout: "", stderr: "", exitCode: null, timedOut: true }),
    apply: async () => {
      throw new Error("apply must not run");
    },
  });

  assert.equal(outcome.status, "unavailable");
  assert.ok(outcome.status === "unavailable" && /timed out/.test(outcome.detail));
  assert.equal(calls.length, 1);
  await rm(dir, { recursive: true, force: true });
});

test("an unstageable draft (read-only/missing directory) is unavailable", async () => {
  const dir = await tmpDir();
  let runs = 0;
  const { chat, calls } = scriptedChat(["```toml\n" + VALID + "```"]);
  const outcome = await generateValidatedDraft("make a demo", {
    targetPath: path.join(dir, "no-such-subdir", "out.oxoflow"),
    executable: "oxo-flow",
    cwd: dir,
    chat,
    run: async () => {
      runs += 1;
      return cli(0);
    },
    apply: async () => {
      throw new Error("apply must not run");
    },
  });

  assert.equal(outcome.status, "unavailable");
  assert.ok(outcome.status === "unavailable" && /cannot stage the draft/.test(outcome.detail));
  assert.equal(runs, 0, "the CLI is never invoked when staging fails");
  assert.equal(calls.length, 1);
  await rm(dir, { recursive: true, force: true });
});

test("validateDraftContent separates invalid findings from infra failures", async () => {
  const dir = await tmpDir();
  const deps = {
    targetPath: path.join(dir, "out.oxoflow"),
    executable: "oxo-flow",
    cwd: dir,
  };

  const invalid = await validateDraftContent(
    { ...deps, run: async () => cli(1, BAIL, E007_JSON) },
    VALID
  );
  assert.equal(invalid.ok, false);
  assert.equal(invalid.unavailable, false);
  assert.ok(invalid.detail.includes("E007"), invalid.detail);
  assert.ok(invalid.detail.includes("(rule: r1)"), invalid.detail);
  assert.ok(invalid.detail.includes("hint:"), invalid.detail);

  const unavailable = await validateDraftContent(
    { ...deps, run: async () => ({ stdout: "", stderr: "", exitCode: null, spawnError: "EACCES" }) },
    VALID
  );
  assert.equal(unavailable.ok, false);
  assert.equal(unavailable.unavailable, true);
  assert.ok(unavailable.detail.includes("cannot run"));
  assert.ok(unavailable.detail.includes("EACCES"));
  await rm(dir, { recursive: true, force: true });
});
