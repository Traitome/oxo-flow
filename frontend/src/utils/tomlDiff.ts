// Line-level diff for the AI proposal review gate (audit UX-05, issue #1147):
// an AI-generated workflow is staged as a pending proposal and the editor
// only changes after the user accepts the diff shown here. No diff
// dependency is pulled in — the editor already ships zero runtime deps
// beyond React/CodeMirror, and a line diff is small enough to own.

export type DiffLineKind = 'context' | 'added' | 'removed';

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
  /** 1-based line number in the current (before) document, or null when added. */
  oldLine: number | null;
  /** 1-based line number in the proposed (after) document, or null when removed. */
  newLine: number | null;
}

export interface DiffSummary {
  added: number;
  removed: number;
}

interface DiffBlock {
  kind: DiffLineKind;
  text: string;
}

// The LCS table is (n+1)*(m+1) 32-bit cells. Workflows are small, but a
// pasted document must not be able to freeze the tab: past this ceiling the
// middle collapses into one removed block + one added block — a coarser but
// always truthful rendering (never a false "no change").
const MAX_DP_CELLS = 1_500_000;

function diffMiddle(a: string[], b: string[]): DiffBlock[] {
  if (a.length === 0) return b.map((text) => ({ kind: 'added', text }));
  if (b.length === 0) return a.map((text) => ({ kind: 'removed', text }));
  if (a.length * b.length > MAX_DP_CELLS) {
    return [
      ...a.map((text) => ({ kind: 'removed' as const, text })),
      ...b.map((text) => ({ kind: 'added' as const, text })),
    ];
  }

  const width = b.length + 1;
  const lcs = new Uint32Array((a.length + 1) * width);
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lcs[i * width + j] =
        a[i] === b[j]
          ? lcs[(i + 1) * width + j + 1] + 1
          : Math.max(lcs[(i + 1) * width + j], lcs[i * width + j + 1]);
    }
  }

  const out: DiffBlock[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ kind: 'context', text: a[i] });
      i++;
      j++;
    } else if (lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]) {
      out.push({ kind: 'removed', text: a[i] });
      i++;
    } else {
      out.push({ kind: 'added', text: b[j] });
      j++;
    }
  }
  while (i < a.length) out.push({ kind: 'removed', text: a[i++] });
  while (j < b.length) out.push({ kind: 'added', text: b[j++] });
  return out;
}

/** Line diff between the current editor content and a proposed document. */
export function diffLines(before: string, after: string): DiffLine[] {
  const a = before.split('\n');
  const b = after.split('\n');

  // Trim the identical prefix/suffix before running the LCS so a small edit
  // in a long document stays cheap.
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }

  const out: DiffLine[] = [];
  let oldNo = 0;
  let newNo = 0;
  for (let k = 0; k < start; k++) {
    out.push({ kind: 'context', text: a[k], oldLine: ++oldNo, newLine: ++newNo });
  }
  for (const block of diffMiddle(a.slice(start, endA), b.slice(start, endB))) {
    if (block.kind === 'context') {
      out.push({ kind: 'context', text: block.text, oldLine: ++oldNo, newLine: ++newNo });
    } else if (block.kind === 'removed') {
      out.push({ kind: 'removed', text: block.text, oldLine: ++oldNo, newLine: null });
    } else {
      out.push({ kind: 'added', text: block.text, oldLine: null, newLine: ++newNo });
    }
  }
  for (let k = endA; k < a.length; k++) {
    out.push({ kind: 'context', text: a[k], oldLine: ++oldNo, newLine: ++newNo });
  }
  return out;
}

export function summarizeDiff(lines: DiffLine[]): DiffSummary {
  let added = 0;
  let removed = 0;
  for (const line of lines) {
    if (line.kind === 'added') added++;
    else if (line.kind === 'removed') removed++;
  }
  return { added, removed };
}
