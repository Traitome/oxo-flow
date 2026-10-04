import * as vscode from "vscode";
import {
  aiStatusArgs,
  cleanArgs,
  schemaArgs,
  statusArgs,
  templateArgs,
  validateArgs,
  lintArgs,
  versionArgs,
} from "./core/cliArgs";
import { runCli } from "./core/exec";
import { runArgs, type GraphFormat } from "./core/cliArgs";
import { parseReport } from "./core/jsonReport";
import {
  buildIssueBody,
  buildIssueUrl,
  ISSUE_NEW_URL,
  type ReportContext,
  type ReportEnvironment,
} from "./core/reportIssue";
import { buildGenerationPrompt, extractToml, resolveBackend } from "./core/aiBackend";
import { OxoflowCodeLensProvider } from "./providers/codelens";
import { OxoflowCodeActionProvider } from "./providers/codeActions";
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

/**
 * Structural subset of vscode.LanguageModelChat relied upon for the `ide`
 * AI backend. Casts (not type imports) keep the code loadable in editors
 * whose language-model API surface differs (VSCodium, forks).
 */
interface IdeLanguageModel {
  readonly family: string;
  readonly vendor: string;
  sendRequest(
    messages: vscode.LanguageModelChatMessage[],
    options?: Record<string, unknown>
  ): { text: AsyncIterable<string> };
}

// Rolling tail of everything written to the output channel — Report Issue
// attaches the last lines so bug reports carry the failing CLI output.
const LOG_TAIL_LINES = 60;
const logTail: string[] = [];
let extensionVersion = "unknown";

function appendLog(channel: vscode.OutputChannel, text: string): void {
  channel.appendLine(text);
  logTail.push(...text.split("\n"));
  while (logTail.length > LOG_TAIL_LINES) logTail.shift();
}

/** Error toast that always offers a one-click, context-aware issue report. */
async function showErrorWithReport(message: string): Promise<void> {
  const pick = await vscode.window.showErrorMessage(message, "Report Issue");
  if (pick === "Report Issue") {
    void vscode.commands.executeCommand("oxo-flow.reportIssue", { lastError: message } satisfies ReportContext);
  }
}

async function withOxoProgress<T>(title: string, task: () => Promise<T>): Promise<T> {
  return vscode.window.withProgress({ location: vscode.ProgressLocation.Notification, title }, task);
}

export function activate(context: vscode.ExtensionContext): void {
  extensionVersion = String(context.extension.packageJSON.version ?? "unknown");

  const output = vscode.window.createOutputChannel("oxo-flow");
  context.subscriptions.push(output);

  const statusBar = new StatusBar();
  statusBar.activate(context);

  const diagnostics = new OxoflowDiagnostics(output);
  diagnostics.activate(context);

  // External .oxoflow changes (git checkout, teammates, generators) do not
  // fire editor events: refresh the CLI probe and re-diagnose any open copy.
  const watcher = vscode.workspace.createFileSystemWatcher("**/*.oxoflow");
  const reDiagnose = (uri: vscode.Uri): void => {
    const doc = vscode.workspace.textDocuments.find((d) => d.uri.toString() === uri.toString());
    if (doc) void diagnostics.diagnose(doc);
  };
  watcher.onDidChange((uri) => {
    void statusBar.refresh();
    reDiagnose(uri);
  });
  watcher.onDidCreate((uri) => {
    void statusBar.refresh();
    reDiagnose(uri);
  });
  context.subscriptions.push(watcher);

  const taskProvider = new OxoflowTaskProvider(() => statusBar.executable());
  context.subscriptions.push(
    vscode.tasks.registerTaskProvider("oxo-flow", taskProvider),
    vscode.languages.registerCompletionItemProvider("oxoflow", new OxoflowCompletionProvider(), "[", "{", ".", '"'),
    vscode.languages.registerHoverProvider("oxoflow", new OxoflowHoverProvider()),
    vscode.languages.registerDocumentFormattingEditProvider("oxoflow", new OxoflowFormattingProvider()),
    vscode.languages.registerCodeLensProvider("oxoflow", new OxoflowCodeLensProvider()),
    vscode.languages.registerCodeActionsProvider(
      "oxoflow",
      new OxoflowCodeActionProvider(),
      { providedCodeActionKinds: OxoflowCodeActionProvider.providedKinds }
    )
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

  // Auto-open the DAG graph after a successful pipeline run when
  // `oxo-flow.autoOpenGraph` is enabled. The format is the last one picked
  // via "Show DAG Graph" (default ASCII) so the flow stays hands-off.
  context.subscriptions.push(
    vscode.tasks.onDidEndTaskProcess((event) => {
      if (event.exitCode !== 0) return;
      const runDef = event.execution.task.definition as OxoflowTaskDefinition;
      if (runDef?.type !== "oxo-flow" || (runDef.kind ?? "run") !== "run") return;
      const cfg = vscode.workspace.getConfiguration("oxo-flow");
      if (!cfg.get<boolean>("autoOpenGraph", false)) return;
      const folder = folderFor(vscode.Uri.file(runDef.workflow));
      if (!folder) return;
      const format = context.workspaceState.get<GraphFormat>("graphFormat") ?? "ascii";
      const graphDef: OxoflowTaskDefinition = { type: "oxo-flow", workflow: runDef.workflow, kind: "graph", format };
      void vscode.tasks.executeTask(createTask(statusBar.executable(), folder, graphDef, "graph"));
    })
  );

  const register = (id: string, fn: (...args: unknown[]) => unknown) => {
    context.subscriptions.push(vscode.commands.registerCommand(id, fn));
  };

  register("oxo-flow.run", () => executePipelineTask(context, statusBar, "run"));
  register("oxo-flow.runTargets", (...args: unknown[]) => executeRunTargets(statusBar, args[0]));
  register("oxo-flow.dryRun", () => executePipelineTask(context, statusBar, "dry-run"));
  register("oxo-flow.graph", () => executePipelineTask(context, statusBar, "graph"));
  register("oxo-flow.validate", () => runQualityCommand(output, statusBar, "validate"));
  register("oxo-flow.lint", () => runQualityCommand(output, statusBar, "lint"));
  register("oxo-flow.format", () => formatDocument());
  register("oxo-flow.resume", () => resumePipeline(statusBar));
  register("oxo-flow.generate", () => generateWithAI(statusBar, context));
  register("oxo-flow.aiStatus", () => showAiStatus(output, statusBar));
  register("oxo-flow.exportSchema", () => exportSchema(statusBar));
  register("oxo-flow.clean", () => cleanOutputs(output, statusBar));
  register("oxo-flow.status", () => showRunStatus(output, statusBar));
  register("oxo-flow.openDocs", () => vscode.env.openExternal(vscode.Uri.parse(DOCS_URL)));
  register("oxo-flow.openSettings", () =>
    vscode.commands.executeCommand("workbench.action.openSettings", "oxo-flow.executablePath")
  );
  register("oxo-flow.pickCommand", () => pickCommand());
  register("oxo-flow.reportIssue", (...args: unknown[]) => {
    const ctx = (args[0] ?? {}) as ReportContext;
    return reportIssue(statusBar, ctx);
  });
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
  extContext: vscode.ExtensionContext,
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
  if (kind === "graph") {
    const picked = await pickGraphFormat(extContext);
    if (!picked) return;
    def.format = picked;
  }
  await vscode.tasks.executeTask(createTask(statusBar.executable(), target.folder, def, kind));
}

const GRAPH_FORMATS: { label: string; format: GraphFormat; description?: string; detail: string }[] = [
  { label: "ASCII DAG", format: "ascii", description: "default", detail: "Terminal DAG with level grouping and metrics" },
  { label: "Mermaid", format: "mermaid", description: "graph LR", detail: "Paste into GitHub / VS Code markdown preview" },
  { label: "DOT", format: "dot", description: "needs Graphviz", detail: "Graphviz digraph, machine-consumable" },
  { label: "DOT (clustered)", format: "dot-clustered", description: "needs Graphviz", detail: "Graphviz digraph grouped into level clusters" },
  { label: "Dependency tree", format: "tree", detail: "Indented dependency tree" },
  { label: "Metro map", format: "metro", description: "nf-metro", detail: "Transit-map directives (%%metro) for nf-metro render" },
];

/** Quick pick over `oxo-flow graph -f` formats; remembers the last choice. */
async function pickGraphFormat(extContext: vscode.ExtensionContext): Promise<GraphFormat | undefined> {
  const previous = extContext.workspaceState.get<GraphFormat>("graphFormat");
  const ordered = previous ? [...GRAPH_FORMATS].sort((a, b) => (a.format === previous ? -1 : b.format === previous ? 1 : 0)) : GRAPH_FORMATS;
  const pick = await vscode.window.showQuickPick(ordered, {
    placeHolder: "Graph format (oxo-flow graph -f)",
    matchOnDescription: true,
  });
  if (!pick) return undefined;
  if (pick.format !== previous) void extContext.workspaceState.update("graphFormat", pick.format);
  return pick.format;
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
  const res = await withOxoProgress(`oxo-flow: ${kind}…`, () =>
    runCli(executable, args, { cwd: target.folder.uri.fsPath })
  );

  if (res.spawnError) {
    void showErrorWithReport(
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
  appendLog(output, lines.join("\n"));
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

async function generateWithAI(statusBar: StatusBar, extContext: vscode.ExtensionContext): Promise<void> {
  const description = await vscode.window.showInputBox({
    prompt: "Describe the pipeline to generate (e.g. 'RNA-seq with STAR and featureCounts')",
    placeHolder: "Natural-language description",
  });
  if (!description) return;
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    void showErrorWithReport("Open a workspace folder first — the pipeline needs a home.");
    return;
  }

  const cfg = vscode.workspace.getConfiguration("oxo-flow", folder.uri);
  const configured = cfg.get<"auto" | "cli" | "ide">("ai.backend", "auto");
  // Feature-detect the language-model API: forks (VSCodium, some Trae builds)
  // omit it entirely, and Copilot-signed stock VS Code is where it yields.
  const lm = vscode.lm as unknown as { selectChatModels?: (q: unknown) => Promise<unknown[]> } | undefined;
  const ideModels =
    typeof lm?.selectChatModels === "function"
      ? ((await lm.selectChatModels({})) as unknown[])
      : [];
  const resolution = resolveBackend(configured, {
    cliAiConfigured: await cliAiConfigured(statusBar),
    ideModelsAvailable: ideModels.length > 0,
  });
  if (resolution.kind === "unavailable") {
    void showErrorWithReport(resolution.reason);
    return;
  }
  if (resolution.backend === "cli") {
    await generateWithCli(statusBar, folder, description);
    return;
  }
  await generateWithIdeModel(extContext, statusBar, folder, description, resolution.reason);
}

/** Best-effort CLI AI probe: `oxo-flow ai` JSON reports the active provider. */
async function cliAiConfigured(statusBar: StatusBar): Promise<boolean> {
  const res = await runCli(statusBar.executable(), aiStatusArgs(), { timeoutMs: 15_000 });
  if (res.spawnError) return false;
  try {
    const parsed = JSON.parse(res.stdout) as { provider?: string };
    return Boolean(parsed.provider) && parsed.provider !== "disabled";
  } catch {
    return res.stdout.trim().length > 0 && !/disabled/i.test(res.stdout);
  }
}

/** CLI engine path: the eval-tuned generation loop runs as a task. */
async function generateWithCli(statusBar: StatusBar, folder: vscode.WorkspaceFolder, description: string): Promise<void> {
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

/**
 * Editor language-model path (zero-config): generate with the picked model,
 * write the file, then ground it through `oxo-flow validate` with ONE
 * repair round feeding the findings back to the model.
 */
async function generateWithIdeModel(
  extContext: vscode.ExtensionContext,
  statusBar: StatusBar,
  folder: vscode.WorkspaceFolder,
  description: string,
  via: string
): Promise<void> {
  const models = (await (
    vscode.lm as unknown as { selectChatModels: (q: unknown) => Promise<IdeLanguageModel[]> }
  ).selectChatModels({}));
  const picks = models.map((m) => ({ label: `${m.family} — ${m.vendor}`, model: m }));
  const pick =
    picks.length === 1
      ? picks[0]
      : await vscode.window.showQuickPick(picks, { placeHolder: "Language model for this generation" });
  if (!pick) return;
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

  const output = vscode.window.createOutputChannel("oxo-flow");
  extContext.subscriptions.push(output);
  await withOxoProgress("oxo-flow: generating pipeline via the editor model…", async () => {
    const prompt = buildGenerationPrompt(description);
    const chat = async (messages: vscode.LanguageModelChatMessage[]): Promise<string> => {
      const request = await pick.model.sendRequest(messages, {});
      let text = "";
      for await (const chunk of request.text) text += chunk;
      return text;
    };

    let response = await chat([vscode.LanguageModelChatMessage.User(prompt)]);
    appendLog(output, `[ai:ide:${pick.model.family}] ${response.slice(0, 500)}`);
    let toml = extractToml(response);
    if (!toml) {
      void showErrorWithReport("The model response contained no [workflow] pipeline — see the oxo-flow output panel.");
      output.show(true);
      return;
    }
    await vscode.workspace.fs.writeFile(target, Buffer.from(toml, "utf8"));

    // Grounding gate: the engine validates every draft; on failure, feed the
    // findings back for exactly one repair round before surfacing the result.
    const verdict = await runCli(statusBar.executable(), validateArgs(target.fsPath), {
      cwd: folder.uri.fsPath,
    });
    if (verdict.exitCode !== 0) {
      appendLog(output, "[ai:ide] draft failed validation — one repair round");
      const findings = `${verdict.stderr.trim() || verdict.stdout.trim()}`.slice(0, 2000);
      const repaired = await chat([
        vscode.LanguageModelChatMessage.User(prompt),
        vscode.LanguageModelChatMessage.Assistant(response),
        vscode.LanguageModelChatMessage.User(
          `That pipeline failed \`oxo-flow validate\`:\n\n${findings}\n\nReturn the corrected COMPLETE pipeline as one fenced toml block.`
        ),
      ]);
      const fixed = extractToml(repaired);
      if (fixed) {
        toml = fixed;
        await vscode.workspace.fs.writeFile(target, Buffer.from(fixed, "utf8"));
      }
    }
    void vscode.window.showInformationMessage(
      `Pipeline written to ${vscode.workspace.asRelativePath(target)} (via ${via}).`
    );
  });
}

async function showAiStatus(output: vscode.OutputChannel, statusBar: StatusBar): Promise<void> {
  const executable = statusBar.executable();
  const res = await withOxoProgress("oxo-flow: reading AI status…", () =>
    runCli(executable, aiStatusArgs(), { timeoutMs: 30_000 })
  );
  if (res.spawnError) {
    void showErrorWithReport(`Cannot run \`${executable}\` (${res.spawnError}).`);
    return;
  }
  appendLog(output, `$ oxo-flow ${aiStatusArgs().join(" ")}\n${res.stdout.trimEnd()}\n${res.stderr.trimEnd()}`);
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
  const res = await withOxoProgress("oxo-flow: exporting schema…", () =>
    runCli(executable, schemaArgs(), { cwd: folder.uri.fsPath })
  );
  if (res.spawnError || res.exitCode !== 0) {
    void showErrorWithReport(`oxo-flow schema failed: ${res.spawnError ?? res.stderr}`);
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

  const preview = await withOxoProgress("oxo-flow: previewing clean…", () =>
    runCli(executable, cleanArgs(target.file.fsPath, { dryRun: true }), {
      cwd: target.folder.uri.fsPath,
    })
  );
  if (preview.spawnError) {
    void showErrorWithReport(
      `Cannot run \`${executable}\` (${preview.spawnError}). Set oxo-flow.executablePath in settings.`
    );
    return;
  }
  appendLog(output, `$ oxo-flow clean ${JSON.stringify(rel)} -n\n${preview.stderr.trimEnd()}`);
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
        cleanArgs(target.file.fsPath, { force: true, orphans: true }),
        `clean (orphans) ${rel}`
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
      cleanArgs(target.file.fsPath, { force: true }),
      `clean ${rel}`
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
  const res = await withOxoProgress("oxo-flow: reading run status…", () =>
    runCli(executable, args, { cwd: folder.uri.fsPath })
  );
  if (res.spawnError) {
    void showErrorWithReport(`Cannot run \`${executable}\` (${res.spawnError}).`);
    appendLog(output, `Cannot run \`${executable}\` (${res.spawnError}).`);
    output.show(true);
    return;
  }
  appendLog(output, `$ oxo-flow ${args.join(" ")}\n${res.stdout.trimEnd()}${res.stderr.trimEnd()}`);
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
  { label: "$(feedback) Report Issue…", command: "oxo-flow.reportIssue" },
];

/**
 * Collect a sanitized, pre-filled GitHub issue and open it in the browser.
 * Pipeline content is never attached — only environment facts, the
 * extension's own settings, and the output-channel tail (home paths masked).
 */
async function reportIssue(statusBar: StatusBar, context: ReportContext): Promise<void> {
  const cfg = vscode.workspace.getConfiguration("oxo-flow");
  const executable = statusBar.executable();
  let cliVersion: string | undefined;
  await withOxoProgress("oxo-flow: preparing the issue report…", async () => {
    const probe = await runCli(executable, versionArgs(), { timeoutMs: 3000 });
    if (!probe.spawnError) cliVersion = probe.stdout.trim().split(/\s+/).pop();
  });
  const env: ReportEnvironment = {
    extensionVersion,
    vscodeVersion: vscode.version,
    appName: vscode.env.appName,
    appHost: vscode.env.appHost ?? "desktop",
    remoteName: vscode.env.remoteName,
    platform: process.platform,
    arch: process.arch,
    cliExecutable: executable,
    cliVersion,
    settings: {
      "oxo-flow.executablePath": cfg.get("executablePath", "oxo-flow"),
      "oxo-flow.diagnosticMode": cfg.get("diagnosticMode", "save"),
      "oxo-flow.enableLintDiagnostics": cfg.get("enableLintDiagnostics", true),
      "oxo-flow.formatOnSave": cfg.get("formatOnSave", false),
      "oxo-flow.autoOpenGraph": cfg.get("autoOpenGraph", false),
      "oxo-flow.runArgs": cfg.get("runArgs", []),
    },
  };
  const body = buildIssueBody(env, { lastError: context.lastError, outputTail: logTail.join("\n") });
  const url = buildIssueUrl(body);
  if (url) {
    // Pass the URL as a plain string: openExternal keeps strings verbatim,
    // while Uri.parse re-decodes the query and the open pipeline re-encodes
    // it — `###` reached GitHub as literal `%23%23%23` and the body was
    // truncated at the first `&` (verified in VS Code 1.99.3 on macOS).
    // String targets are supported by the runtime since VS Code 1.90; the
    // @types signature has not caught up.
    await vscode.env.openExternal(url as unknown as vscode.Uri);
    return;
  }
  // Oversized report: GitHub would drop the query string — clipboard fallback.
  await vscode.env.clipboard.writeText(body);
  void vscode.window.showInformationMessage(
    "The report exceeded the URL limit — it was copied to your clipboard. Paste it into the new issue."
  );
  await vscode.env.openExternal(vscode.Uri.parse(ISSUE_NEW_URL));
}

async function pickCommand(): Promise<void> {
  const pick = await vscode.window.showQuickPick(COMMAND_PICKS, { placeHolder: "oxo-flow command" });
  if (pick) await vscode.commands.executeCommand(pick.command);
}

// TaskKind re-export guard: keeps the union honest if a kind is added to the
// task provider but not wired here.
export type _WiredTaskKinds = TaskKind;
