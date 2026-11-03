import * as vscode from "vscode";
import { dryRunArgs, graphArgs, resumeArgs, runArgs, templateArgs, type GraphFormat } from "../core/cliArgs";

export const TASK_TYPE = "oxo-flow";

export interface OxoflowTaskDefinition extends vscode.TaskDefinition {
  workflow: string;
  kind?: TaskKind;
  target?: string;
  jobs?: number;
  keepGoing?: boolean;
  /** `graph` tasks only: output format (`-f`, ascii is the CLI default). */
  format?: GraphFormat;
  extraArgs?: string[];
}

export type TaskKind = "run" | "dry-run" | "graph" | "resume" | "generate";

/**
 * Task integration: `oxo-flow` tasks show up in the task list and run in the
 * terminal panel with proper lifecycle (rerun, cancellation). The commands
 * reuse the same factory so Run/Dry-Run/Graph always behave like tasks.
 */
export class OxoflowTaskProvider implements vscode.TaskProvider {
  constructor(private readonly executable: () => string) {}

  async provideTasks(): Promise<vscode.Task[]> {
    const folders = vscode.workspace.workspaceFolders ?? [];
    const tasks: vscode.Task[] = [];
    for (const folder of folders) {
      const files = await vscode.workspace.findFiles(
        new vscode.RelativePattern(folder, "**/*.oxoflow"),
        "**/node_modules/**",
        20
      );
      for (const file of files.slice(0, 10)) {
        tasks.push(
          createTask(
            this.executable(),
            folder,
            {
              type: TASK_TYPE,
              workflow: relativeWorkflow(folder, file),
            },
            "run"
          )
        );
      }
    }
    return tasks;
  }

  resolveTask(task: vscode.Task): vscode.Task | undefined {
    const def = task.definition as OxoflowTaskDefinition;
    if (!def?.workflow) return undefined;
    const folder = resolveFolder(vscode.Uri.file(def.workflow));
    if (!folder) return undefined;
    // Tasks saved in tasks.json predate the `kind` field — fall back to the
    // legacy name-prefix sniffing ("dry-run …") for those.
    return createTask(
      this.executable(),
      folder,
      def,
      def.kind ?? (task.name.startsWith("dry-run") ? "dry-run" : "run")
    );
  }
}

export function resolveFolder(uri: vscode.Uri): vscode.WorkspaceFolder | undefined {
  return vscode.workspace.getWorkspaceFolder(uri) ?? vscode.workspace.workspaceFolders?.[0];
}

export function relativeWorkflow(folder: vscode.WorkspaceFolder, file: vscode.Uri): string {
  const folderPath = folder.uri.fsPath;
  const filePath = file.fsPath;
  // A bare prefix match treats sibling folders sharing a name prefix
  // ("proj" vs "proj-archive") as inside the workspace — require a path
  // separator after the folder (a trailing separator in folderPath counts).
  const endsWithSep = folderPath.endsWith("/") || folderPath.endsWith("\\");
  const inside = endsWithSep
    ? filePath.startsWith(folderPath)
    : filePath.startsWith(`${folderPath}/`) || filePath.startsWith(`${folderPath}\\`);
  if (inside) {
    const rel = filePath.slice(folderPath.length).replace(/^[/\\]/, "");
    return rel.length > 0 ? rel : filePath;
  }
  return filePath;
}

export function createTask(
  executable: string,
  folder: vscode.WorkspaceFolder,
  def: OxoflowTaskDefinition,
  kind: TaskKind,
  overrideArgs?: string[],
  /** Display name override (e.g. "clean …" for tasks that ride on `run`). */
  nameOverride?: string
): vscode.Task {
  const runOpts = {
    file: def.workflow,
    jobs: def.jobs,
    keepGoing: def.keepGoing,
    targets: def.target ? [def.target] : [],
    extraArgs: def.extraArgs,
  };
  const args =
    overrideArgs ??
    (kind === "run"
      ? runArgs(runOpts)
      : kind === "dry-run"
        // CLI dry-run takes neither `-j` nor `-k` (see dryRunArgs) — jobs and
        // keepGoing on the shared task definition are honored for `run` only.
        ? dryRunArgs(runOpts)
        : kind === "resume"
          ? resumeArgs(def.workflow, def.extraArgs)
          : kind === "generate"
            // Documented contract (vscode-extension.md): a tasks.json
            // `generate` task runs AI template generation with extraArgs[0]
            // as the description — without this branch it fell through to
            // `graph` and silently produced a DAG instead.
            ? templateArgs(def.extraArgs?.[0] ?? "", def.workflow)
            : graphArgs(def.workflow, def.format));
  // ShellQuotedString is a plain object interface, not a constructor: pass
  // {value, quoting} literals so no shell metacharacter in paths/args breaks.
  const strong = (value: string) => ({ value, quoting: vscode.ShellQuoting.Strong });
  const execution = new vscode.ShellExecution(strong(executable), args.map(strong), {
    cwd: folder.uri.fsPath,
  });
  const name = nameOverride ?? `${kind} ${def.workflow}`;
  const task = new vscode.Task(def, folder, name, TASK_TYPE, execution, []);
  task.presentationOptions = { reveal: vscode.TaskRevealKind.Always, panel: vscode.TaskPanelKind.Dedicated, clear: true };
  return task;
}
