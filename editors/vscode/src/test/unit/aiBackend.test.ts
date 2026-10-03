import test from "node:test";
import assert from "node:assert/strict";
import {
  buildGenerationPrompt,
  extractToml,
  resolveBackend,
  schemaDigest,
} from "../../core/aiBackend";

const FENCED = '[workflow]\nname = "demo"\n\n[[rules]]\nname = "r1"\n';

test("extractToml prefers a fenced toml block", () => {
  const response = `Here is your pipeline:\n\n\`\`\`toml\n${FENCED}\`\`\`\n\nEnjoy!`;
  assert.equal(extractToml(response), FENCED.trim());
});

test("extractToml strips prose before a raw [workflow] table", () => {
  const response = "Sure!\n\n[workflow]\nname = 'demo'\n";
  assert.equal(extractToml(response), "[workflow]\nname = 'demo'");
});

test("extractToml returns null without a workflow table", () => {
  assert.equal(extractToml("no pipeline here"), null);
  assert.equal(extractToml(""), null);
});

test("resolveBackend auto prefers the CLI, then the IDE, then fails loudly", () => {
  const ok = resolveBackend("auto", { cliAiConfigured: true, ideModelsAvailable: true });
  assert.deepEqual(ok, { kind: "ok", backend: "cli", reason: "auto: CLI AI provider configured" });
  const ide = resolveBackend("auto", { cliAiConfigured: false, ideModelsAvailable: true });
  assert.equal(ide.kind === "ok" && ide.backend, "ide");
  const none = resolveBackend("auto", { cliAiConfigured: false, ideModelsAvailable: false });
  assert.equal(none.kind, "unavailable");
  assert.match(none.kind === "unavailable" ? none.reason : "", /No AI backend available/);
});

test("resolveBackend honors forced backends and explains gaps", () => {
  const forcedCli = resolveBackend("cli", { cliAiConfigured: false, ideModelsAvailable: true });
  assert.equal(forcedCli.kind, "unavailable");
  assert.match(forcedCli.kind === "unavailable" ? forcedCli.reason : "", /forced to 'cli'/);
  const forcedIde = resolveBackend("ide", { cliAiConfigured: true, ideModelsAvailable: false });
  assert.equal(forcedIde.kind, "unavailable");
  assert.match(forcedIde.kind === "unavailable" ? forcedIde.reason : "", /forced to 'ide'/);
  assert.equal(resolveBackend("ide", { cliAiConfigured: false, ideModelsAvailable: true }).kind, "ok");
});

test("schemaDigest lists rules keys and buildGenerationPrompt embeds the request", () => {
  const digest = schemaDigest();
  assert.ok(digest.includes("[rules]"), digest.slice(0, 200));
  assert.ok(digest.includes("[workflow]"));
  const prompt = buildGenerationPrompt("RNA-seq with STAR");
  assert.ok(prompt.includes("RNA-seq with STAR"));
  assert.ok(prompt.includes("```toml"));
});
