import { build } from 'esbuild';
import { chromium } from 'playwright-core';
import fs from 'node:fs';
import path from 'node:path';
const bundle = await build({ stdin: { contents: `export { TableDesigner } from './src/ui/TableDesigner'; export { openTableOperationDialog } from './src/dialogs/TableOperationDialog'; export * from './src/core/table-design-draft';`, resolveDir: process.cwd(), sourcefile: 'table-designer-browser-fixture.ts' }, bundle: true, platform: 'browser', format: 'iife', globalName: 'DesignerModules', loader: { '.css': 'empty' }, write: false, logLevel: 'silent', plugins: [{ name: 'isolated-api', setup(plugin) { plugin.onResolve({ filter: /(?:^|\/)core\/api$|^\.\/api$/ }, () => ({ path: 'api', namespace: 'test-api' })); plugin.onLoad({ filter: /.*/, namespace: 'test-api' }, () => ({ contents: 'export const api = globalThis.__api;', loader: 'js' })); } }] });
export async function launchDesignerBrowser() {
  const executablePath = [process.env.NAVKIT_TEST_BROWSER, 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', 'C:/Program Files/Microsoft/Edge/Application/msedge.exe', 'C:/Program Files/Google/Chrome/Application/chrome.exe', '/usr/bin/chromium', '/usr/bin/chromium-browser'].filter(Boolean).find(file => fs.existsSync(file));
  return chromium.launch({ headless: true, ...(executablePath ? { executablePath } : {}) });
}
export async function launchOwnedDesignerBrowser(onServer = () => {}) {
  const executablePath = [process.env.NAVKIT_TEST_BROWSER, 'C:/Program Files/Google/Chrome/Application/chrome.exe', 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', 'C:/Program Files/Microsoft/Edge/Application/msedge.exe', '/usr/bin/chromium', '/usr/bin/chromium-browser'].filter(Boolean).find(file => fs.existsSync(file));
  const server = await chromium.launchServer({ headless: true, ...(executablePath ? { executablePath } : {}) });
  onServer(server);
  const browser = await chromium.connect(server.wsEndpoint(), { timeout: 30_000 });
  return { browser, server };
}
export async function designerPage(browser, kind = 'mysql') {
  const page = await browser.newPage({ viewport: { width: 1500, height: 980 } }); page.setDefaultTimeout(5000);
  await page.setContent('<button id="outside">外部焦点</button><div id="designer"></div>');
  await page.addStyleTag({ content: fs.readFileSync(path.resolve('src/styles/table-designer.css'), 'utf8') + '\n.dialog-backdrop{position:fixed;inset:0;background:#ddd8;display:grid;place-items:center}.dialog{background:white;border:1px solid #888;padding:12px}.dialog-footer{display:flex;gap:8px}.dialog-body{max-height:65vh;overflow:auto}' });
  await page.evaluate(kind => {
    let uuid = 0; if (!crypto.randomUUID) crypto.randomUUID = () => `12345678-1234-4123-8123-${String(++uuid).padStart(12, '0')}`;
    const feature = { supported: true, reason: null };
    const columns = [
      { name: 'id', orig_name: 'id', data_type: 'bigint', length: null, scale: null, nullable: false, default_value: null, auto_increment: true, unsigned: true, zerofill: false, comment: null },
      { name: 'title', orig_name: 'title', data_type: "enum('a,b','c)')", length: null, scale: null, nullable: false, default_value: "'a,b'", auto_increment: false, unsigned: false, zerofill: false, comment: '完整注释' },
      { name: 'computed', orig_name: 'computed', data_type: 'int', length: null, scale: null, nullable: true, default_value: null, auto_increment: false, unsigned: false, zerofill: false, comment: null },
      { name: 'updated', orig_name: 'updated', data_type: 'timestamp', length: '3', scale: null, nullable: false, default_value: 'CURRENT_TIMESTAMP(3)', auto_increment: false, unsigned: false, zerofill: false, comment: null },
    ];
    const state = globalThis.fixture = { kind, mode: 'success', previews: [], executions: [], grants: [], notices: [], refreshed: [], referenceCalls: [], snapshotError: false, holdPreview: false, releasePreview: null, controllerChanges: 0 };
    state.snapshot = { schema: { database: 'db', table: 'items', columns: columns.map(c => ({ name: c.name, type: c.data_type, length: c.length === null ? null : Number(c.length), scale: null, nullable: c.nullable, primary_key: ['title', 'id'].includes(c.name), auto_increment: c.auto_increment, default: c.default_value, comment: c.comment })), indexes: [{ name: 'PRIMARY', columns: ['title', 'id'], primary: true, unique: true }], foreign_keys: [], engine: 'InnoDB', collation: 'utf8mb4_bin', comment: '原表注释' }, objects: { checks: [], triggers: [], completeness: 'complete', limitations: [] }, options: { engine: 'InnoDB', charset: 'utf8mb4', collation: 'utf8mb4_bin' }, design: { schema: 'db', table: 'items', columns, primary_key: ['title', 'id'], indexes: [{ name: 'title_idx', columns: ['title'], unique: false }], foreign_keys: [], checks: [], triggers: [], options: { engine: 'InnoDB', charset: 'utf8mb4', collation: 'utf8mb4_bin' }, comment: '原表注释' }, revision: 'r1', source_fingerprint: 'f1', owner_id: 't', completeness: 'complete', capabilities: { checks: feature, triggers: feature, options: feature, rebuild: feature, clear: feature, copy_structure: feature, copy_data: feature, supported_options: kind === 'sqlite' ? ['strict', 'without_rowid'] : ['engine', 'charset', 'collation', 'row_format', 'fillfactor'], limitations: [] } };
    const clone = value => structuredClone(value);
    const outcome = request => ({ execution_id: request.execution_id, state: state.mode === 'unknown' ? 'unknown' : 'succeeded', result: null, error: null, deadline: { execution_id: request.execution_id, budget_ms: null, elapsed_ms: 1, deadline_reached: false, stop_confirmed: false }, effects: state.mode === 'unknown' ? 'possibly_applied' : 'completed', transaction_state: state.mode === 'unknown' ? 'unknown' : 'none', resources_pending: false });
    globalThis.__api = {
      readTableDesignSnapshot: async (_id, _schema, _table, owner) => { if (state.snapshotError) throw new Error('metadata unavailable'); const snapshot = clone(state.snapshot); snapshot.owner_id = owner; return snapshot; },
      listDatabases: async connection => { state.referenceCatalogCalls ??= []; state.referenceCatalogCalls.push({ connection, kind: 'namespaces' }); return [['db', 'database'], ['other_schema', 'database'], ['empty_schema', 'database']]; },
      listTables: async (connection, schema) => { state.referenceCatalogCalls ??= []; state.referenceCatalogCalls.push({ connection, schema, kind: 'tables' }); return schema === 'empty_schema' ? [] : [['parents', 'table'], ['parents_view', 'view']]; },
      getTableSchema: async (connection, schema, table) => { state.referenceCalls.push({ connection, schema, table }); return { columns: [{ name: 'parent_a' }, { name: 'parent_b' }] }; },
      previewTableDesign: async (_id, design, owner) => { state.previews.push(clone(design)); if (state.mode === 'cas') throw new Error('BASE_REVISION_CONFLICT'); if (state.holdPreview) await new Promise(resolve => { state.releasePreview = resolve; }); return { request_sql: JSON.stringify({ owner, design }), steps: [{ label: 'fixture backend plan', sql: 'ALTER TABLE backend_actual_plan;' }], destructive: false, revision: state.snapshot.revision, capabilities: state.snapshot.capabilities, limitations: ['fixture plan only; no database connected'] }; },
      issueGrant: async request => { state.grants.push(clone(request)); return { ...request, execution_grant: 'fixture-controlled-grant' }; },
      executeTableDesign: async (request, design) => { state.executions.push({ request: clone(request), design: clone(design) }); const next = clone(state.snapshot); next.design = { ...next.design, ...clone(design) }; next.design.columns = next.design.columns.map(c => ({ ...c, orig_name: c.name })); next.revision = 'r2'; next.source_fingerprint = 'f2'; next.schema.columns = design.columns.map(c => ({ name: c.name, type: c.data_type, nullable: c.nullable, primary_key: design.primary_key.includes(c.name), auto_increment: c.auto_increment, default: c.default_value, comment: c.comment })); if (state.mode === 'success') state.snapshot = clone(next); return { outcome: outcome(request), receipt: { verified: state.mode === 'success', unverified: state.mode !== 'success', mismatches: state.mode === 'success' ? [] : ['readback incomplete'], steps: [{ label: 'apply', sql: 'backend_actual_plan', status: state.mode === 'partial' ? 'failed' : 'applied' }], transaction: state.mode === 'unknown' ? 'unknown' : 'committed', schema: next.schema, snapshot: next } }; },
      previewTableOperation: async (operation, owner) => { state.previews.push(clone(operation)); return { request_sql: JSON.stringify({ owner, operation }), revision: state.snapshot.revision, operation: { kind: operation.kind, table: operation.table, schema: operation.schema, connection_id: operation.connection_id, source_fingerprint: operation.source_fingerprint, base_revision: operation.base_revision, target_table: operation.target_table }, steps: [{ label: 'fixture backend operation', sql: 'BACKEND_OPERATION_PLAN;' }], destructive: true, capabilities: state.snapshot.capabilities, limitations: [] }; },
      executeTableOperation: async (request, operation) => { state.executions.push({ request: clone(request), operation: clone(operation) }); return { outcome: outcome(request), receipt: { kind: operation.kind, source_table: operation.table, target_table: operation.target_table ?? null, affected_rows: 7, copied_rows: operation.kind === 'copy_data' ? 7 : 0, verified: state.mode === 'success', mismatches: state.mode === 'success' ? [] : ['readback incomplete'], transaction: state.mode === 'unknown' ? 'unknown' : 'committed', steps: [{ label: 'apply', sql: 'BACKEND_OPERATION_PLAN', status: 'applied' }], snapshot: null } }; },
      cancelExecution: async context => ({ execution_id: context.execution_id, accepted: false, state: 'unknown' }),
      executionStatus: async context => ({ execution_id: context.execution_id, state: 'unknown', resources_pending: false }),
    };
  }, kind);
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  await page.evaluate(async () => {
    const f = globalThis.fixture;
    globalThis.tab = { id: 't', connectionId: 'c', schema: 'db', object: 'items', kind: 'structure', dirty: false };
    globalThis.designer = new DesignerModules.TableDesigner(globalThis.tab, { connection: () => ({ id: 'c', kind: f.kind, params: '', read_only: !!f.readOnly }), writeReason: () => f.readOnly ? '只读' : f.transaction ? '手动事务' : null, changed: () => { f.controllerChanges++; }, status() {}, toast: (kind, text) => f.notices.push([kind, text]) }, document.querySelector('#designer'));
    await designer.reload();
  });
  return page;
}
export async function finishSave(page) {
  await page.getByRole('button', { name: '确认执行', exact: true }).click();
  await page.waitForFunction(() => !globalThis.designer.draftBusy());
}
