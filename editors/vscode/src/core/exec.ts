/**
 * Thin, promise-based wrapper around the `oxo-flow` executable. vscode-free
 * so it can be exercised without an editor host.
 */
import { execFile } from "node:child_process";

export interface CliResult {
  stdout: string;
  stderr: string;
  exitCode: number | null;
  /** Set when the binary could not be launched at all (ENOENT, EACCES, …). */
  spawnError?: string;
}

export interface RunCliOptions {
  cwd?: string;
  timeoutMs?: number;
  onSpawn?: (kill: () => void) => void;
}

export async function runCli(
  executable: string,
  args: string[],
  opts: RunCliOptions = {}
): Promise<CliResult> {
  return new Promise((resolve) => {
    const child = execFile(
      executable,
      args,
      {
        cwd: opts.cwd,
        timeout: opts.timeoutMs ?? 60_000,
        maxBuffer: 16 * 1024 * 1024,
        // JSON lands on stdout uncolored anyway, but be explicit: the CLI
        // strips ANSI when NO_COLOR is set.
        env: { ...process.env, NO_COLOR: "1" },
        windowsHide: true,
      },
      (error, stdout, stderr) => {
        const err = error as (NodeJS.ErrnoException & { code?: number | string }) | null;
        if (err && typeof err.code === "string") {
          resolve({ stdout: stdout.toString(), stderr: stderr.toString(), exitCode: null, spawnError: err.code });
          return;
        }
        resolve({
          stdout: stdout.toString(),
          stderr: stderr.toString(),
          exitCode: typeof err?.code === "number" ? err.code : err ? 1 : 0,
        });
      }
    );
    opts.onSpawn?.(() => child.kill());
  });
}
