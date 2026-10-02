import * as vscode from "vscode";
import { dryRunArgs, graphArgs, resumeArgs, runArgs } from "../core/cliArgs";

export const TASK_TYPE = "oxo-flow";

export interface OxoflowTaskDefinition extends vscode.TaskDefinition {
  workflow: string;
  target?: string;
  jobs?: number;
  keepGoing?: boolean;
  extraArgs?: string[];
}

export type TaskKind = "run" | "dry-run" | "graph" | "resume";

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
    const folder =
      vscode.workspace.getWorkspaceFolder(vscode.Uri.file(def.workflow)) ??
      vscode.workspace.workspaceFolders?.[0];
    if (!folder) return undefined;
    return createTask(
      this.executable(),
      folder,
      def,
      (task.name.startsWith("dry-run") ? "dry-run" : "run") as TaskKind
    );
  }
}

export function relativeWorkflow(folder: vscode.WorkspaceFolder, file: vscode.Uri): string {
  const folderPath = folder.uri.fsPath;
  const filePath = file.fsPath;
  if (filePath.startsWith(folderPath)) {
    const rel = filePath.slice(folderPath.length).replace(/^[/\\]/, "");
    return rel.length > 0 ? rel : filePath;
  }
  return filePath;
}

export function createTask(
  executable: string,
  folder: vscode.WorkspaceFolder,
  def: OxoflowTaskDefinition,
  kind: TaskKind
): vscode.Task {
  const runOpts = {
    file: def.workflow,
    jobs: def.jobs,
    keepGoing: def.keepGoing,
    targets: def.target ? [def.target] : [],
    extraArgs: def.extraArgs,
  };
  const args =
    kind === "run"
      ? runArgs(runOpts)
      : kind === "dry-run"
        ? dryRunArgs(runOpts)
        : kind === "resume"
          ? resumeArgs(def.workflow, def.extraArgs)
          : graphArgs(def.workflow);
  // ShellQuotedString is a plain object interface, not a constructor: pass
  // {value, quoting} literals so no shell metacharacter in paths/args breaks.
  const strong = (value: string) => ({ value, quoting: vscode.ShellQuoting.Strong });
  const execution = new vscode.ShellExecution(strong(executable), args.map(strong), {
    cwd: folder.uri.fsPath,
  });
  const name = `${kind} ${def.workflow}`;
  const task = new vscode.Task(def, folder, name, TASK_TYPE, execution, []);
  task.presentationOptions = { reveal: vscode.TaskRevealKind.Always, panel: vscode.TaskPanelKind.Dedicated, clear: true };
  return task;
}
