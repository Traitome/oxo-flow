import * as vscode from "vscode";
import {
  aiStatusArgs,
  cleanArgs,
  schemaArgs,
  statusArgs,
  templateArgs,
  validateArgs,
  lintArgs,
} from "./core/cliArgs";
import { runCli } from "./core/exec";
import { runArgs } from "./core/cliArgs";
import { parseReport } from "./core/jsonReport";
import { OxoflowCodeLensProvider } from "./providers/codelens";
import { OxoflowCompletionProvider } from "./providers/completion";
import { OxoflowDiagnostics } from "./providers/diagnostics";
import { OxoflowFormattingProvider } from "./providers/formatting";
import { OxoflowHoverProvider } from "./providers/hover";
import { StatusBar } from "./providers/statusBar";
import {
  createTask,
  OxoflowTaskProvider,
  relativeWorkflow,
  type OxoflowTaskDefinition,
  type TaskKind,
} from "./providers/taskProvider";

const DOCS_URL = "https://traitome.github.io/oxo-flow/";

export function activate(context: vscode.ExtensionContext): void {
  const output = vscode.window.createOutputChannel("oxo-flow");
  context.subscriptions.push(output);

  const statusBar = new StatusBar();
  statusBar.activate(context);

  const diagnostics = new OxoflowDiagnostics(output);
  diagnostics.activate(context);

  const taskProvider = new OxoflowTaskProvider(() => statusBar.executable());
  context.subscriptions.push(
    vscode.tasks.registerTaskProvider("oxo-flow", taskProvider),
    vscode.languages.registerCompletionItemProvider("oxoflow", new OxoflowCompletionProvider(), "[", "{", ".", '"'),
    vscode.languages.registerHoverProvider("oxoflow", new OxoflowHoverProvider()),
    vscode.languages.registerDocumentFormattingEditProvider("oxoflow", new OxoflowFormattingProvider()),
    vscode.languages.registerCodeLensProvider("oxoflow", new OxoflowCodeLensProvider())
  );

  // Format-on-save: reuse the canonical formatter when `oxo-flow.formatOnSave`
  // is enabled for the file. This is an independent opt-in (so users can have
  // oxo-flow formatting on save without turning on global format-on-save for
  // every other language); when *Editor: Format On Save* is already on, VS
  // Code invokes our registered formatting provider itself and we stay out of
  // the way. waitUntil keeps the save waiting until the CLI formatter applied.
  context.subscriptions.push(
    vscode.workspace.onWillSaveTextDocument((event) => {
      const doc = event.document;
      if (doc.languageId !== "oxoflow") return;
      const cfg = vscode.workspace.getConfiguration("oxo-flow", doc.uri);
      if (!cfg.get<boolean>("formatOnSave", false)) return;
      const editorCfg = vscode.workspace.getConfiguration("editor", doc.uri);
      if (editorCfg.get<boolean>("formatOnSave", false)) return;
      event.waitUntil(
        new OxoflowFormattingProvider().provideDocumentFormattingEdits(
          doc,
          { insertSpaces: true, tabSize: 4 },
          new vscode.CancellationTokenSource().token
        )
      );
    })
  );

  const register = (id: string, fn: (...args: unknown[]) => unknown) => {
    context.subscriptions.push(vscode.commands.registerCommand(id, fn));
  };

  register("oxo-flow.run", () => executePipelineTask(statusBar, "run"));
  register("oxo-flow.runTargets", (...args: unknown[]) => executeRunTargets(statusBar, args[0]));
  register("oxo-flow.dryRun", () => executePipelineTask(statusBar, "dry-run"));
  register("oxo-flow.graph", () => executePipelineTask(statusBar, "graph"));
  register("oxo-flow.validate", () => runQualityCommand(output, statusBar, "validate"));
  register("oxo-flow.lint", () => runQualityCommand(output, statusBar, "lint"));
  register("oxo-flow.format", () => formatDocument());
  register("oxo-flow.resume", () => resumePipeline(statusBar));
  register("oxo-flow.generate", () => generateWithAI(statusBar));
  register("oxo-flow.aiStatus", () => showAiStatus(output, statusBar));
  register("oxo-flow.exportSchema", () => exportSchema(statusBar));
  register("oxo-flow.clean", () => cleanOutputs(output, statusBar));
  register("oxo-flow.status", () => showRunStatus(output, statusBar));
  register("oxo-flow.openDocs", () => vscode.env.openExternal(vscode.Uri.parse(DOCS_URL)));
  register("oxo-flow.openSettings", () =>
    vscode.commands.executeCommand("workbench.action.openSettings", "oxo-flow.executablePath")
  );
  register("oxo-flow.pickCommand", () => pickCommand());
}

export function deactivate(): void {
  // All disposables are registered on the extension context.
}

// ─── pipeline selection ───────────────────────────────────────────────────

async function pipelineTarget(): Promise<{ folder: vscode.WorkspaceFolder; file: vscode.Uri } | undefined> {
  const active = vscode.window.activeTextEditor?.document;
  if (active && active.languageId === "oxoflow" && active.uri.scheme === "file") {
    const folder = folderFor(active.uri);
    if (folder) return { folder, file: active.uri };
  }
  const files = await vscode.workspace.findFiles("**/*.oxoflow", "**/node_modules/**", 50);
  if (files.length === 0) {
    void vscode.window.showInformationMessage("No .oxoflow pipeline found in this workspace.");
    return undefined;
  }
  const file =
    files.length === 1
      ? files[0]
      : await vscode.window
          .showQuickPick(
            files.map((f) => ({ label: vscode.workspace.asRelativePath(f), file: f })),
            { placeHolder: "Select a pipeline" }
          )
          .then((pick) => pick?.file);
  if (!file) return undefined;
  const folder = folderFor(file);
  if (!folder) {
    void vscode.window.showErrorMessage(`Cannot determine the workspace folder for ${file.fsPath}.`);
    return undefined;
  }
  return { folder, file };
}

function folderFor(uri: vscode.Uri): vscode.WorkspaceFolder | undefined {
  return vscode.workspace.getWorkspaceFolder(uri) ?? vscode.workspace.workspaceFolders?.[0];
}

async function executePipelineTask(
  statusBar: StatusBar,
  kind: "run" | "dry-run" | "graph"
): Promise<void> {
  const target = await pipelineTarget();
  if (!target) return;
  const def: OxoflowTaskDefinition = {
    type: "oxo-flow",
    workflow: relativeWorkflow(target.folder, target.file),
    kind,
  };
  const cfg = vscode.workspace.getConfiguration("oxo-flow", target.file);
  if (kind === "run") {
    def.extraArgs = cfg.get<string[]>("runArgs", []);
  }
  await vscode.tasks.executeTask(createTask(statusBar.executable(), target.folder, def, kind));
}

/**
 * CodeLens entry: run an explicit set of `-t` targets. A single name lets the
 * CLI resolve the rule's upstream closure; a list pins every rule up to a
 * point in file order ("run to here").
 */
async function executeRunTargets(statusBar: StatusBar, rawTargets: unknown): Promise<void> {
  const targets = Array.isArray(rawTargets) ? rawTargets.filter((t): t is string => typeof t === "string") : [];
  if (targets.length === 0) return;
  const target = await pipelineTarget();
  if (!target) return;
  const cfg = vscode.workspace.getConfiguration("oxo-flow", target.file);
  const runOpts = {
    file: relativeWorkflow(target.folder, target.file),
    targets,
    extraArgs: cfg.get<string[]>("runArgs", []),
  };
  await vscode.tasks.executeTask(
    createTask(
      statusBar.executable(),
      target.folder,
      { type: "oxo-flow", workflow: runOpts.file, kind: "run" },
      "run",
      runArgs(runOpts)
    )
  );
}

// ─── validate / lint ──────────────────────────────────────────────────────

async function runQualityCommand(
  output: vscode.OutputChannel,
  statusBar: StatusBar,
  kind: "validate" | "lint"
): Promise<void> {
  const target = await pipelineTarget();
  if (!target) return;
  const doc = vscode.window.activeTextEditor?.document;
  const dirty = doc?.uri.toString() === target.file.toString() && doc.isDirty;
  const executable = statusBar.executable();
  const args = kind === "validate" ? validateArgs(target.file.fsPath) : lintArgs(target.file.fsPath);
  const res = await runCli(executable, args, { cwd: target.folder.uri.fsPath });

  if (res.spawnError) {
    void vscode.window.showErrorMessage(
      `Cannot run \`${executable}\` (${res.spawnError}). Set oxo-flow.executablePath in settings.`
    );
    return;
  }

  const lines = [`$ oxo-flow ${args.map((a) => JSON.stringify(a)).join(" ")}`];
  if (dirty) lines.push("(document has unsaved changes — the saved file was analyzed)");
  if (res.stderr.trim()) lines.push(res.stderr.trimEnd());
  try {
    const report = parseReport(res.stdout);
    lines.push("```json", JSON.stringify(report, null, 2), "```");
  } catch {
    if (res.stdout.trim()) lines.push(res.stdout.trimEnd());
  }
  output.appendLine(lines.join("\n"));
  output.show(true);

  if (kind === "validate") {
    const ok = res.exitCode === 0;
    const pick = await vscode.window.showInformationMessage(
      ok ? "Pipeline is valid." : "Validation failed — see the oxo-flow output panel.",
      "Show Problems"
    );
    if (pick === "Show Problems") void vscode.commands.executeCommand("workbench.actions.view.problems");
  } else {
    void vscode.window.showInformationMessage(
      res.exitCode === 0 ? "Lint passed." : "Lint found issues — see the oxo-flow output panel."
    );
  }
}

async function formatDocument(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "oxoflow") {
    void vscode.window.showInformationMessage("Open a .oxoflow file to format it.");
    return;
  }
  await vscode.commands.executeCommand("editor.action.formatDocument");
}

// ─── resume / AI / schema ─────────────────────────────────────────────────

async function resumePipeline(statusBar: StatusBar): Promise<void> {
  const checkpoints = await vscode.workspace.findFiles("**/.oxo-flow/checkpoint.json", "**/node_modules/**", 20);
  if (checkpoints.length === 0) {
    void vscode.window.showInformationMessage("No .oxo-flow/checkpoint.json found in this workspace.");
    return;
  }
  const pick = await vscode.window.showQuickPick(
    checkpoints.map((c) => ({ label: vscode.workspace.asRelativePath(c), file: c })),
    { placeHolder: "Select a checkpoint to resume" }
  );
  if (!pick) return;
  const folder = folderFor(pick.file);
  if (!folder) return;
  await vscode.tasks.executeTask(
    createTask(
      statusBar.executable(),
      folder,
      {
        type: "oxo-flow",
        workflow: vscode.workspace.asRelativePath(pick.file),
        kind: "resume",
      },
      "resume"
    )
  );
}

async function generateWithAI(statusBar: StatusBar): Promise<void> {
  const description = await vscode.window.showInputBox({
    prompt: "Describe the pipeline to generate (e.g. 'RNA-seq with STAR and featureCounts')",
    placeHolder: "Natural-language description",
  });
  if (!description) return;
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    void vscode.window.showErrorMessage("Open a workspace folder first — the pipeline needs a home.");
    return;
  }
  const slug =
    description
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 40) || "generated-pipeline";
  const target = await vscode.window.showSaveDialog({
    defaultUri: vscode.Uri.joinPath(folder.uri, `${slug}.oxoflow`),
    filters: { "oxo-flow pipeline": ["oxoflow"] },
  });
  if (!target) return;

  // Run through a task (not terminal.sendText): the description is natural
  // language and may contain quotes/newlines — ShellExecution quoting handles
  // every metacharacter, whereas a hand-quoted shell string breaks on `\n`
  // (sendText treats it as Enter) and on shell-specific escapes.
  await vscode.tasks.executeTask(
    createTask(
      statusBar.executable(),
      folder,
      {
        type: "oxo-flow",
        workflow: vscode.workspace.asRelativePath(target),
        kind: "generate",
      },
      "generate",
      templateArgs(description, target.fsPath)
    )
  );
  void vscode.window.showInformationMessage(
    "AI generation runs as a task; the file appears when the CLI finishes."
  );
}

async function showAiStatus(output: vscode.OutputChannel, statusBar: StatusBar): Promise<void> {
  const executable = statusBar.executable();
  const res = await runCli(executable, aiStatusArgs(), { timeoutMs: 30_000 });
  if (res.spawnError) {
    void vscode.window.showErrorMessage(`Cannot run \`${executable}\` (${res.spawnError}).`);
    return;
  }
  output.appendLine(`$ oxo-flow ${aiStatusArgs().join(" ")}\n${res.stdout.trimEnd()}\n${res.stderr.trimEnd()}`);
  output.show(true);
  let summary = "AI status — see the oxo-flow output panel.";
  try {
    const parsed = JSON.parse(res.stdout) as { provider?: string };
    if (parsed.provider) summary = `AI provider: ${parsed.provider}`;
  } catch {
    // Human output — the panel already shows it.
  }
  void vscode.window.showInformationMessage(summary);
}

async function exportSchema(statusBar: StatusBar): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    void vscode.window.showErrorMessage("Open a workspace folder first.");
    return;
  }
  const executable = statusBar.executable();
  const res = await runCli(executable, schemaArgs(), { cwd: folder.uri.fsPath });
  if (res.spawnError || res.exitCode !== 0) {
    void vscode.window.showErrorMessage(`oxo-flow schema failed: ${res.spawnError ?? res.stderr}`);
    return;
  }
  const dest = vscode.Uri.joinPath(folder.uri, "oxo-flow-schema.json");
  await vscode.workspace.fs.writeFile(dest, Buffer.from(res.stdout, "utf8"));
  const open = await vscode.window.showInformationMessage(
    'Exported oxo-flow-schema.json. Pair it with Even Better TOML via "evenBetterToml.schema.associations".',
    "Open editor setup docs"
  );
  if (open) {
    await vscode.env.openExternal(vscode.Uri.parse(`${DOCS_URL}how-to/editor-setup/`));
  }
}

// ─── clean / status ───────────────────────────────────────────────────────

/**
 * Clean declared outputs: always previews with `-n` first (the CLI also
 * defaults to a dry-run), shows the preview, then asks for confirmation
 * before running with `--force`. Destructive work never happens silently.
 */
async function cleanOutputs(output: vscode.OutputChannel, statusBar: StatusBar): Promise<void> {
  const target = await pipelineTarget();
  if (!target) return;
  const executable = statusBar.executable();
  const rel = relativeWorkflow(target.folder, target.file);

  const preview = await runCli(executable, cleanArgs(target.file.fsPath, { dryRun: true }), {
    cwd: target.folder.uri.fsPath,
  });
  if (preview.spawnError) {
    void vscode.window.showErrorMessage(
      `Cannot run \`${executable}\` (${preview.spawnError}). Set oxo-flow.executablePath in settings.`
    );
    return;
  }
  output.appendLine(`$ oxo-flow clean ${JSON.stringify(rel)} -n\n${preview.stderr.trimEnd()}`);
  output.show(true);

  if (preview.exitCode !== 0) {
    void vscode.window.showErrorMessage("Clean preview failed — see the oxo-flow output panel.");
    return;
  }

  const confirm = await vscode.window.showWarningMessage(
    `Delete outputs of ${rel}? Check the preview in the oxo-flow output panel.`,
    { modal: true },
    "Delete outputs",
    "Orphan chunks only"
  );
  if (confirm === "Orphan chunks only") {
    await vscode.tasks.executeTask(
      createTask(
        executable,
        target.folder,
        { type: "oxo-flow", workflow: rel, kind: "run" },
        "run",
        cleanArgs(target.file.fsPath, { force: true, orphans: true })
      )
    );
    return;
  }
  if (confirm !== "Delete outputs") return;
  await vscode.tasks.executeTask(
    createTask(
      executable,
      target.folder,
      { type: "oxo-flow", workflow: rel, kind: "run" },
      "run",
      cleanArgs(target.file.fsPath, { force: true })
    )
  );
}

/**
 * Show run status: quick pick over checkpoint files (.oxo-flow/checkpoint.json;
 * the CLI's status takes a CHECKPOINT, not a workflow) and dump `status
 * --timing` into the output panel.
 */
async function showRunStatus(output: vscode.OutputChannel, statusBar: StatusBar): Promise<void> {
  const checkpoints = await vscode.workspace.findFiles("**/.oxo-flow/checkpoint.json", "**/node_modules/**", 20);
  if (checkpoints.length === 0) {
    void vscode.window.showInformationMessage("No .oxo-flow/checkpoint.json found in this workspace.");
    return;
  }
  const picks = checkpoints.map((c) => ({ label: vscode.workspace.asRelativePath(c), file: c }));
  const pick =
    picks.length === 1
      ? picks[0]
      : await vscode.window.showQuickPick(picks, { placeHolder: "Select a checkpoint" });
  if (!pick) return;
  const folder = folderFor(pick.file);
  if (!folder) return;
  const executable = statusBar.executable();
  const args = statusArgs(vscode.workspace.asRelativePath(pick.file), { timing: true });
  const res = await runCli(executable, args, { cwd: folder.uri.fsPath });
  if (res.spawnError) {
    void vscode.window.showErrorMessage("Cannot run oxo-flow — see the oxo-flow output panel.");
    output.appendLine(`Cannot run \`${executable}\` (${res.spawnError}).`);
    output.show(true);
    return;
  }
  output.appendLine(`$ oxo-flow ${args.join(" ")}\n${res.stdout.trimEnd()}${res.stderr.trimEnd()}`);
  output.show(true);
}

// ─── command quick pick (status bar) ──────────────────────────────────────

const COMMAND_PICKS: { label: string; command: string }[] = [
  { label: "$(play) Run Pipeline", command: "oxo-flow.run" },
  { label: "$(debug-step-over) Dry Run (plan only)", command: "oxo-flow.dryRun" },
  { label: "$(checklist) Validate Pipeline", command: "oxo-flow.validate" },
  { label: "$(shield) Lint Pipeline", command: "oxo-flow.lint" },
  { label: "$(alignment-align) Format Document", command: "oxo-flow.format" },
  { label: "$(graph) Show DAG Graph", command: "oxo-flow.graph" },
  { label: "$(debug-restart) Resume from Checkpoint", command: "oxo-flow.resume" },
  { label: "$(info) Show Run Status", command: "oxo-flow.status" },
  { label: "$(trash) Clean Outputs…", command: "oxo-flow.clean" },
  { label: "$(sparkle) Generate Pipeline with AI…", command: "oxo-flow.generate" },
  { label: "$(hubot) Show AI Provider Status", command: "oxo-flow.aiStatus" },
  { label: "$(json) Export JSON Schema", command: "oxo-flow.exportSchema" },
  { label: "$(book) Open Documentation", command: "oxo-flow.openDocs" },
  { label: "$(gear) Open Settings", command: "oxo-flow.openSettings" },
];

async function pickCommand(): Promise<void> {
  const pick = await vscode.window.showQuickPick(COMMAND_PICKS, { placeHolder: "oxo-flow command" });
  if (pick) await vscode.commands.executeCommand(pick.command);
}

// TaskKind re-export guard: keeps the union honest if a kind is added to the
// task provider but not wired here.
export type _WiredTaskKinds = TaskKind;
