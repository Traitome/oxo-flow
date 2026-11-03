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

// Lexically scan TOML top-level section headers (`[table]` and `[[array]]`),
// ignoring anything inside basic/literal single- or multi-line strings,
// `#` comments, or multi-line inline arrays/tables.
export function scanTomlTopLevelSections(toml: string): TomlSectionSpan[] {
  const sections: TomlSectionSpan[] = [];
  let inMultiBasic = false;
  let inMultiLiteral = false;
  let bracketDepth = 0;

  let lineStart = 0;
  while (lineStart <= toml.length) {
    const nlIdx = toml.indexOf('\n', lineStart);
    const lineEnd = nlIdx === -1 ? toml.length : nlIdx;
    const rawLine = toml.slice(lineStart, lineEnd);

    if (!inMultiBasic && !inMultiLiteral && bracketDepth === 0) {
      const arrayMatch = rawLine.match(/^[ \t]*\[\[([A-Za-z0-9_.-]+)\]\][ \t]*(?:#.*)?\r?$/);
      const tableMatch = !arrayMatch ? rawLine.match(/^[ \t]*\[([A-Za-z0-9_.-]+)\][ \t]*(?:#.*)?\r?$/) : null;
      if (arrayMatch || tableMatch) {
        if (sections.length > 0) {
          sections[sections.length - 1].end = lineStart;
        }
        sections.push({
          name: (arrayMatch ?? tableMatch)![1],
          isArrayTable: Boolean(arrayMatch),
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

  return sections;
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
