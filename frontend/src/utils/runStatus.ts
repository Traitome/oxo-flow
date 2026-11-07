// Single source for "this run can no longer change state" (issue #1040 B5):
// the sidebar's active-run dot and the monitor page both clear on these
// statuses, and a drifted copy in either place would leave the badge stuck.
export const TERMINAL_RUN_STATUSES = new Set(['completed', 'failed', 'cancelled']);

export function isTerminalRunStatus(status: string | null | undefined): boolean {
  return status !== null && status !== undefined && TERMINAL_RUN_STATUSES.has(status);
}
