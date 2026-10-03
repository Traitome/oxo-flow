import * as vscode from "vscode";
import { contextKeys, enumFor } from "../core/completionData";
import { resolveEditContext } from "../core/editContext";

/** Hover docs for schema-known keys and enum values. */
export class OxoflowHoverProvider implements vscode.HoverProvider {
  provideHover(document: vscode.TextDocument, position: vscode.Position): vscode.Hover | undefined {
    const word = document.getWordRangeAtPosition(position, /[A-Za-z0-9_.-]+/);
    if (!word) return undefined;
    const wordText = document.getText(word);

    const { ctx } = resolveEditContext(document.getText(), position.line, position.character);
    let info: { key: string; description?: string; type?: string; enums?: string[] } | undefined;
    let title = wordText;

    switch (ctx.kind) {
      case "key":
        info = contextKeys(ctx.context || "root").find((k) => k.key === wordText);
        break;
      case "inline-table-key": {
        const inline = ctx.context === "rules" ? `rules.${ctx.tableKey}` : null;
        info = inline ? contextKeys(inline).find((k) => k.key === wordText) : undefined;
        break;
      }
      case "value":
      case "inline-table-value": {
        const lookup = ctx.kind === "value" ? ctx.context : `rules.${ctx.tableKey}`;
        const key = ctx.kind === "value" ? ctx.key : ctx.key;
        const enums = enumFor(lookup, key);
        if (enums?.includes(wordText)) {
          info = { key, type: "enum value", enums };
        }
        break;
      }
      default:
        return undefined;
    }

    if (!info) return undefined;
    const md = new vscode.MarkdownString();
    md.appendMarkdown(`**${title}**`);
    if (info.type) md.appendMarkdown(` — \`${info.type}\``);
    if (info.enums?.length) {
      md.appendMarkdown("\n\nAllowed values: " + info.enums.map((e) => `\`${e}\``).join(", "));
    }
    if (info.description) md.appendMarkdown(`\n\n${info.description}`);
    return new vscode.Hover(md, word);
  }
}
