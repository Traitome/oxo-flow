import { promises as fs } from "node:fs";
import * as path from "node:path";

/**
 * Buffer-snapshot staging shared by diagnostics and AI draft validation.
 *
 * The `oxo-flow` CLI only reads workflows from a path (it has no stdin
 * surface), so an unsaved editor buffer — or a model draft that must be
 * validated BEFORE it is applied — is staged as a sibling `.oxoflow` file in
 * the destination directory. Keeping the snapshot next to the real workflow
 * preserves relative `[[include]]` / sample-pattern resolution, and the
 * `.oxoflow` suffix keeps the CLI's file checks identical to a saved file.
 *
 * Snapshots are always removed, including when the write itself fails partway
 * through (ENOSPC/quota) and when the body throws.
 */

/** Basename prefix of a staged snapshot — shared with the file-watcher guard. */
export const SNAPSHOT_PREFIX = ".oxo-flow-snapshot-";

let counter = 0;

/** A unique, hidden, same-directory snapshot path for `anchorPath`. */
export function snapshotPathFor(anchorPath: string, token?: string): string {
  const dir = path.dirname(anchorPath);
  const tag = token ?? `${process.pid}-${Date.now().toString(36)}-${(counter += 1)}`;
  return path.join(dir, `${SNAPSHOT_PREFIX}${tag}.oxoflow`);
}

/** True when `basename` names a staging snapshot (never a user workflow). */
export function isSnapshotBasename(basename: string): boolean {
  return basename.startsWith(SNAPSHOT_PREFIX);
}

export interface SnapshotOptions {
  /** Test seam for the staging write; defaults to node's `fs.writeFile`. */
  readonly write?: (snapshotPath: string, content: string) => Promise<void>;
}

/** Write `content` to a temporary `.oxoflow` next to `anchorPath`, run
 * `body` against it, and delete the snapshot in all cases. */
export async function withContentSnapshot<T>(
  anchorPath: string,
  content: string,
  body: (snapshotPath: string) => Promise<T>,
  opts: SnapshotOptions = {}
): Promise<T> {
  const snapshotPath = snapshotPathFor(anchorPath);
  const write = opts.write ?? ((p: string, c: string) => fs.writeFile(p, c, "utf8"));
  try {
    // The write is inside the guard: a partial file left by a failed write
    // must be removed as well.
    await write(snapshotPath, content);
    return await body(snapshotPath);
  } finally {
    await fs.rm(snapshotPath, { force: true });
  }
}
