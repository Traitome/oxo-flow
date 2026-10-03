import * as vscode from "vscode";
import { versionArgs } from "../core/cliArgs";
import { runCli } from "../core/exec";

/**
 * Status bar: shows CLI availability + version, click opens a command quick
 * pick. The version probe is cached until the executable path changes; a
 * newer probe always supersedes an older in-flight one.
 */
export class StatusBar implements vscode.Disposable {
  private readonly item: vscode.StatusBarItem;
  private probeSeq = 0;

  constructor() {
    this.item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);
    this.item.name = "oxo-flow";
    this.item.command = "oxo-flow.pickCommand";
    this.item.text = "$(circle-filled) oxo-flow";
    this.item.tooltip = new vscode.MarkdownString("Resolving `oxo-flow` CLI…");
  }

  activate(context: vscode.ExtensionContext): void {
    this.item.show();
    context.subscriptions.push(this.item, this);
    void this.refresh();
    context.subscriptions.push(
      vscode.workspace.onDidChangeConfiguration((e) => {
        if (e.affectsConfiguration("oxo-flow.executablePath")) void this.refresh();
      })
    );
  }

  async refresh(): Promise<void> {
    const executable = this.executable();
    const seq = ++this.probeSeq;
    const res = await runCli(executable, versionArgs(), { timeoutMs: 10_000 });
    if (seq !== this.probeSeq) return; // superseded by a newer probe
    if (res.spawnError) {
      this.item.text = "$(warning) oxo-flow";
      const md = new vscode.MarkdownString();
      md.appendMarkdown(
        `\`${executable}\` was not found on this machine.\n\n` +
          "Set **oxo-flow.executablePath** to the binary location (e.g. `~/.cargo/bin/oxo-flow`)."
      );
      this.item.tooltip = md;
      this.item.backgroundColor = new vscode.ThemeColor("statusBarItem.warningBackground");
      return;
    }
    const version = res.stdout.trim().split(/\s+/).pop() ?? "unknown";
    this.item.text = "$(circle-filled) oxo-flow";
    const md = new vscode.MarkdownString();
    md.appendMarkdown(`**oxo-flow** \`${version}\`\n\n\`${executable}\`\n\nClick for pipeline commands.`);
    this.item.tooltip = md;
    this.item.backgroundColor = undefined;
  }

  executable(): string {
    // Resource-scoped read so a folder-level override (multi-root, SSH remote)
    // wins; falls back to the merged view when no editor context exists.
    const uri = vscode.window.activeTextEditor?.document.uri;
    return vscode.workspace
      .getConfiguration("oxo-flow", uri)
      .get<string>("executablePath", "oxo-flow");
  }

  dispose(): void {
    this.item.dispose();
  }
}
