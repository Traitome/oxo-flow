/**
 * Resolves what the cursor is editing inside a `.oxoflow` document. Shared by
 * the completion and hover providers so both agree on semantics (and both are
 * conservative: anything ambiguous degrades to "none").
 */
import { indexToml, stripComment, type TomlIndex } from "./tomlIndex";

export type EditContext =
  | { kind: "none" }
  | { kind: "comment" }
  | { kind: "multiline-string" }
  | { kind: "header"; context: string; typed: string }
  | { kind: "key"; context: string; typed: string }
  | { kind: "inline-table-key"; context: string; tableKey: string; typed: string; usedKeys: string[] }
  | { kind: "value"; context: string; key: string; typed: string }
  | { kind: "inline-table-value"; context: string; tableKey: string; key: string; typed: string };

export interface ResolvedContext {
  ctx: EditContext;
  index: TomlIndex;
}

export function resolveEditContext(text: string, lineIdx: number, col: number): ResolvedContext {
  const index = indexToml(text);
  const lines = text.split(/\r\n|\n|\r/);
  const lineText = lines[lineIdx] ?? "";
  const before = lineText.slice(0, col);

  // Multi-line string content takes precedence: a `# ...` line inside a
  // `shell = '''` block is script text, not a comment.
  if (insideMultilineString(lines, lineIdx, col)) return { ctx: { kind: "multiline-string" }, index };
  // A line whose code portion is empty (after comment stripping) but which
  // has raw content is a full-line comment — completion would only confuse.
  if (stripComment(lineText).trim() === "" && lineText.trim() !== "") {
    return { ctx: { kind: "comment" }, index };
  }

  const section = index.sectionBefore(lineIdx);
  const context = sectionName(section);

  const headerTyped = /^\s*\[\[?\s*([A-Za-z0-9_.-]*)$/.exec(before);
  if (headerTyped) return { ctx: { kind: "header", context, typed: headerTyped[1] }, index };

  const seg = index.segmentAt(lineIdx);
  // A `=` on the current line puts us on the value side; a continuation line
  // of a multi-line value (array / string) is value side all the way.
  const ownEq = before.indexOf("=");
  const continuation = seg !== null && seg.startLine < lineIdx && ownEq < 0;
  const eqPos = ownEq >= 0 ? ownEq : continuation ? 0 : -1;
  if (seg && eqPos >= 0) {
    const valueText = continuation ? before : before.slice(eqPos + 1);
    const brace = lastUnclosedBracePos(valueText);
    if (brace !== null && valueText[brace] === "{") {
      const inner = valueText.slice(brace + 1);
      const usedKeys = collectInlineKeys(inner);
      const eqIn = inner.lastIndexOf("=");
      if (eqIn >= 0) {
        const key = trailingToken(inner.slice(0, eqIn).replace(/,\s*$/, ""));
        const inlineKey = key.length > 0 ? key : lastInlineKeyBefore(inner.slice(0, eqIn));
        if (inlineKey) {
          return {
            ctx: {
              kind: "inline-table-value",
              context,
              tableKey: seg.key,
              key: inlineKey,
              typed: trailingToken(inner.slice(eqIn + 1)),
            },
            index,
          };
        }
      }
      return {
        ctx: { kind: "inline-table-key", context, tableKey: seg.key, typed: trailingToken(inner), usedKeys },
        index,
      };
    }
    return { ctx: { kind: "value", context, key: seg.key, typed: trailingToken(valueText) }, index };
  }

  // Key side: no `=` before the cursor on this line and not a continuation.
  return { ctx: { kind: "key", context, typed: trailingToken(before) }, index };
}

/** "rules" for [[rules]], table names as-is, "root" before any header. */
export function contextName(section: { kind: string; name: string } | null): string {
  if (section === null) return "root";
  if (section.kind === "array-of-tables" && section.name === "rules") return "rules";
  return section.name;
}

function sectionName(section: ReturnType<TomlIndex["sectionBefore"]>): string {
  return contextName(section);
}

/** The token the cursor sits in (identifiers, dots, dashes, braces). */
function trailingToken(text: string): string {
  const m = /[A-Za-z0-9_.{}-]*$/.exec(text);
  return m ? m[0] : "";
}

/** Position of the last unclosed `{` (or `[`) in a value fragment, if any. */
function lastUnclosedBracePos(text: string): number | null {
  let depthSq = 0;
  let depthCu = 0;
  let inBasic = false;
  let inLiteral = false;
  let lastOpen = -1;
  let openChar = "";
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (inBasic) {
      if (ch === "\\") i++;
      else if (ch === '"') inBasic = false;
    } else if (inLiteral) {
      if (ch === "'") inLiteral = false;
    } else if (ch === '"') inBasic = true;
    else if (ch === "'") inLiteral = true;
    else if (ch === "[") {
      if (depthCu === 0) depthSq++;
    } else if (ch === "]") {
      if (depthCu === 0) depthSq--;
    } else if (ch === "{") {
      if (depthSq === 0) {
        depthCu++;
        lastOpen = i;
        openChar = "{";
      }
    } else if (ch === "}") {
      if (depthSq === 0) depthCu--;
    }
  }
  return depthCu > 0 && openChar === "{" ? lastOpen : null;
}

/** Keys already assigned inside an inline table fragment (before the cursor). */
function collectInlineKeys(fragment: string): string[] {
  const used: string[] = [];
  const re = /(?:^|[{,]\s*)([A-Za-z0-9_-]+)\s*=/g;
  for (const m of fragment.matchAll(re)) {
    if (!used.includes(m[1])) used.push(m[1]);
  }
  return used;
}

/** The most recent `key =` inside an inline-table fragment. */
function lastInlineKeyBefore(fragment: string): string | null {
  const matches = [...fragment.matchAll(/([A-Za-z0-9_-]+)\s*=/g)];
  const last = matches.at(-1);
  return last ? last[1] : null;
}

function insideMultilineString(lines: string[], lineIdx: number, col: number): boolean {
  let state: string | null = null;
  const scan = (l: string) => {
    if (state === null) {
      const m = /("""|''')/.exec(l);
      if (!m) return;
      // Parity of the delimiter run from the first opening: odd means the
      // string is still open when the line ends.
      const from = l.slice(m.index);
      const count = from.split(m[1]).length - 1;
      if (count % 2 === 1) state = m[1][0];
      return;
    }
    const term = state === '"' ? '"""' : "'''";
    while (l.length > 0) {
      const idx = l.indexOf(term);
      if (idx < 0) return;
      state = null;
      l = l.slice(idx + 3);
      // A new string may open right after the close on the same line.
      const next = /("""|''')/.exec(l);
      if (next) {
        const from = l.slice(next.index);
        const count = from.split(next[1]).length - 1;
        if (count % 2 === 1) state = next[1][0];
        return;
      }
    }
  };
  for (let i = 0; i < lineIdx; i++) scan(lines[i]);
  if (state === null) scan((lines[lineIdx] ?? "").slice(0, col));
  return state !== null;
}
