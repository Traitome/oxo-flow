import * as vscode from "vscode";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { formatArgs } from "../core/cliArgs";
import { runCli } from "../core/exec";

/**
 * Formatting via the engine's canonical TOML formatter (`oxo-flow format`).
 * The buffer (not the disk file) is formatted: content goes to a temp copy so
 * unsaved documents work and the original file is never touched directly.
 */
export class OxoflowFormattingProvider implements vscode.DocumentFormattingEditProvider {
  async provideDocumentFormattingEdits(
    document: vscode.TextDocument,
    _options: vscode.FormattingOptions,
    token: vscode.CancellationToken
  ): Promise<vscode.TextEdit[] | undefined> {
    const cfg = vscode.workspace.getConfiguration("oxo-flow", document.uri);
    const executable = cfg.get<string>("executablePath", "oxo-flow");

    const dir = await mkdtemp(join(tmpdir(), "oxoflow-format-"));
    const input = join(dir, "in.oxoflow");
    const output = join(dir, "out.oxoflow");
    try {
      await writeFile(input, document.getText(), "utf8");
      if (token.isCancellationRequested) return undefined;
      const res = await runCli(executable, formatArgs(input, output), { cwd: dir, timeoutMs: 30_000 });
      if (res.spawnError) {
        throw new Error(`cannot run ${executable}: ${res.spawnError}`);
      }
      if (res.exitCode !== 0) {
        throw new Error(`oxo-flow format failed (exit ${res.exitCode}): ${tail(res.stderr)}`);
      }
      const formatted = await readFile(output, "utf8");
      if (formatted === document.getText()) return [];
      const fullRange = new vscode.Range(
        document.positionAt(0),
        document.positionAt(document.getText().length)
      );
      return [vscode.TextEdit.replace(fullRange, formatted)];
    } catch (e) {
      void vscode.window.showErrorMessage(
        `oxo-flow format: ${e instanceof Error ? e.message : String(e)}`
      );
      return undefined;
    } finally {
      await rm(dir, { recursive: true, force: true }).catch(() => undefined);
    }
  }
}

function tail(text: string, lines = 3): string {
  const parts = text.split("\n").filter(Boolean);
  return parts.slice(-lines).join(" ");
}
