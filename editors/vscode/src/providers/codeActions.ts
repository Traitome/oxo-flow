import * as vscode from "vscode";

const REVALIDATE_TITLE = "oxo-flow: Re-validate this file";
const SET_PATH_TITLE = "oxo-flow: Set CLI executable path";
const REPORT_TITLE = "oxo-flow: Report Issue with this diagnostic";

/**
 * Quick actions on oxo-flow diagnostics: re-run the gates after editing,
 * jump to the executable-path setting when the CLI could not be run (the
 * unparseable-output fallback), and attach any diagnostic to a pre-filled
 * issue report.
 */
export class OxoflowCodeActionProvider implements vscode.CodeActionProvider {
  static readonly providedKinds = [vscode.CodeActionKind.QuickFix];

  provideCodeActions(
    _document: vscode.TextDocument,
    _range: vscode.Range | vscode.Selection,
    context: vscode.CodeActionContext
  ): vscode.CodeAction[] {
    const actions: vscode.CodeAction[] = [];
    const seen = new Set<string>();
    for (const diag of context.diagnostics) {
      if (diag.source !== "oxo-flow") continue;
      const key = `${String(diag.code)}:${diag.message}`;
      if (seen.has(key)) continue;
      seen.add(key);

      const revalidate = new vscode.CodeAction(REVALIDATE_TITLE, vscode.CodeActionKind.QuickFix);
      revalidate.command = { command: "oxo-flow.validate", title: REVALIDATE_TITLE };
      actions.push(revalidate);

      const report = new vscode.CodeAction(REPORT_TITLE, vscode.CodeActionKind.QuickFix);
      report.command = {
        command: "oxo-flow.reportIssue",
        title: REPORT_TITLE,
        arguments: [{ lastError: `${String(diag.code)}: ${diag.message}` }],
      };
      actions.push(report);

      if (diag.message.includes("could not parse")) {
        const setPath = new vscode.CodeAction(SET_PATH_TITLE, vscode.CodeActionKind.QuickFix);
        setPath.command = { command: "oxo-flow.openSettings", title: SET_PATH_TITLE };
        actions.push(setPath);
      }
    }
    return actions;
  }
}
