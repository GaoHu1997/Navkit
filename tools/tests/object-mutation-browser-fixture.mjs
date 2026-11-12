// Real product dialog/controller in Chromium; only the api/IPC boundary is mocked.
// This fixture provides no desktop shell, native IPC, database, or SQL fallback.
import { build } from 'esbuild';
import { chromium } from 'playwright-core';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import assert from 'node:assert/strict';

export const root = fileURLToPath(new URL('../../', import.meta.url));

function installIpcFixture() {
  const copy = value => structuredClone(value);
  const target = { connection_id: 'browser-fixture-connection', catalog: null, schema: 'main', kind: 'trigger', name: 'audit"中文<img>', identity_arguments: null, parent: { schema: 'main', kind: 'table', name: 'items"中文' } };
  const f = window.objectMutationFixture = {
    target, owner: 'browser-fixture-page', calls: [], forbidden: [], nativeCalls: [], plans: [], contextAlive: true, contextChecks: 0, committed: 0,
    holdBaseline: false, holdPreview: false, holdGrant: false, holdExecution: false,
    baselineError: false, previewError: false, previewMismatch: false,
    executeMode: 'success', resourcesPending: false, statusPending: true,
    releaseBaseline: null, releasePreview: null, releaseGrant: null, releaseExecution: null,
  };
  f.baseline = { baseline_id: crypto.randomUUID(), object: copy(target), source_fingerprint: 'a'.repeat(64), base_revision: 'b'.repeat(64), backend_object_id: 'mock-sqlite/root/trigger-ddl', metadata_state: 'known', expires_in_ms: 120000 };
  const record = (name, ...args) => { f.calls.push({ name, args: copy(args) }); };
  const hold = async name => { if (f[`hold${name}`]) await new Promise(resolve => { f[`release${name}`] = resolve; }); };
  const deadline = id => ({ execution_id: id, budget_ms: null, elapsed_ms: 1, deadline_reached: false, stop_confirmed: false });
  const forbidden = name => (...args) => { f.forbidden.push({ name, args: copy(args) }); throw new Error(`Forbidden SQL/native fallback: ${name}`); };
  window.__objectMutationApi = {
    async readObjectMutationBaseline(object, owner) {
      record('baseline', object, owner); await hold('Baseline');
      if (f.baselineError) throw new Error('fixture baseline unavailable');
      f.openingBaseline = copy(f.baseline); return copy(f.baseline);
    },
    async previewObjectMutation(draft, owner) {
      record('preview', draft, owner); await hold('Preview');
      if (f.previewError) throw new Error('fixture preview unavailable');
      const plan = { plan_id: crypto.randomUUID(), baseline_id: draft.baseline_id, object: copy(draft.object), source_fingerprint: draft.source_fingerprint, base_revision: draft.base_revision, review_digest: 'c'.repeat(64), expires_in_ms: 100000, confirmation_required: true, transaction: 'transactional' };
      plan.request_sql = f.previewMismatch ? 'DROP TRIGGER raw_client_fallback' : `OBJECT_MUTATION_DROP ${plan.plan_id} ${plan.review_digest}`;
      f.plans.push(copy(plan)); return copy(plan);
    },
    async issueGrant(request) {
      record('grant', request);
      f.signed = { ...copy(request), execution_id: crypto.randomUUID(), execution_grant: crypto.randomUUID() };
      await hold('Grant'); return copy(f.signed);
    },
    async executeObjectMutation(request, work) {
      record('typed_execute', request, work); await hold('Execution');
      if (f.executeMode === 'lost') throw new Error('fixture lost commit acknowledgement');
      const stopped = request.cancel_before_send === true;
      const unknown = f.executeMode === 'unknown';
      const state = stopped ? 'cancelled' : unknown ? 'unknown' : 'succeeded';
      f.outcome = { execution_id: request.execution_id, state, effects: stopped ? 'no_effect' : unknown ? 'possibly_applied' : 'completed', transaction_state: stopped ? 'none' : unknown ? 'unknown' : 'committed', resources_pending: f.resourcesPending, result: null, error: stopped ? { code: 'STOPPED_BEFORE_SQL', message: 'fixture stopped before SQL', retryable: false } : null, deadline: deadline(request.execution_id) };
      const b = f.openingBaseline;
      const receipt = stopped || unknown ? null : { plan_id: work.plan_id, baseline_id: b.baseline_id, object: copy(b.object), backend_object_id: b.backend_object_id, source_fingerprint: b.source_fingerprint, before_revision: b.base_revision, after_revision: 'd'.repeat(64), status: 'applied', verified: true, transaction: 'committed', mismatches: [] };
      return { outcome: copy(f.outcome), receipt };
    },
    async cancelExecution(context) { record('cancel', context); return { execution_id: context.execution_id, accepted: true, state: 'cancel_requested', message: 'fixture cancel requested; not settlement' }; },
    async executionStatus(context) {
      record('status', context);
      return { execution_id: context.execution_id, state: f.outcome?.state ?? 'unknown', resources_pending: f.statusPending, deadline: deadline(context.execution_id) };
    },
    executeSql: forbidden('execute_sql'), executeSqlV2: forbidden('execute_sql_v2'), executeSqlQuery: forbidden('execute_sql_query'),
  };
}

export async function buildObjectMutationBrowserBundle() {
  const result = await build({
    stdin: { contents: `
      import { openObjectMutationDialog } from './src/dialogs/ObjectMutationDialog';
      export { ObjectMutationSession } from './src/core/object-mutation';
      const f = window.objectMutationFixture;
      f.open = () => {
        f.dialog = openObjectMutationDialog(structuredClone(f.target), f.owner,
          () => { f.contextChecks++; if (!f.contextAlive) throw new Error('fixture 页面归属已变化'); },
          async () => { f.committed++; });
      };
      document.querySelector('#open-object-mutation').addEventListener('click', f.open);
      f.ready = true;
    `, resolveDir: root, sourcefile: 'object-mutation-browser-entry.ts', loader: 'ts' },
    bundle: true, platform: 'browser', format: 'iife', globalName: 'ObjectMutationModules', write: false, metafile: true, logLevel: 'silent',
    plugins: [{ name: 'isolated-object-mutation-ipc', setup(builder) {
      builder.onResolve({ filter: /(?:^|\/)core\/api(?:\.ts)?$|^\.\/api(?:\.ts)?$/ }, () => ({ path: 'api', namespace: 'object-mutation-fixture-api' }));
      builder.onLoad({ filter: /.*/, namespace: 'object-mutation-fixture-api' }, () => ({ contents: 'export const api = window.__objectMutationApi;', loader: 'js' }));
      builder.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'forbidden-native', namespace: 'object-mutation-forbidden-native' }));
      builder.onLoad({ filter: /.*/, namespace: 'object-mutation-forbidden-native' }, () => ({ contents: 'export function invoke(...args) { window.objectMutationFixture.nativeCalls.push(args); throw new Error("Native IPC forbidden in browser fixture"); }', loader: 'js' }));
    } }],
  });
  const inputs = Object.keys(result.metafile.inputs).map(input => input.replaceAll('\\', '/'));
  assert.ok(inputs.some(input => input.endsWith('src/dialogs/ObjectMutationDialog.ts')), 'The real product dialog must be bundled');
  assert.ok(inputs.some(input => input.endsWith('src/core/object-mutation.ts')), 'The real product session must be bundled');
  assert.ok(inputs.some(input => input.endsWith('src/core/execution.ts')), 'The real C1 controller must be bundled');
  assert.ok(!inputs.some(input => input.endsWith('src/core/api.ts')), 'Only the IPC boundary is isolated');
  return { javascript: result.outputFiles[0].text, inputs };
}

export async function startObjectMutationBrowserFixture() {
  const { javascript, inputs } = await buildObjectMutationBrowserBundle();
  const css = await readFile(path.join(root, 'src/styles/base.css'), 'utf8');
  const html = `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>Object mutation: real browser, mock IPC, not desktop</title><link rel="stylesheet" href="/fixture.css"><body><button id="open-object-mutation">打开受控对象删除</button><button id="outside">外部焦点</button><script>(${installIpcFixture.toString()})();</script><script src="/fixture.js"></script></body></html>`;
  let browser;
  const server = createServer((request, response) => {
    if (request.url === '/') { response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }); response.end(html); }
    else if (request.url === '/fixture.js') { response.writeHead(200, { 'Content-Type': 'text/javascript; charset=utf-8' }); response.end(javascript); }
    else if (request.url === '/fixture.css') { response.writeHead(200, { 'Content-Type': 'text/css; charset=utf-8' }); response.end(css); }
    else { response.writeHead(404); response.end(); }
  });
  const close = async () => { try { await browser?.close(); } finally { if (server.listening) await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve())); } };
  try {
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
    const executablePath = [process.env.NAVKIT_TEST_BROWSER, process.env.BROWSER_EXE, 'C:/Program Files/Google/Chrome/Application/chrome.exe', 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', 'C:/Program Files/Microsoft/Edge/Application/msedge.exe', '/usr/bin/chromium', '/usr/bin/chromium-browser'].filter(Boolean).find(existsSync);
    browser = await chromium.launch({ headless: true, ...(executablePath ? { executablePath } : {}) });
    return { browser, url: `http://127.0.0.1:${server.address().port}/`, inputs, close };
  } catch (error) { await close(); throw error; }
}

export async function objectMutationPage(fixture, options = {}) {
  const page = await fixture.browser.newPage({ viewport: { width: 1100, height: 760 } });
  page.setDefaultTimeout(5000);
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  try {
    await page.goto(fixture.url, { waitUntil: 'load', timeout: 15000 });
    await page.waitForFunction(() => window.objectMutationFixture?.ready === true);
    await page.evaluate(options => Object.assign(window.objectMutationFixture, options), options);
    return { page, errors };
  } catch (error) { await page.close(); throw error; }
}
