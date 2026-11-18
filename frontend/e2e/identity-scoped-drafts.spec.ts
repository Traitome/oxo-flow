import { test, expect, type APIRequestContext, type Page } from '@playwright/test';
import {
  AUTH_TOKEN_KEY,
  CANVAS_POSITION_PREFIX,
  IDENTITY_HINT_KEY,
  SESSION_KEY_PREFIX,
} from '../src/utils/identityStorage';

// Issue #1144 (ledger UX-02): browser-wide drafts and chat state used to
// survive logout and could be read by the next account on the same browser.
//
// These specs drive the REAL two-login flow against the shipped personal-mode
// server: `POST /api/users` mints bcrypt accounts (personal mode trust model),
// the login form mints a session, drafts are typed into the real CodeMirror
// editor, and the chat transcript is written through the real SSE handler
// (mocked at the network layer, exactly like chat-tool-cards.spec.ts).
// No storage helper is stubbed — every assertion reads what the app wrote.
//
// Per-user bucket keys are asserted as LITERALS (`oxo_session::user:<name>`)
// on purpose: deriving them from `identityScope()` would make the isolation
// tests pass even if that function were broken (review finding D4).

const PASSWORD = 'ux02-e2e-pass';

let seq = 0;
/** Unique per run: the e2e SQLite database persists between invocations. */
function uniqueName(prefix: string): string {
  seq += 1;
  return `${prefix}_${Date.now().toString(36)}${seq}`;
}

/** Literal bucket key — deliberately NOT computed by the module under test. */
function userBucket(username: string): string {
  return `${SESSION_KEY_PREFIX}::user:${username}`;
}

async function createAccount(request: APIRequestContext, username: string): Promise<void> {
  const resp = await request.post('/api/users', {
    data: { username, role: 'user', password: PASSWORD },
  });
  expect(resp.ok(), `create user ${username}: HTTP ${resp.status()} ${await resp.text()}`).toBeTruthy();
}

async function signIn(page: Page, username: string): Promise<void> {
  await page.goto('/login');
  await page.locator('input[autocomplete="username"]').fill(username);
  await page.locator('input[type="password"]').fill(PASSWORD);
  await page.locator('button[type="submit"]').click();
  // Layout refreshes the identity chip only after /api/auth/me confirms the
  // session, so this waits for the whole login to settle.
  await expect(page.locator('.header-user[title="Signed in"]')).toHaveText(username, { timeout: 15_000 });
}

async function signOut(page: Page): Promise<void> {
  await page.locator('button.header-user', { hasText: 'Sign out' }).click();
  await expect(page).toHaveURL(/\/login\?reason=signed-out/, { timeout: 15_000 });
}

/** Type TOML into the editor's CodeMirror pane and wait for validation. */
async function typeDraft(page: Page, toml: string): Promise<void> {
  await page.locator('.cm-content').click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.insertText(toml);
  await expect(page.locator('.val-badge')).toContainText('Valid', { timeout: 20_000 });
}

function draftToml(workflowName: string): string {
  return [
    '[workflow]',
    `name = "${workflowName}"`,
    'version = "1.0.0"',
    '',
    '[[rules]]',
    `name = "${workflowName.replace(/-/g, '_')}"`,
    'output = ["out.txt"]',
    'shell = "echo hi > out.txt"',
  ].join('\n');
}

const CHAT_SSE = [
  'event: ready',
  'data: {"mode":"chat","idle_timeout_secs":30}',
  '',
  'event: text',
  'data: {"chunk":"noted"}',
  '',
  'event: done',
  'data: {"mode":"chat","message":"noted"}',
  '',
].join('\n');

/** Send one chat turn through the real ChatUI stream handler (mocked wire). */
async function sendChat(page: Page, text: string): Promise<void> {
  await page.route('**/api/ai/config', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ provider: 'stub', model: 'stub', api_url: '', is_configured: true }),
    }),
  );
  await page.route('**/api/chat/send', (route) =>
    route.fulfill({ status: 200, contentType: 'text/event-stream', body: CHAT_SSE }),
  );
  await page.goto('/chat');
  const input = page.locator('textarea.intent-input');
  await expect(input).toBeEnabled({ timeout: 15_000 });
  await input.fill(text);
  await page.locator('button[aria-label="Send message"]').click();
  await expect(page.locator('.chat-container')).toContainText('noted', { timeout: 15_000 });
  await expect(page.locator('.chat-container')).toContainText(text);
}

/** Every key the app persists as identity-scoped client state. */
async function identityScopedKeys(page: Page): Promise<string[]> {
  // Prefixes come from the shipped module so this assertion cannot drift.
  return page.evaluate(
    ({ sessionPrefix, canvasPrefix, tokenKey, hintKey }) => {
      const scoped = (key: string): boolean =>
        key === tokenKey ||
        key === sessionPrefix ||
        key.startsWith(`${sessionPrefix}::`) ||
        key.startsWith(canvasPrefix) ||
        key === hintKey;
      return Object.keys(localStorage).filter(scoped);
    },
    {
      sessionPrefix: SESSION_KEY_PREFIX,
      canvasPrefix: CANVAS_POSITION_PREFIX,
      tokenKey: AUTH_TOKEN_KEY,
      hintKey: IDENTITY_HINT_KEY,
    },
  );
}

/** Keys of every signed-in identity bucket (the guest bucket is not one). */
async function userBucketKeys(page: Page): Promise<string[]> {
  return page.evaluate(
    (prefix) => Object.keys(localStorage).filter((key) => key.startsWith(`${prefix}::user:`)),
    SESSION_KEY_PREFIX,
  );
}

test('logout wipes identity-scoped state and a second login cannot read it', async ({ page, request }) => {
  // Canvas view so the TOML pane (the draft surface) is mounted. Editor mode
  // is a per-device preference, deliberately not identity-scoped.
  await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));

  const userA = uniqueName('ux02_a');
  const userB = uniqueName('ux02_b');
  await createAccount(request, userA);
  await createAccount(request, userB);

  const markerA = uniqueName('ux02-draft-a');
  const chatA = `ux02-chat-marker-${uniqueName('a')}`;
  const bucketA = userBucket(userA);

  // ── 1. Identity A writes a draft and a chat turn ──
  await signIn(page, userA);
  expect(await page.evaluate((key) => localStorage.getItem(key), IDENTITY_HINT_KEY)).toBe(userA);

  await page.goto('/editor');
  await typeDraft(page, draftToml(markerA));
  // Independent key-shape assertion: the literal bucket for this username
  // must hold the draft (guards against a helper that collapses every
  // identity to the guest scope). Polled — the write is debounced 500 ms.
  await expect
    .poll(async () => page.evaluate((key) => localStorage.getItem(key) ?? '', bucketA), { timeout: 15_000 })
    .toContain(markerA);
  expect(await page.evaluate(() => Object.keys(localStorage))).toContain(bucketA);

  await sendChat(page, chatA);
  await expect
    .poll(async () => page.evaluate((key) => localStorage.getItem(key) ?? '', bucketA), { timeout: 15_000 })
    .toContain(chatA);

  // ── 2. Logout removes every identity-scoped key ──
  await signOut(page);
  // Give the unload flush and the login page's provider a chance to run: a
  // suppressed-but-leaking flush would recreate a bucket here.
  await page.waitForTimeout(800);
  expect(await identityScopedKeys(page)).toEqual([]);
  expect(await page.evaluate((key) => localStorage.getItem(key), bucketA)).toBeNull();
  expect(await page.evaluate((key) => localStorage.getItem(key), AUTH_TOKEN_KEY)).toBeNull();

  // ── 3. Identity B logs in on the same browser ──
  await signIn(page, userB);
  expect(await page.evaluate((key) => localStorage.getItem(key), IDENTITY_HINT_KEY)).toBe(userB);
  expect(await page.evaluate((key) => localStorage.getItem(key), bucketA)).toBeNull();

  // B's editor shows B's (default) workflow — never A's draft.
  await page.goto('/editor');
  await expect(page.locator('.cm-content')).toBeVisible({ timeout: 15_000 });
  await expect(page.locator('.cm-content')).not.toContainText(markerA);
  // The editor's own draft lands in B's literal bucket (debounced write).
  await expect
    .poll(async () => page.evaluate((key) => Object.keys(localStorage).includes(key), userBucket(userB)), { timeout: 15_000 })
    .toBe(true);

  // B's chat page starts empty — never A's transcript.
  await page.goto('/chat');
  await expect(page.locator('textarea.intent-input')).toBeEnabled({ timeout: 15_000 });
  await expect(page.locator('.chat-container')).not.toContainText(chatA);
});

test('switching accounts without signing out re-keys the live session', async ({ page, request }) => {
  await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));

  const userA = uniqueName('ux02_sw_a');
  const userB = uniqueName('ux02_sw_b');
  await createAccount(request, userA);
  await createAccount(request, userB);

  const markerA = uniqueName('ux02-switch-a');
  const bucketA = userBucket(userA);
  const bucketB = userBucket(userB);

  await signIn(page, userA);
  await page.goto('/editor');
  await typeDraft(page, draftToml(markerA));
  await expect
    .poll(async () => page.evaluate((key) => localStorage.getItem(key) ?? '', bucketA), { timeout: 15_000 })
    .toContain(markerA);

  // B signs in from the login page while A is still signed in — no logout.
  await signIn(page, userB);
  expect(await page.evaluate((key) => localStorage.getItem(key), IDENTITY_HINT_KEY)).toBe(userB);

  await page.goto('/editor');
  await expect(page.locator('.cm-content')).toBeVisible({ timeout: 15_000 });
  await expect(page.locator('.cm-content')).not.toContainText(markerA);
  // The live session is B's bucket; A's bucket is preserved on disk (only an
  // explicit sign-out wipes it) and is never rendered for B.
  await expect
    .poll(async () => page.evaluate((key) => Object.keys(localStorage).includes(key), bucketB), { timeout: 15_000 })
    .toBe(true);
  expect(await page.evaluate((key) => localStorage.getItem(key), bucketA)).toContain(markerA);
});

/**
 * Record whether each marker EVER reaches the rendered document.
 *
 * The observer must not be attached at `document_start` — `documentElement`
 * can still be null there, which throws and left the previous version of this
 * probe `false` by construction (review finding). Attach after DOM readiness
 * and back it with a 25 ms sampler. `owner` is the anti-vacuity control: if
 * the probe were dead, `owner` would stay false and every assertion using it
 * fails.
 */
async function installPaintProbe(
  page: Page,
  markers: { foreign: string; owner: string },
): Promise<void> {
  await page.addInitScript((m: { foreign: string; owner: string }) => {
    const seen = { foreign: false, owner: false };
    (window as unknown as { __ux02Probe: { foreign: boolean; owner: boolean } }).__ux02Probe = seen;
    const sample = () => {
      const root = document.documentElement;
      if (!root) return;
      const text = root.innerText ?? '';
      if (!text) return;
      if (text.includes(m.foreign)) seen.foreign = true;
      if (text.includes(m.owner)) seen.owner = true;
    };
    const attach = () => {
      const root = document.documentElement;
      if (!root) {
        setTimeout(attach, 10);
        return;
      }
      new MutationObserver(sample).observe(root, {
        subtree: true,
        childList: true,
        characterData: true,
      });
      sample();
    };
    attach();
    setInterval(sample, 25);
  }, markers);
}

async function paintedMarkers(page: Page): Promise<{ foreign: boolean; owner: boolean }> {
  return page.evaluate(
    () => (window as unknown as {
      __ux02Probe?: { foreign: boolean; owner: boolean };
    }).__ux02Probe ?? { foreign: false, owner: false },
  );
}

/** Sign in as A, write a draft, then forge the hint to B + plant B's bucket. */
async function seedForgedHintScenario(
  page: Page,
  request: APIRequestContext,
  names: { owner: string; foreign: string },
): Promise<{ userA: string; markerA: string; markerB: string; bucketA: string; bucketB: string }> {
  const userA = names.owner;
  const userB = names.foreign;
  await createAccount(request, userA);
  await createAccount(request, userB);

  const markerA = uniqueName('ux02-owner-a');
  const markerB = uniqueName('ux02-foreign-b');
  const bucketA = userBucket(userA);
  const bucketB = userBucket(userB);

  await signIn(page, userA);
  await page.goto('/editor');
  await typeDraft(page, draftToml(markerA));
  await expect
    .poll(async () => page.evaluate((key) => localStorage.getItem(key) ?? '', bucketA), { timeout: 15_000 })
    .toContain(markerA);

  // Corrupt the cached identity (simulating a client that failed to clear it)
  // and plant a distinguishable foreign bucket. The session token still
  // belongs to A.
  await page.evaluate(
    ({ hintKey, hint, foreignKey, foreignToml }) => {
      localStorage.setItem(hintKey, hint);
      localStorage.setItem(foreignKey, JSON.stringify({
        _stamp: 500,
        pipelineToml: foreignToml,
        dagData: null,
        activeRunId: null,
        chatMessages: { dashboard: [], editor: [], monitor: [], report: [] },
      }));
    },
    { hintKey: IDENTITY_HINT_KEY, hint: userB, foreignKey: bucketB, foreignToml: draftToml(markerB) },
  );

  return { userA, markerA, markerB, bucketA, bucketB };
}

test("a stale identity hint never paints a foreign bucket's draft", async ({ page, request }) => {
  await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
  const { userA, markerA, markerB } = await seedForgedHintScenario(page, request, {
    owner: uniqueName('ux02_sa'),
    foreign: uniqueName('ux02_sb'),
  });

  await installPaintProbe(page, { foreign: markerB, owner: markerA });
  await page.reload();

  // The confirmed identity is the token's owner: the app re-keys from the
  // forged hint and renders A's own draft.
  await expect
    .poll(() => page.evaluate((key) => localStorage.getItem(key), IDENTITY_HINT_KEY), { timeout: 20_000 })
    .toBe(userA);
  await expect(page.locator('.header-user[title="Signed in"]')).toHaveText(userA, { timeout: 20_000 });
  await expect(page.locator('.cm-content')).toContainText(markerA, { timeout: 20_000 });
  await expect(page.locator('.cm-content')).not.toContainText(markerB);

  const painted = await paintedMarkers(page);
  expect(painted.foreign, `foreign TOML reached the DOM: ${JSON.stringify(painted)}`).toBe(false);
  // Anti-vacuity control: the probe must have seen the owner's draft.
  expect(painted.owner, `paint probe never observed the owner draft: ${JSON.stringify(painted)}`).toBe(true);
});

test('an unconfirmed identity fails closed: no foreign paint even past the cap', async ({ page, request }) => {
  await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
  const { userA, markerA, markerB } = await seedForgedHintScenario(page, request, {
    owner: uniqueName('ux02_da'),
    foreign: uniqueName('ux02_db'),
  });

  // Hold the provider's confirmation request past the 5 s cap. Only the
  // first `/api/auth/me` is held; later ones (Layout after adoption) pass.
  let release: (() => void) | null = null;
  let seen = 0;
  await page.route('**/api/auth/me', async (route) => {
    seen += 1;
    if (seen === 1) {
      await new Promise<void>((resolve) => { release = resolve; });
    }
    await route.continue();
  });

  await installPaintProbe(page, { foreign: markerB, owner: markerA });
  await page.reload();

  // Past the cap the app must fail CLOSED. The probe is asserted FIRST (a
  // build that falls back to the cached identity paints the foreign bucket
  // here, so this is the assertion that must fail on the old behavior), then
  // the notice and the absence of any editor.
  await page.waitForTimeout(7000);
  const whileUnconfirmed = await paintedMarkers(page);
  expect(whileUnconfirmed.foreign, `foreign TOML painted before confirmation: ${JSON.stringify(whileUnconfirmed)}`).toBe(false);
  expect(whileUnconfirmed.owner).toBe(false);
  await expect(page.getByTestId('identity-unconfirmed')).toBeVisible({ timeout: 10_000 });
  await expect(page.locator('.cm-content')).toHaveCount(0);

  // A late answer is still adopted: the token belongs to A, so the notice
  // gives way to A's own draft.
  expect(release, 'the confirmation request was never intercepted').not.toBeNull();
  (release as () => void)();
  await expect(page.locator('.header-user[title="Signed in"]')).toHaveText(userA, { timeout: 20_000 });
  await expect(page.locator('.cm-content')).toContainText(markerA, { timeout: 20_000 });
  const settled = await paintedMarkers(page);
  expect(settled.foreign, `foreign TOML reached the DOM: ${JSON.stringify(settled)}`).toBe(false);
  // Anti-vacuity control.
  expect(settled.owner).toBe(true);
});

test('an unreachable identity check fails closed and retry adopts the confirmed owner', async ({ page, request }) => {
  await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
  const { userA, markerA, markerB } = await seedForgedHintScenario(page, request, {
    owner: uniqueName('ux02_ua'),
    foreign: uniqueName('ux02_ub'),
  });

  await page.route('**/api/auth/me', (route) => route.abort('failed'));
  await installPaintProbe(page, { foreign: markerB, owner: markerA });
  await page.reload();

  // Probe first (see the delayed case): a cached-identity fallback paints the
  // foreign bucket, so this assertion is the one that must fail on it.
  await page.waitForTimeout(2000);
  const whileUnconfirmed = await paintedMarkers(page);
  expect(whileUnconfirmed.foreign, `foreign TOML painted without confirmation: ${JSON.stringify(whileUnconfirmed)}`).toBe(false);
  expect(whileUnconfirmed.owner).toBe(false);
  await expect(page.getByTestId('identity-unconfirmed')).toBeVisible({ timeout: 10_000 });
  await expect(page.locator('.cm-content')).toHaveCount(0);

  // The cached identity must not have been trusted as a fallback.
  expect(await page.evaluate((key) => localStorage.getItem(key), IDENTITY_HINT_KEY)).not.toBe(userA);

  // Retry with the server reachable again: only the confirmed owner renders.
  await page.unroute('**/api/auth/me');
  await page.getByRole('button', { name: /^Retry$/ }).click();
  await expect(page.locator('.header-user[title="Signed in"]')).toHaveText(userA, { timeout: 20_000 });
  await expect(page.locator('.cm-content')).toContainText(markerA, { timeout: 20_000 });
  await expect(page.locator('.cm-content')).not.toContainText(markerB);
  const settled = await paintedMarkers(page);
  expect(settled.foreign, `foreign TOML reached the DOM: ${JSON.stringify(settled)}`).toBe(false);
  expect(settled.owner).toBe(true);
});

test('signing out in one tab is not undone by a background tab', async ({ page, context, request }) => {
  await page.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));

  const userA = uniqueName('ux02_tab_a');
  await createAccount(request, userA);
  const markerA = uniqueName('ux02-tab-draft');
  const bucketA = userBucket(userA);

  await signIn(page, userA);
  await page.goto('/editor');
  await typeDraft(page, draftToml(markerA));
  await expect
    .poll(async () => page.evaluate((key) => localStorage.getItem(key) ?? '', bucketA), { timeout: 15_000 })
    .toContain(markerA);

  // A second tab of the same browser profile: shared localStorage, live
  // in-memory copy of A's draft.
  const background = await context.newPage();
  await background.addInitScript(() => localStorage.setItem('oxo_editor_mode', 'canvas'));
  await background.goto('/editor');
  await expect(background.locator('.header-user[title="Signed in"]')).toHaveText(userA, { timeout: 20_000 });
  await expect(background.locator('.cm-content')).toContainText(markerA, { timeout: 20_000 });

  // Foreground tab signs out. The background tab hears the identity change,
  // reloads — and its unload flush must NOT rewrite the bucket the logout
  // just removed (review finding D1).
  await signOut(page);
  await expect.poll(async () => background.evaluate((key) => localStorage.getItem(key), bucketA), {
    timeout: 20_000,
  }).toBeNull();

  // Closing the background tab fires the flush again; nothing may come back.
  await background.close();
  await page.waitForTimeout(1000);
  expect(await page.evaluate((key) => localStorage.getItem(key), bucketA)).toBeNull();
  expect(await userBucketKeys(page)).toEqual([]);
  expect(await page.evaluate((key) => localStorage.getItem(key), AUTH_TOKEN_KEY)).toBeNull();
  expect(await page.evaluate((key) => localStorage.getItem(key), IDENTITY_HINT_KEY)).toBeNull();
});
