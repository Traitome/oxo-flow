// Guided-mode safety gate (issue #1143, audit UX-01).
//
// The guided rule builder models only a fraction of the workflow format:
// the `[workflow]` name/version and, per rule, name/shell/input/output/
// threads/memory/environment. Everything else — `[[values]]`, `[engine]`,
// `when`, hooks, retries, staging, tags, benchmarks, resources, `script`
// bodies, `[rules.*]` sub-tables — has no widget. The card write-back
// splices most of it through, but the card model still REGENERATES every
// managed key (e.g. a map/dir `output` becomes a plain list, a table
// `environment` becomes a kind string, a script-only rule gains an empty
// `shell`), so a guided save can silently change semantics.
//
// The safe contract is the audit's preferred route: refuse the guided save
// when the loaded document contains anything the form does not model,
// telling the user exactly what and pointing them at the TOML editor.
// This module is the single source of truth for that decision — the
// builder (renders the refusal) and the editor page (refuses Save) both
// call detectGuidedUnsupported.

import { scanTomlDocument, splitSectionStatements, statementKey } from './tomlSections';

// Rule keys the guided card owns outright. `resources.threads` /
// `resources.memory` are the documented spellings of two of these (the
// card reads and rewrites them), so they are modeled too.
export const MODELED_RULE_KEYS: ReadonlySet<string> = new Set([
  'name',
  'shell',
  'input',
  'output',
  'threads',
  'memory',
  'environment',
]);

const MODELED_RESOURCE_LEAVES: ReadonlySet<string> = new Set(['threads', 'memory']);

// Inline-table value (`output = { bam = "…" }`, `environment = { … }`):
// the card can only emit the list/string forms, so the table form is
// unmodeled even though the KEY is managed.
function hasTableValue(stmt: string): boolean {
  const eq = stmt.indexOf('=');
  if (eq === -1) return false;
  return stmt.slice(eq + 1).trimStart().startsWith('{');
}

// First quoted string value of a statement (`name = "align"`), used only
// for human-readable labels.
function stringValue(stmt: string): string | null {
  const eq = stmt.indexOf('=');
  if (eq === -1) return null;
  const m = stmt
    .slice(eq + 1)
    .trimStart()
    .match(/^(?:"((?:[^"\\]|\\.)*)"|'([^']*)')/);
  return m ? (m[1] ?? m[2] ?? null) : null;
}

function collectRuleStatement(stmt: string, rule: string, labels: string[]): void {
  const trimmed = stmt.trim();
  if (trimmed === '' || trimmed.startsWith('#')) return;
  const key = statementKey(trimmed);
  if (key === '') return;
  if (key === 'resources.threads' || key === 'resources.memory') return;
  if (!MODELED_RULE_KEYS.has(key)) {
    labels.push(`rule "${rule}" → ${key}`);
    return;
  }
  if (!hasTableValue(trimmed)) return;
  if (key === 'input' || key === 'output') {
    labels.push(`rule "${rule}" → ${key} (map/directory form)`);
  } else if (key === 'environment') {
    labels.push(`rule "${rule}" → environment (table form)`);
  }
}

function collectRuleSubtable(name: string, body: string, rule: string, labels: string[]): void {
  const prefix = rule ? `rule "${rule}" → ` : '';
  if (name === 'rules.resources') {
    // `[rules.resources]` carrying only threads/memory is exactly the
    // spelling the card models; anything else (gpu, time_limit, queue …)
    // has no widget.
    const advanced = splitSectionStatements(body)
      .map((s) => s.trim())
      .filter((s) => s !== '' && !s.startsWith('#'))
      .map(statementKey)
      .filter((k) => k !== '' && !MODELED_RESOURCE_LEAVES.has(k));
    if (advanced.length === 0) return;
    labels.push(`${prefix}[rules.resources] (${advanced.join(', ')})`);
    return;
  }
  labels.push(`${prefix}[${name}]`);
}

/**
 * List the constructs present in `toml` that the guided form cannot
 * represent, one human-readable label per construct. An empty array means
 * guided editing is safe for this document.
 */
export function detectGuidedUnsupported(toml: string): string[] {
  const labels: string[] = [];
  const { sections, unrecognizedHeaders } = scanTomlDocument(toml);

  // A header-shaped line the TOML grammar could not parse: it would be
  // swallowed into the previous section span, so fail closed instead of
  // letting a nested section ride along invisibly.
  for (const header of unrecognizedHeaders) {
    const line = toml.slice(0, header.lineStart).split('\n').length;
    labels.push(`unrecognized table header at line ${line}: ${header.text}`);
  }

  // No recognized section header in a non-empty document: the splice has
  // no anchors (cardsToToml would regenerate the whole file from the card
  // model), so guided editing is unsafe regardless of the content.
  if (sections.length === 0) {
    const hasContent = toml
      .split('\n')
      .some((line) => line.trim() !== '' && !line.trim().startsWith('#'));
    if (hasContent) labels.push('unrecognized document layout (no readable section headers)');
    return [...new Set(labels)];
  }

  // Root-level keys before the first section header: preserved by the
  // splice, but not modeled anywhere in the form.
  const rootEnd = sections.length > 0 ? sections[0].headerLineStart : toml.length;
  for (const stmt of splitSectionStatements(toml.slice(0, rootEnd))) {
    const trimmed = stmt.trim();
    if (trimmed === '' || trimmed.startsWith('#')) continue;
    const key = statementKey(trimmed);
    if (key !== '') labels.push(`top-level key "${key}"`);
  }

  let ruleName = '';
  let ruleIndex = 0;
  for (const sec of sections) {
    if (sec.isArrayTable && sec.name === 'rules') {
      ruleIndex += 1;
      const statements = splitSectionStatements(toml.slice(sec.bodyStart, sec.end));
      const nameStmt = statements.find((s) => statementKey(s) === 'name');
      ruleName = (nameStmt ? stringValue(nameStmt) : null) ?? `#${ruleIndex}`;
      for (const stmt of statements) collectRuleStatement(stmt, ruleName, labels);
      continue;
    }
    if (sec.name.startsWith('rules.')) {
      // A `[rules.*]` sub-table belongs to the most recent [[rules]] entry.
      collectRuleSubtable(sec.name, toml.slice(sec.bodyStart, sec.end), ruleName, labels);
      continue;
    }
    // `[workflow]` keys other than name/version (description …) are
    // preserved verbatim by the splice and are harmless, so the section
    // itself is modeled. Every other top-level section is not.
    if (!sec.isArrayTable && sec.name === 'workflow') continue;
    labels.push(sec.isArrayTable ? `[[${sec.name}]]` : `[${sec.name}]`);
  }

  return [...new Set(labels)];
}
