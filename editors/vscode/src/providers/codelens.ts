import * as vscode from "vscode";
import { ruleLensInfos, rulesUpTo } from "../core/ruleIndex";

/**
 * CodeLens over `[[rules]]` headers: one-click "run this rule" (single
 * `-t <name>`, which the CLI resolves to the rule + its upstream closure)
 * and "run to here" (repeated `-t` for every rule at or before this one in
 * file order).
 */
export class OxoflowCodeLensProvider implements vscode.CodeLensProvider {
  provideCodeLenses(document: vscode.TextDocument): vscode.CodeLens[] {
    const infos = ruleLensInfos(document.getText());
    const lenses: vscode.CodeLens[] = [];
    for (const info of infos) {
      const header = new vscode.Range(info.headerLine, 0, info.headerLine, 0);
      lenses.push(
        new vscode.CodeLens(header, {
          title: "▶ Run this rule",
          command: "oxo-flow.runTargets",
          arguments: [[info.name], info.name],
        }),
        new vscode.CodeLens(header, {
          title: "▶ Run to here",
          command: "oxo-flow.runTargets",
          arguments: [rulesUpTo(infos, info.name), info.name],
        })
      );
    }
    return lenses;
  }
}
