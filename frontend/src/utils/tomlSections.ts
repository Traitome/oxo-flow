// Lexical TOML section helpers shared by the editor surfaces that must
// splice documents without re-serializing them (PipelineEditor's engine
// form, the guided rule builder's card write-back).

export interface TomlSectionSpan {
  name: string;
  isArrayTable: boolean;
  headerLineStart: number;
  bodyStart: number;
  end: number;
}

export interface TomlUnrecognizedHeader {
  lineStart: number;
  /** Trimmed source line, for a specific user-facing message. */
  text: string;
}

export interface TomlScanResult {
  sections: TomlSectionSpan[];
  /**
   * Lines that lex like a table header (leading `[`) but do not parse as
   * TOML-legal table headers. Callers that must fail closed (the guided
   * safety gate) refuse the document when this is non-empty — an
   * unparsed header would otherwise be swallowed into the previous
   * section span.
   */
  unrecognizedHeaders: TomlUnrecognizedHeader[];
}

function isBareKeyChar(ch: string | undefined): boolean {
  return ch !== undefined && /[A-Za-z0-9_-]/.test(ch);
}

function isHex(text: string): boolean {
  for (const ch of text) {
    if (!/[0-9A-Fa-f]/.test(ch)) return false;
  }
  return text.length > 0;
}

// Decode the content of a TOML basic string used as a key part. Unknown
// or malformed escapes return null so the caller can fail closed instead
// of comparing a raw escape sequence as if it were a key.
function decodeBasicKey(raw: string): string | null {
  let out = '';
  for (let i = 0; i < raw.length; i++) {
    const ch = raw[i];
    if (ch !== '\\') {
      out += ch;
      continue;
    }
    const next = raw[++i];
    switch (next) {
      case 'b': out += '\b'; break;
      case 't': out += '\t'; break;
      case 'n': out += '\n'; break;
      case 'f': out += '\f'; break;
      case 'r': out += '\r'; break;
      case '"': out += '"'; break;
      case '\\': out += '\\'; break;
      case 'u':
      case 'U': {
        const len = next === 'u' ? 4 : 8;
        const hex = raw.slice(i + 1, i + 1 + len);
        if (hex.length !== len || !isHex(hex)) return null;
        const cp = parseInt(hex, 16);
        if (cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) return null;
        out += String.fromCodePoint(cp);
        i += len;
        break;
      }
      default:
        return null;
    }
  }
  return out;
}

// Parse a TOML key expression (bare / basic-quoted / literal-quoted parts
// separated by dotted keys, with optional whitespace around the dots).
// Returns the canonical dotted name and the index just past the key.
function parseKeyExpression(line: string, start: number): { key: string; next: number } | null {
  const parts: string[] = [];
  let i = start;
  for (;;) {
    while (line[i] === ' ' || line[i] === '\t') i++;
    const ch = line[i];
    if (ch === undefined) return null;
    if (ch === '"') {
      let j = i + 1;
      let raw = '';
      while (j < line.length) {
        if (line[j] === '\\') {
          raw += line[j] + (line[j + 1] ?? '');
          j += 2;
          continue;
        }
        if (line[j] === '"') break;
        raw += line[j];
        j++;
      }
      if (j >= line.length) return null;
      const decoded = decodeBasicKey(raw);
      if (decoded === null || decoded === '') return null;
      parts.push(decoded);
      i = j + 1;
    } else if (ch === "'") {
      const j = line.indexOf("'", i + 1);
      if (j === -1 || j === i + 1) return null;
      parts.push(line.slice(i + 1, j));
      i = j + 1;
    } else {
      let j = i;
      while (isBareKeyChar(line[j])) j++;
      if (j === i) return null;
      parts.push(line.slice(i, j));
      i = j;
    }
    while (line[i] === ' ' || line[i] === '\t') i++;
    if (line[i] === '.') {
      i++;
      continue;
    }
    return { key: parts.join('.'), next: i };
  }
}

export interface ParsedTableHeader {
  name: string;
  isArrayTable: boolean;
}

/**
 * Parse one line as a TOML table header, accepting every TOML-legal
 * spelling (`[table]`, `[[array]]`, inner whitespace, quoted key parts,
 * dotted keys, a trailing comment and CRLF). Returns:
 *
 * - `null` when the line is not header-shaped at all (no leading `[`);
 * - `'invalid'` when it starts with `[` but is not a legal header;
 * - the canonical `{ name, isArrayTable }` otherwise.
 */
export function parseTableHeader(rawLine: string): ParsedTableHeader | 'invalid' | null {
  const line = rawLine.endsWith('\r') ? rawLine.slice(0, -1) : rawLine;
  let i = 0;
  while (line[i] === ' ' || line[i] === '\t') i++;
  if (line[i] !== '[') return null;
  i++;
  let isArrayTable = false;
  if (line[i] === '[') {
    isArrayTable = true;
    i++;
  }
  const parsed = parseKeyExpression(line, i);
  if (!parsed) return 'invalid';
  i = parsed.next;
  if (isArrayTable) {
    if (line[i] !== ']' || line[i + 1] !== ']') return 'invalid';
    i += 2;
  } else {
    if (line[i] !== ']') return 'invalid';
    i += 1;
  }
  while (line[i] === ' ' || line[i] === '\t') i++;
  if (line[i] === '#') return { name: parsed.key, isArrayTable };
  if (i !== line.length) return 'invalid';
  return { name: parsed.key, isArrayTable };
}

// Lexically scan TOML top-level section headers, ignoring anything inside
// basic/literal single- or multi-line strings, `#` comments, or multi-line
// inline arrays/tables. Header recognition is total over TOML-legal
// spellings; anything header-shaped but unparseable is reported so
// fail-closed callers can refuse it instead of silently swallowing it
// into the previous section (issue #1143 review finding).
export function scanTomlDocument(toml: string): TomlScanResult {
  const sections: TomlSectionSpan[] = [];
  const unrecognizedHeaders: TomlUnrecognizedHeader[] = [];
  let inMultiBasic = false;
  let inMultiLiteral = false;
  let bracketDepth = 0;

  let lineStart = 0;
  while (lineStart <= toml.length) {
    const nlIdx = toml.indexOf('\n', lineStart);
    const lineEnd = nlIdx === -1 ? toml.length : nlIdx;
    const rawLine = toml.slice(lineStart, lineEnd);

    if (!inMultiBasic && !inMultiLiteral && bracketDepth === 0) {
      const header = parseTableHeader(rawLine);
      if (header === 'invalid') {
        unrecognizedHeaders.push({ lineStart, text: rawLine.trim() });
      } else if (header) {
        if (sections.length > 0) {
          sections[sections.length - 1].end = lineStart;
        }
        sections.push({
          name: header.name,
          isArrayTable: header.isArrayTable,
          headerLineStart: lineStart,
          bodyStart: nlIdx === -1 ? toml.length : nlIdx + 1,
          end: toml.length,
        });
        if (nlIdx === -1) break;
        lineStart = nlIdx + 1;
        continue;
      }
    }

    let i = 0;
    let inBasic = false;
    let inLiteral = false;
    while (i < rawLine.length) {
      const ch = rawLine[i];
      if (inMultiBasic) {
        if (ch === '\\') { i += 2; continue; }
        if (rawLine.startsWith('"""', i)) { inMultiBasic = false; i += 3; continue; }
        i++;
      } else if (inMultiLiteral) {
        if (rawLine.startsWith("'''", i)) { inMultiLiteral = false; i += 3; continue; }
        i++;
      } else if (inBasic) {
        if (ch === '\\') { i += 2; continue; }
        if (ch === '"') inBasic = false;
        i++;
      } else if (inLiteral) {
        if (ch === "'") inLiteral = false;
        i++;
      } else {
        if (ch === '#') break;
        if (rawLine.startsWith('"""', i)) { inMultiBasic = true; i += 3; continue; }
        if (rawLine.startsWith("'''", i)) { inMultiLiteral = true; i += 3; continue; }
        if (ch === '"') { inBasic = true; i++; continue; }
        if (ch === "'") { inLiteral = true; i++; continue; }
        if (ch === '[' || ch === '{') bracketDepth++;
        else if ((ch === ']' || ch === '}') && bracketDepth > 0) bracketDepth--;
        i++;
      }
    }

    if (nlIdx === -1) break;
    lineStart = nlIdx + 1;
  }

  return { sections, unrecognizedHeaders };
}

/** Section spans only — the historical API used by the editor surfaces. */
export function scanTomlTopLevelSections(toml: string): TomlSectionSpan[] {
  return scanTomlDocument(toml).sections;
}

// Top-level key of a TOML statement, bare or quoted (dotted keys like
// `resources.memory` come back with the TOML-legal spellings normalized —
// whitespace around dots, quoted parts). Returns '' when the text does not
// look like a key = value statement (comments, continuations). Shared by
// the guided builder (preserved-statement classification) and its
// unmodeled-construct detector so both read the document identically.
export function statementKey(stmt: string): string {
  const firstLine = stmt.split('\n', 1)[0];
  const parsed = parseKeyExpression(firstLine, 0);
  if (!parsed) return '';
  let i = parsed.next;
  while (firstLine[i] === ' ' || firstLine[i] === '\t') i++;
  return firstLine[i] === '=' ? parsed.key : '';
}

export function splitSectionStatements(body: string): string[] {
  const lines = body.replace(/\r\n/g, '\n').split('\n');
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
  const statements: string[] = [];
  let current: string[] = [];
  let inMultiBasic = false;
  let inMultiLiteral = false;
  let bracketDepth = 0;

  for (const line of lines) {
    current.push(line);
    let i = 0;
    let inBasic = false;
    let inLiteral = false;
    while (i < line.length) {
      const ch = line[i];
      if (inMultiBasic) {
        if (ch === '\\') { i += 2; continue; }
        if (line.startsWith('"""', i)) { inMultiBasic = false; i += 3; continue; }
        i++;
      } else if (inMultiLiteral) {
        if (line.startsWith("'''", i)) { inMultiLiteral = false; i += 3; continue; }
        i++;
      } else if (inBasic) {
        if (ch === '\\') { i += 2; continue; }
        if (ch === '"') inBasic = false;
        i++;
      } else if (inLiteral) {
        if (ch === "'") inLiteral = false;
        i++;
      } else {
        if (ch === '#') break;
        if (line.startsWith('"""', i)) { inMultiBasic = true; i += 3; continue; }
        if (line.startsWith("'''", i)) { inMultiLiteral = true; i += 3; continue; }
        if (ch === '"') { inBasic = true; i++; continue; }
        if (ch === "'") { inLiteral = true; i++; continue; }
        if (ch === '[' || ch === '{') bracketDepth++;
        else if ((ch === ']' || ch === '}') && bracketDepth > 0) bracketDepth--;
        i++;
      }
    }
    if (!inMultiBasic && !inMultiLiteral && bracketDepth === 0) {
      statements.push(current.join('\n'));
      current = [];
    }
  }
  if (current.length > 0) statements.push(current.join('\n'));
  return statements;
}
