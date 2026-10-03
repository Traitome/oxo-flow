import * as assert from "node:assert/strict";
import * as path from "node:path";
import * as vscode from "vscode";

// `describe`/`it` come from the mocha instance inside the test runner
// (injected as globals; .vscode-test.mjs switches its UI to BDD). Importing
// our own copy of mocha here would create a second interface instance bound
// to no suite.

const EXT_ID = "traitome.oxo-flow";
// __dirname = out/test/integration → editors/vscode/src/test/fixtures.
const FIXTURES = path.join(__dirname, "..", "..", "..", "src", "test", "fixtures");
const WAIT_MS = 60_000;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function openFixture(name: string): Promise<vscode.TextDocument> {
  return vscode.workspace.openTextDocument(path.join(FIXTURES, name));
}

/** Poll until the probe yields a value; throw with `what` on timeout. */
async function waitFor<T>(what: string, probe: () => T | undefined): Promise<T> {
  const deadline = Date.now() + WAIT_MS;
  for (;;) {
    const value = probe();
    if (value !== undefined) return value;
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${what}`);
    }
    await sleep(250);
  }
}

/** Point the extension at the freshly built CLI for the duration of `body`. */
async function withTestBinary(bin: string, body: () => Promise<void>): Promise<void> {
  const cfg = vscode.workspace.getConfiguration("oxo-flow");
  await cfg.update("executablePath", bin, vscode.ConfigurationTarget.Workspace);
  try {
    await body();
  } finally {
    await cfg.update("executablePath", undefined, vscode.ConfigurationTarget.Workspace);
  }
}

describe("oxo-flow extension", () => {
  it("activates and registers its commands", async () => {
    const ext = vscode.extensions.getExtension(EXT_ID);
    assert.ok(ext, `extension ${EXT_ID} not found`);
    await ext!.activate();
    assert.ok(ext!.isActive);
    const commands = await vscode.commands.getCommands(true);
    for (const id of [
      "oxo-flow.run",
      "oxo-flow.runTargets",
      "oxo-flow.validate",
      "oxo-flow.format",
      "oxo-flow.pickCommand",
    ]) {
      assert.ok(commands.includes(id), `command ${id} not registered`);
    }
  });

  it("shows Run CodeLens on [[rules]] headers", async () => {
    const content = [
      "[workflow]",
      'name = "codelens-fixture"',
      "",
      "[[rules]]",
      'name = "greet"',
      'output = ["hello.txt"]',
      'shell = "echo hello > {output}"',
      "",
      "[[rules]]",
      'name = "second"',
      'depends_on = ["greet"]',
    ].join("\n");
    const doc = await vscode.workspace.openTextDocument({ content, language: "oxoflow" });
    await vscode.window.showTextDocument(doc);
    const lenses = await vscode.commands.executeCommand<vscode.CodeLens[]>(
      "vscode.executeCodeLensProvider",
      doc.uri
    );
    const titles = lenses.map((l) => (l.command?.title ?? ""));
    assert.ok(
      titles.includes("▶ Run this rule") && titles.includes("▶ Run to here"),
      `expected run lenses; got: ${JSON.stringify(titles)}`
    );
  });

  it("recognizes .oxoflow documents as the oxoflow language", async () => {
    const doc = await openFixture("valid.oxoflow");
    assert.equal(doc.languageId, "oxoflow");
  });

  it("offers schema-driven completions at the document root", async () => {
    // An empty document has no sections yet, so the cursor sits in the root
    // context (the fixtures all start with [workflow] at line 0).
    const doc = await vscode.workspace.openTextDocument({ content: "", language: "oxoflow" });
    const pos = new vscode.Position(0, 0);
    const list = await vscode.commands.executeCommand<vscode.CompletionList>(
      "vscode.executeCompletionItemProvider",
      doc.uri,
      pos
    );
    const labels = list.items.map((i) => (typeof i.label === "string" ? i.label : i.label.label));
    for (const expected of ["workflow", "rules", "config", "ai"]) {
      assert.ok(labels.includes(expected), `completion missing ${expected}; got: ${labels.join(", ")}`);
    }
  });

  it("offers rule-reference completions inside depends_on", async () => {
    const content = [
      "[workflow]",
      'name = "completion-fixture"',
      "",
      "[[rules]]",
      'name = "greet"',
      'output = ["hello.txt"]',
      "",
      "[[rules]]",
      'name = "second"',
      'depends_on = [""]',
    ].join("\n");
    const doc = await vscode.workspace.openTextDocument({ content, language: "oxoflow" });
    // Cursor inside the empty depends_on string of rule "second".
    const pos = new vscode.Position(9, 'depends_on = ["'.length);
    const list = await vscode.commands.executeCommand<vscode.CompletionList>(
      "vscode.executeCompletionItemProvider",
      doc.uri,
      pos
    );
    const labels = list.items.map((i) => (typeof i.label === "string" ? i.label : i.label.label));
    assert.ok(
      labels.includes("greet"),
      `depends_on completion should list rule 'greet'; got: ${labels.join(", ")}`
    );
  });

  it("flags a missing input as a rule-anchored warning", async function () {
    const bin = process.env.OXOFLOW_TEST_BIN;
    if (!bin) {
      this.skip();
      return;
    }
    await withTestBinary(bin, async () => {
      const doc = await openFixture("missing-input.oxoflow");
      await vscode.window.showTextDocument(doc);
      const diags = await waitFor("oxo-flow diagnostics", () => {
        const found = vscode.languages.getDiagnostics(doc.uri).filter((d) => d.source === "oxo-flow");
        return found.length > 0 ? found : undefined;
      });
      const w020 = diags.find((d) => String(d.code) === "W020");
      assert.ok(w020, `expected a W020 missing-input warning; got: ${diags.map((d) => String(d.code)).join(",")}`);
      assert.equal(w020!.severity, vscode.DiagnosticSeverity.Warning);
      const ruleLine = doc
        .getText()
        .split("\n")
        .findIndex((l) => l.includes('name = "copy_missing"'));
      assert.equal(w020!.range.start.line, ruleLine, "W020 should anchor to the rule's name line");
    });
  });

  it("anchors validate errors to the failing rule's name line", async function () {
    const bin = process.env.OXOFLOW_TEST_BIN;
    if (!bin) {
      this.skip();
      return;
    }
    await withTestBinary(bin, async () => {
      const doc = await openFixture("invalid-dep.oxoflow");
      await vscode.window.showTextDocument(doc);
      const diags = await waitFor("validate error diagnostics", () => {
        const found = vscode.languages
          .getDiagnostics(doc.uri)
          .filter((d) => d.source === "oxo-flow" && d.severity === vscode.DiagnosticSeverity.Error);
        return found.length > 0 ? found : undefined;
      });
      const e007 = diags.find((d) => String(d.code) === "E007");
      assert.ok(e007, `expected an E007 unknown-rule error; got: ${diags.map((d) => String(d.code)).join(",")}`);
      const ruleLine = doc
        .getText()
        .split("\n")
        .findIndex((l) => l.includes('name = "r1"'));
      assert.equal(e007!.range.start.line, ruleLine, "E007 should anchor to rule r1's name line");
    });
  });

  it("offers quick actions on oxo-flow diagnostics", async function () {
    const bin = process.env.OXOFLOW_TEST_BIN;
    if (!bin) {
      this.skip();
      return;
    }
    await withTestBinary(bin, async () => {
      const doc = await openFixture("missing-input.oxoflow");
      await vscode.window.showTextDocument(doc);
      const diags = await waitFor("diagnostics for quick actions", () => {
        const found = vscode.languages.getDiagnostics(doc.uri).filter((d) => d.source === "oxo-flow");
        return found.length > 0 ? found : undefined;
      });
      const context: vscode.CodeActionContext = {
        triggerKind: vscode.CodeActionTriggerKind.Invoke,
        diagnostics: diags,
        only: vscode.CodeActionKind.QuickFix,
      };
      const actions = await vscode.commands.executeCommand<vscode.CodeAction[]>(
        "vscode.executeCodeActionProvider",
        doc.uri,
        diags[0].range,
        context
      );
      const titles = (actions ?? []).map((a) => a.title);
      assert.ok(
        titles.some((t) => t.includes("Re-validate")),
        `expected a Re-validate action; got: ${JSON.stringify(titles)}`
      );
      assert.ok(
        titles.some((t) => t.includes("Report Issue")),
        `expected a Report Issue action; got: ${JSON.stringify(titles)}`
      );
    });
  });

  it("provides hover docs for schema-known keys", async () => {
    const doc = await vscode.workspace.openTextDocument({
      content: '[workflow]\nname = "hover-fixture"\n',
      language: "oxoflow",
    });
    const hovers = await vscode.commands.executeCommand<vscode.Hover[]>(
      "vscode.executeHoverProvider",
      doc.uri,
      new vscode.Position(1, 1)
    );
    assert.ok(hovers.length > 0, "expected hover for workflow.name");
    // Multiple providers may contribute; ours carries the schema description.
    // NB: MarkdownString keeps `value` in a non-enumerable field — read it,
    // JSON.stringify would yield `{}`.
    const md = hovers
      .flatMap((h) => h.contents)
      .map((c) => (typeof c === "string" ? c : ((c as vscode.MarkdownString).value ?? "")))
      .join("\n");
    assert.ok(
      md.includes("name") && md.length > 20,
      `expected schema docs in hover contents; got: ${md.slice(0, 120)}`
    );
  });
});
