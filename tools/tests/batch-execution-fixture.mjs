// 真实产品模块的传输 fixture；没有原生数据库，也不提供旧写入回落。
import { build } from 'esbuild';
import { createServer } from 'node:http';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';

export const root = fileURLToPath(new URL('../../', import.meta.url));
export const flush = async () => { for (let i = 0; i < 24; i++) await Promise.resolve(); };
export const requestFor = (kind = 'sql_file') => ({ connection_id: 'fixture-connection', namespace: 'main', owner_id: 'fixture-owner', task: kind === 'sql_file'
  ? { kind, path: 'D:/fixtures/SQL 文件.sql', encoding: 'utf-8', stop_on_error: true }
  : { kind, request: { connection_id: 'fixture-connection', schema: 'main', table: 'items', columns: ['id'], rows: [['1'], ['2']] } } });

export function createBatchTransport() {
  const calls = [], tasks = new Map();
  let release, releaseRun;
  const source = {
    calls, tasks, mode: 'running',
    releaseRegistration() { release?.(); }, releaseRun() { releaseRun?.(); },
    async ensureConnection(id) { calls.push(['ensure', id]); return { ok: true }; },
    async listDatabases() { return [['main', 'database'], ['other', 'database']]; },
    async listTables() { return [['items', 'table'], ['fixture_view', 'view']]; },
    async getTableSchema() { return { columns: [{ name: 'id', type: 'INTEGER', primary_key: true, nullable: false, auto_increment: false, default: null }], foreign_keys: [], indexes: [] }; },
    async insertGeneratedRows() { throw new Error('禁止旧写入回落'); },
    async registerBatchExecution(request) {
      calls.push(['register', structuredClone(request)]);
      if (source.mode === 'delayed-register') await new Promise(resolve => { release = resolve; });
      const ticket = { execution_id: crypto.randomUUID(), connection_id: request.connection_id, namespace: request.namespace, owner_id: request.owner_id,
        task_kind: request.task.kind, execution_grant: crypto.randomUUID(), expires_in_ms: 60000, summary: '固定目标批任务',
        total_rows: request.task.kind === 'generated_rows' ? request.task.request.rows.length : null, source_bytes: request.task.kind === 'sql_file' ? 8192 : null };
      tasks.set(ticket.execution_id, { ticket, request, started: false, snapshot: null });
      source.setSnapshot(ticket.execution_id, { state: 'queued', pending: true, elapsed: 0 }); return ticket;
    },
    async runBatchExecution(ticket) {
      calls.push(['run', structuredClone(ticket)]);
      const task = tasks.get(ticket.execution_id);
      if (!task || task.started) throw new Error('禁止重新消费票据');
      task.started = true;
      source.setSnapshot(ticket.execution_id, { state: 'running', pending: true, elapsed: 1234 });
      if (source.mode === 'run-reject') throw Object.assign(new Error('RAW_DB_ERROR secret-token'), { code: 'TRANSPORT_LOST' });
      if (source.mode === 'delayed-run') {
        const snapshot = structuredClone(task.snapshot);
        await new Promise(resolve => { releaseRun = resolve; }); return snapshot;
      }
      return structuredClone(task.snapshot);
    },
    async batchExecutionStatus(context, afterSequence) {
      calls.push(['status', structuredClone(context), afterSequence]);
      const snapshot = structuredClone(tasks.get(context.execution_id).snapshot);
      snapshot.logs = snapshot.logs.filter(log => log.sequence > afterSequence); return snapshot;
    },
    async cancelExecution(context) {
      calls.push(['cancel', structuredClone(context)]);
      return { execution_id: context.execution_id, accepted: true, state: 'cancelled', message: 'RAW_DB_ERROR secret-token' };
    },
    setSnapshot(id, options = {}) {
      const task = tasks.get(id), ticket = task.ticket;
      const { state = 'running', pending = true, elapsed = 2345, transaction = ticket.task_kind === 'generated_rows' ? 'active' : 'none', logs = [], dropped = 0, next = logs.at(-1)?.sequence ?? task.snapshot?.next_sequence ?? 0 } = options;
      const context = { execution_id: id, connection_id: ticket.connection_id, namespace: ticket.namespace, owner_id: ticket.owner_id };
      const deadline = { execution_id: id, budget_ms: options.budgetMs ?? null, elapsed_ms: elapsed, deadline_reached: options.deadlineReached ?? state === 'timed_out', stop_confirmed: options.stopConfirmed ?? ['cancelled', 'timed_out'].includes(state) };
      const terminal = !['running', 'queued', 'cancel_requested'].includes(state);
      task.snapshot = { context, kind: ticket.task_kind, status: { execution_id: id, state, resources_pending: pending, deadline },
        metrics: { scanned: options.scanned ?? 7, executed: options.executed ?? 4, errors: options.errors ?? 1, elapsed_ms: elapsed },
        scan_complete: options.scanComplete ?? false, parse_error: options.parseError ?? null, affected_rows: options.affected ?? 3, total_rows: ticket.total_rows,
        transaction_state: transaction, logs, next_sequence: next, dropped_before_sequence: dropped,
        outcome: terminal ? { execution_id: id, state, result: null, error: options.error === undefined ? (state === 'succeeded' ? null : { code: 'FIXTURE_ERROR', message: 'RAW_DB_ERROR secret-token', retryable: false }) : options.error,
          deadline, effects: 'fixture', transaction_state: transaction, resources_pending: pending } : null };
      return task.snapshot;
    },
  };
  return source;
}

// 浏览器仅走显式 mock；原生 bridge 在此 fixture 内禁止调用。
const ipcBoundary = { name: 'explicit-fixture-ipc', setup(builder) {
  builder.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'native-ipc-forbidden', namespace: 'batch-fixture' }));
  builder.onLoad({ filter: /.*/, namespace: 'batch-fixture' }, () => ({ contents: 'export function invoke() { throw new Error("此 fixture 没有原生 IPC"); }', loader: 'js' }));
} };

export async function loadService() {
  const result = await build({ entryPoints: [path.join(root, 'src/core/batchExecution.ts')], bundle: true, write: false, platform: 'node', format: 'esm', plugins: [ipcBoundary], logLevel: 'silent' });
  return import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
}

export async function startBrowserFixture() {
  const temp = await mkdtemp(path.join(tmpdir(), 'navkit-batch-'));
  let server, browser;
  const close = async () => { try { await browser?.close(); } finally { try { if (server?.listening) await new Promise(resolve => server.close(resolve)); } finally { await rm(temp, { recursive: true, force: true }); } } };
  try {
    await build({ stdin: { contents: `
      import { batchExecutionServiceFor } from './src/core/batchExecution.ts';
      import { api } from './src/core/api.ts';
      import { openSqlFileExecutionDialog, openBatchExecutionDialog } from './src/ui/BatchExecutionPanel.ts';
      import { openDataGeneratorDialog } from './src/dialogs/DataGeneratorDialog.ts';
      import { Wizard } from './src/ui/Wizard.ts';
      import './src/styles/base.css';
      const createBatchTransport = ${createBatchTransport.toString()};
      const source = createBatchTransport(), ipcCalls = [];
      const mock = {
        register_batch_execution: ({ request }) => source.registerBatchExecution(request),
        run_batch_execution: ({ ticket }) => source.runBatchExecution(ticket),
        batch_execution_status: ({ context, afterSequence }) => source.batchExecutionStatus(context, afterSequence),
        cancel_execution: ({ executionId, connectionId, ownerId, namespace }) => source.cancelExecution({ execution_id: executionId, connection_id: connectionId, owner_id: ownerId, namespace }),
        ensure_connection: ({ id }) => source.ensureConnection(id),
        list_databases: () => source.listDatabases(), list_tables: () => source.listTables(), get_table_schema: () => source.getTableSchema(),
        insert_generated_rows: () => source.insertGeneratedRows(),
      };
      window.__PREVIEW_MOCK__ = Object.fromEntries(Object.entries(mock).map(([name, handler]) => [name, args => { ipcCalls.push([name, args]); return handler(args); }]));
      const service = batchExecutionServiceFor(api);
      const connection = { id: 'fixture-connection', name: '前端验收目标', kind: 'sqlite', params: '', read_only: false };
      const done = [];
      window.batchFixture = { source, service, connection, done, Wizard, ipcCalls,
        openFile: (readonly = false) => openSqlFileExecutionDialog({ ...connection, read_only: readonly }, 'main', 'D:/fixtures/SQL 文件.sql', service),
        openGenerator: (readonly = false) => openDataGeneratorDialog({ ...connection, read_only: readonly }, 'main', 'items', (...args) => done.push(args), api),
        openRecords: id => openBatchExecutionDialog(service, id),
        snapshot: options => { const id = service.list()[0].context.execution_id; source.setSnapshot(id, options); },
      };
    `, resolveDir: root, loader: 'js' }, bundle: true, platform: 'browser', format: 'iife', outfile: path.join(temp, 'fixture.js'), plugins: [ipcBoundary], logLevel: 'silent' });
    server = createServer(async (req, res) => {
      try {
        if (req.url === '/fixture.js' || req.url === '/fixture.css') {
          res.setHeader('Content-Type', req.url.endsWith('.css') ? 'text/css' : 'text/javascript'); res.end(await readFile(path.join(temp, req.url.slice(1))));
        } else { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end('<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>NavKit 批任务前端验收（mock IPC）</title><link rel="stylesheet" href="/fixture.css"><body><script src="/fixture.js"></script></body></html>'); }
      } catch { res.statusCode = 500; res.end('fixture resource failed'); }
    });
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
    browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
    return { browser, url: `http://127.0.0.1:${server.address().port}`, close };
  } catch (error) { await close(); throw error; }
}
