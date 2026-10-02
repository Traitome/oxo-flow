import * as vscode from "vscode";
import {
  contextKeys,
  enumFor,
  insertionFor,
  type PropInfo,
} from "../core/completionData";
import { resolveEditContext } from "../core/editContext";

/** Completion provider driven by the generated schema data + document index. */
export class OxoflowCompletionProvider implements vscode.CompletionItemProvider {
  provideCompletionItems(
    document: vscode.TextDocument,
    position: vscode.Position
  ): vscode.CompletionItem[] | undefined {
    const { ctx, index } = resolveEditContext(document.getText(), position.line, position.character);

    switch (ctx.kind) {
      case "none":
      case "comment":
      case "multiline-string":
        return undefined;
      case "header":
        return headerItems(ctx.typed, position);
      case "key":
        return keyItems(ctx.context, ctx.typed, index, position, document);
      case "inline-table-key":
        return inlineTableKeyItems(ctx.context, ctx.tableKey, ctx.typed, ctx.usedKeys, position);
      case "value":
        return valueItems(ctx.context, ctx.key, ctx.typed, index, position);
      case "inline-table-value":
        return valueItems(`rules.${ctx.tableKey}`, ctx.key, ctx.typed, index, position);
    }
  }
}

function filterByTyped<T extends { key: string }>(items: T[], typed: string): T[] {
  const t = typed.replace(/^[{[]+/, "").toLowerCase();
  if (!t) return items;
  return items.filter((i) => i.key.toLowerCase().includes(t));
}

function headerItems(typed: string, position: vscode.Position): vscode.CompletionItem[] {
  return filterByTyped(contextKeys("root"), typed)
    .filter((info) => info.key === "rules" || info.type === "object" || info.ref)
    .map((info) => {
      const item = new vscode.CompletionItem(info.key, vscode.CompletionItemKind.Module);
      item.insertText = new vscode.SnippetString(
        insertionFor(info).startsWith("[[") ? `[[${info.key}]]\n$0` : `[${info.key}]\n$0`
      );
      item.detail = info.type === "array" ? "array of tables" : "table";
      if (info.description) item.documentation = new vscode.MarkdownString(info.description);
      item.range = wordRangeAt(position);
      return item;
    });
}

function keyItems(
  context: string,
  typed: string,
  index: ReturnType<typeof resolveEditContext>["index"],
  position: vscode.Position,
  document: vscode.TextDocument
): vscode.CompletionItem[] {
  const contextName = context === "" ? "root" : context;
  const used = usedKeysInContext(index, document, position);
  return filterByTyped(contextKeys(contextName), typed)
    .filter((info) => info.key === "name" || !used.includes(info.key))
    .map((info) => {
      const isTable = info.key === "rules" || info.type === "object" || info.ref;
      const item = new vscode.CompletionItem(
        info.key,
        isTable ? vscode.CompletionItemKind.Module : vscode.CompletionItemKind.Property
      );
      item.insertText = new vscode.SnippetString(insertionFor(info));
      if (info.description) {
        item.documentation = new vscode.MarkdownString(info.description);
      }
      item.detail = propDetail(info);
      item.range = wordRangeAt(position);
      return item;
    });
}

function inlineTableKeyItems(
  context: string,
  tableKey: string,
  typed: string,
  usedKeys: string[],
  position: vscode.Position
): vscode.CompletionItem[] {
  const inline = context === "rules" ? `rules.${tableKey}` : null;
  if (!inline) return [];
  return filterByTyped(contextKeys(inline), typed)
    .filter((info) => !usedKeys.includes(info.key))
    .map((info) => {
      const item = new vscode.CompletionItem(info.key, vscode.CompletionItemKind.Property);
      item.insertText = new vscode.SnippetString(`${info.key} = "$1"`);
      if (info.description) item.documentation = new vscode.MarkdownString(info.description);
      item.detail = propDetail(info);
      item.range = wordRangeAt(position);
      return item;
    });
}

function valueItems(
  context: string,
  key: string,
  typed: string,
  index: ReturnType<typeof resolveEditContext>["index"],
  position: vscode.Position
): vscode.CompletionItem[] {
  void typed;
  const range = wordRangeAt(position);

  // Cross-references first: rules referenced by name.
  if (key === "depends_on" || key === "extends") {
    return [...index.ruleNameLines.keys()].map((name) => {
      const item = new vscode.CompletionItem(name, vscode.CompletionItemKind.Value);
      item.detail = `rule (line ${(index.lineOfRuleName(name) ?? 0) + 1})`;
      item.range = range;
      return item;
    });
  }
  if (key === "env_group") {
    return index.envGroupNames.map((name) => {
      const item = new vscode.CompletionItem(name, vscode.CompletionItemKind.Value);
      item.detail = "[env_groups.*] group in this pipeline";
      item.range = range;
      return item;
    });
  }

  const enums = enumFor(context, key);
  if (enums) {
    return enums.map((v) => {
      const item = new vscode.CompletionItem(v, vscode.CompletionItemKind.EnumMember);
      item.detail = `allowed value of ${key}`;
      item.range = range;
      return item;
    });
  }
  const info = contextKeys(context).find((k) => k.key === key);
  if (info?.type === "boolean") {
    return ["true", "false"].map((v) => {
      const item = new vscode.CompletionItem(v, vscode.CompletionItemKind.Keyword);
      item.range = range;
      return item;
    });
  }
  return [];
}

function usedKeysInContext(
  index: ReturnType<typeof resolveEditContext>["index"],
  document: vscode.TextDocument,
  position: vscode.Position
): string[] {
  const section = index.sectionBefore(position.line);
  return index.keySegments
    .filter(
      (seg) =>
        seg.section !== null &&
        section !== null &&
        seg.section.headerLine === section.headerLine &&
        !(seg.startLine === position.line && seg.key !== "")
    )
    .map((seg) => seg.key.split(".")[0]);
}

function propDetail(info: PropInfo): string {
  const type = info.enums ? `${info.type ?? "string"}: ${info.enums.join(" | ")}` : (info.type ?? "value");
  return type;
}

function wordRangeAt(position: vscode.Position): vscode.Range {
  void position;
  // An empty replacement range is the safe default: VS Code's built-in
  // `editor.suggest.filterGraceful` handles prefix filtering, and a wrong
  // word range would clobber neighboring TOML punctuation.
  return new vscode.Range(position, position);
}
