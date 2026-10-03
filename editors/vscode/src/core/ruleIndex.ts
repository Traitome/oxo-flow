/**
 * Document-order rule extraction for CodeLens and "run to here" targets.
 *
 * Pure (vscode-free) so it is directly unit-testable; the provider in
 * `providers/codelens.ts` only maps lines to ranges.
 */

import { indexToml } from "./tomlIndex";

export interface RuleLensInfo {
  name: string;
  /** Zero-based line of the rule's `name = "..."`. */
  nameLine: number;
  /** Zero-based line of the `[[rules]]` header. */
  headerLine: number;
}

/** Named `[[rules]]` blocks in file order — one lens pair per rule. */
export function ruleLensInfos(text: string): RuleLensInfo[] {
  const idx = indexToml(text);
  const infos: RuleLensInfo[] = [];
  for (const [name, line] of idx.ruleNameLines) {
    const section = idx.sectionBefore(line);
    if (section?.kind === "array-of-tables" && section.name === "rules") {
      infos.push({ name, nameLine: line, headerLine: section.headerLine });
    }
  }
  return infos.sort((a, b) => a.headerLine - b.headerLine);
}

/** File-order rule names up to and including `name` ("run to here"). */
export function rulesUpTo(infos: RuleLensInfo[], name: string): string[] {
  const at = infos.findIndex((r) => r.name === name);
  if (at < 0) return [];
  return infos.slice(0, at + 1).map((r) => r.name);
}
