/**
 * A lightweight, line-oriented TOML index for `.oxoflow` documents.
 *
 * It is deliberately not a full TOML parser: the extension needs just enough
 * structure to (a) anchor CLI diagnostics to the `name = "..."` line of a
 * failing rule and (b) know which table/key the cursor is completing. The
 * scanner tracks multi-line strings and open arrays so those cases don't
 * confuse it, and stays conservative (returns null / no completion) where
 * the structure is ambiguous.
 */

export type SectionKind = "table" | "array-of-tables";

export interface Section {
  kind: SectionKind;
  /** Dotted name without brackets, e.g. "workflow", "rules", "env_groups.rnaseq". */
  name: string;
  /** Zero-based line of the header. */
  headerLine: number;
}

export interface KeySegment {
  /** Full dotted key as written, e.g. "name" or "config.seeds". */
  key: string;
  /** Line the `key =` appears on. */
  startLine: number;
  /** Last line belonging to this key's value (open arrays span lines). */
  endLine: number;
  section: Section | null;
}

export interface TomlIndex {
  sections: Section[];
  keySegments: KeySegment[];
  /** Rule name -> line of its `name = "..."` (inside [[rules]] blocks only). */
  ruleNameLines: Map<string, number>;
  /** Group names from [env_groups.<name>] sub-tables. */
  envGroupNames: string[];
  /** Keys declared under [wildcard_constraints]. */
  wildcardNames: string[];
  sectionBefore(line: number): Section | null;
  /** The key whose value spans `line`, if any. */
  segmentAt(line: number): KeySegment | null;
  lineOfRuleName(name: string): number | null;
}

/** Remove a trailing `# comment`, honoring quotes (basic strings honor \\). */
export function stripComment(line: string): string {
  let inBasic = false;
  let inLiteral = false;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (inBasic) {
      if (ch === "\\") i++;
      else if (ch === '"') inBasic = false;
    } else if (inLiteral) {
      if (ch === "'") inLiteral = false;
    } else if (ch === '"') inBasic = true;
    else if (ch === "'") inLiteral = true;
    else if (ch === "#") return line.slice(0, i);
  }
  return line;
}

const TABLE_RE = /^\s*\[\s*([^\]]+?)\s*\]\s*$/;
const ARRAY_TABLE_RE = /^\s*\[\[\s*([^\]]+?)\s*\]\]\s*$/;
// Bare keys, quoted keys and dotted compositions, followed by `=`.
const KEY_RE =
  /^\s*("(?:[^"\\]|\\.)*"|'[^']*'|(?:[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*))\s*=(.*)$/;
const RULE_NAME_RE = /^\s*"((?:[^"\\]|\\.)*)"/;

function unquote(key: string): string {
  if (key.length >= 2 && ((key[0] === '"' && key.endsWith('"')) || (key[0] === "'" && key.endsWith("'")))) {
    return key.slice(1, -1);
  }
  return key;
}

/** Net bracket depth contributed by a value fragment (strings ignored). */
function bracketDepth(fragment: string): number {
  let depth = 0;
  let inBasic = false;
  let inLiteral = false;
  for (let i = 0; i < fragment.length; i++) {
    const ch = fragment[i];
    if (inBasic) {
      if (ch === "\\") i++;
      else if (ch === '"') inBasic = false;
    } else if (inLiteral) {
      if (ch === "'") inLiteral = false;
    } else if (ch === '"') inBasic = true;
    else if (ch === "'") inLiteral = true;
    else if (ch === "[") depth++;
    else if (ch === "]") depth--;
  }
  return depth;
}

/** Does `fragment` open a multi-line string (""" or ''') without closing it? */
function opensMultilineString(fragment: string): '"' | "'" | null {
  const t = fragment.trimStart();
  for (const quote of ['"""', "'''"] as const) {
    if (t.startsWith(quote)) {
      // Parity of delimiter occurrences decides: `'''x'''` (2) closed on one
      // line, `'''x` (1) stays open across lines.
      const count = t.split(quote).length - 1;
      return count % 2 === 1 ? (quote[0] as '"' | "'") : null;
    }
  }
  return null;
}

const MULTILINE_TERMINATORS: Record<string, string> = { '"': '"""', "'": "'''" };

export function indexToml(text: string): TomlIndex {
  const lines = text.split(/\r\n|\n|\r/);
  const sections: Section[] = [];
  const segments: KeySegment[] = [];
  const ruleNameLines = new Map<string, number>();
  const envGroupNames: string[] = [];
  const wildcardNames: string[] = [];

  let currentSection: Section | null = null;
  let pending: KeySegment | null = null;
  let arrayDepth = 0;
  let multiline: string | null = null;

  const closePending = (upTo: number) => {
    if (pending) {
      pending.endLine = Math.max(pending.startLine, upTo);
      segments.push(pending);
      pending = null;
    }
    arrayDepth = 0;
  };

  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];

    if (multiline !== null) {
      if (raw.includes(MULTILINE_TERMINATORS[multiline])) {
        multiline = null;
        // The string's span ends exactly here — flush the segment now so a
        // later closePending cannot stretch it over unrelated lines.
        if (pending) {
          pending.endLine = i;
          segments.push(pending);
          pending = null;
        }
      }
      continue;
    }

    const code = stripComment(raw);
    if (code.trim() === "") {
      if (pending && arrayDepth > 0) pending.endLine = i; // trailing blank line inside an array
      continue;
    }

    // Table headers are only recognized outside open arrays / strings.
    if (arrayDepth === 0) {
      const arr = ARRAY_TABLE_RE.exec(code);
      const tbl = arr ? null : TABLE_RE.exec(code);
      if (arr || tbl) {
        closePending(i - 1);
        currentSection = {
          kind: arr ? "array-of-tables" : "table",
          name: unquote((arr ? arr[1] : tbl![1]).trim()),
          headerLine: i,
        };
        sections.push(currentSection);
        continue;
      }
    }

    const keyMatch = KEY_RE.exec(code);
    if (keyMatch) {
      closePending(i - 1);
      const key = unquote(keyMatch[1]);
      const rest = keyMatch[2];
      pending = { key, startLine: i, endLine: i, section: currentSection };
      arrayDepth = Math.max(0, bracketDepth(rest));
      const ml = opensMultilineString(rest);
      if (ml) {
        multiline = ml;
        continue;
      }
      // Values closed on this line — remember domain-specific references.
      if (pending.key === "name" && currentSection?.kind === "array-of-tables" && currentSection.name === "rules") {
        const m = RULE_NAME_RE.exec(rest);
        if (m && !ruleNameLines.has(m[1])) ruleNameLines.set(m[1], i);
      }
    } else if (pending && arrayDepth > 0) {
      arrayDepth = Math.max(0, arrayDepth + bracketDepth(code));
      pending.endLine = i;
    }
  }
  closePending(lines.length - 1);

  for (const seg of segments) {
    if (seg.section?.kind === "table" && seg.section.name.startsWith("env_groups.")) {
      const group = seg.section.name.slice("env_groups.".length);
      if (group && !envGroupNames.includes(group)) envGroupNames.push(group);
    }
    if (seg.section?.kind === "table" && seg.section.name === "wildcard_constraints") {
      const head = seg.key.split(".")[0];
      if (!wildcardNames.includes(head)) wildcardNames.push(head);
    }
  }

  return {
    sections,
    keySegments: segments,
    ruleNameLines,
    envGroupNames,
    wildcardNames,
    sectionBefore(line: number) {
      let found: Section | null = null;
      for (const s of sections) {
        if (s.headerLine <= line) found = s;
        else break;
      }
      return found;
    },
    segmentAt(line: number) {
      for (const seg of segments) {
        if (line >= seg.startLine && line <= seg.endLine) return seg;
      }
      return null;
    },
    lineOfRuleName(name: string) {
      return ruleNameLines.get(name) ?? null;
    },
  };
}
