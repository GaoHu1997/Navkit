import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { randomUUID } from 'node:crypto';

// The actual component, model, evidence validator and clipboard encoder execute below.
// Only the DataGrid renderer is replaced: its index callbacks and setData boundary stay real inputs.
// This is a DOM contract suite; it does not claim a browser, desktop IPC or database run.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const { outputFiles } = await build({ entryPoints: [path.join(root, 'src/ui/QueryResultPanel.ts')], bundle: true, platform: 'node', format: 'esm', target: 'es2021', write: false, loader: { '.css': 'empty' },
  plugins: [{ name: 'grid-render-only', setup(build) {
    build.onResolve({ filter: /^\.\/DataGrid$/ }, args => args.importer.endsWith('QueryResultPanel.ts') ? { path: 'DataGrid', namespace: 'test-grid' } : null);
    build.onLoad({ filter: /.*/, namespace: 'test-grid' }, () => ({ contents: `export class DataGrid {
      constructor(callbacks) { this.callbacks = callbacks; this.el = document.createElement('div'); this.el.className = 'grid-wrap'; (globalThis.__PL21_GRIDS__ ??= []).push(this); }
      setData(value) { this.data = value; this.updates = (this.updates ?? 0) + 1; }
      dispose() { this.disposed = true; }
    }`, loader: 'js' }));
  } }],
});
const componentSource = outputFiles[0].text + '\n//# sourceURL=navkit-query-result-panel-contract.mjs';
const { QueryResultPanel, parseQueryPanelValue } = await import(`data:text/javascript;base64,${Buffer.from(componentSource).toString('base64')}`);

class DomElement {
  constructor(tag, document) { this.tagName = tag.toUpperCase(); this.ownerDocument = document; this.children = []; this.parentNode = null; this.attributes = new Map(); this.listeners = new Map(); this.dataset = {}; this.style = {}; this._text = ''; this.className = ''; this.value = ''; this.type = ''; this.id = ''; this.hidden = false; this.disabled = false; this.checked = false;
    this.classList = { add: (...names) => { this.className = [...new Set([...this.className.split(/\s+/).filter(Boolean), ...names])].join(' '); } };
  }
  set textContent(value) { this._text = String(value); this.children.forEach(child => { child.parentNode = null; }); this.children = []; }
  get textContent() { return this._text + this.children.map(child => child.textContent).join(''); }
  append(...children) { for (const child of children) { assert.ok(child instanceof DomElement, 'only actual node-shaped children'); if (child.parentNode) child.remove(); child.parentNode = this; this.children.push(child); } }
  replaceChildren(...children) { this.children.forEach(child => { child.parentNode = null; }); this.children = []; this._text = ''; this.append(...children); }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); this.parentNode = null; }
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  removeAttribute(name) { this.attributes.delete(name); }
  addEventListener(type, fn) { const list = this.listeners.get(type) ?? []; list.push(fn); this.listeners.set(type, list); }
  dispatch(type, detail = {}) { const event = { type, target: this, preventDefault() { this.defaultPrevented = true; }, stopPropagation() {}, ...detail }; for (const fn of this.listeners.get(type) ?? []) fn(event); this[`on${type}`]?.(event); return event; }
  click() { if (!this.disabled) this.dispatch('click'); }
  focus() { this.ownerDocument.activeElement = this; }
  matches(selector) {
    const tag = selector.match(/^[a-z]+/i)?.[0]; if (tag && this.tagName !== tag.toUpperCase()) return false;
    const cls = selector.match(/\.([\w-]+)/)?.[1]; if (cls && !this.className.split(/\s+/).includes(cls)) return false;
    const attr = selector.match(/\[([\w-]+)(?:=['"]?([^'"\]]+)['"]?)?\]/); if (attr) { const actual = this[attr[1]] ?? this.getAttribute(attr[1]); if (attr[2] === undefined ? actual == null : String(actual) !== attr[2]) return false; }
    return !!(tag || cls || attr);
  }
  querySelectorAll(selector) { return descendants(this).filter(node => selector.split(',').some(part => node.matches(part.trim()))); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}
function descendants(node) { return node.children.flatMap(child => [child, ...descendants(child)]); }
function installDom() { const doc = { activeElement: null, createElement(tag) { return new DomElement(tag, this); } }; doc.body = doc.createElement('body'); globalThis.document = doc; globalThis.__PL21_GRIDS__ = []; return doc; }
const integer = value => ({ kind: 'integer', value });
const decimal = value => ({ kind: 'decimal', value });
const text = value => ({ kind: 'text', value });
const nil = () => ({ kind: 'null' });
const binary = value => ({ kind: 'binary', value });
const revision = 'a'.repeat(64);
const ctx = { source_execution_id: 'executed-source', result_index: 0, generation: 'server-generation' };
function snapshot() {
  const columns = ['id', 'label', 'amount', 'payload', 'computed'].map((name, i) => ({ id: `c${i}`, name, data_type: ['INTEGER', 'TEXT', 'TEXT', 'BLOB', 'TEXT'][i] }));
  return { ...ctx, columns, total_rows: 500_000, rows: [
    { token: 'view-a', row_token: 'server-row-a', values: [integer('1'), text('alpha'), decimal('9007199254740993.000000000000000001'), binary('AAD/'), text('generated-a')] },
    { token: 'view-b', row_token: 'server-row-b', values: [integer('2'), text('Beta'), decimal('9007199254740992.999999999999999999'), binary(''), text('generated-b')] },
    { token: 'view-c', row_token: 'server-row-c', values: [integer('3'), text('gamma'), nil(), binary('AA=='), text('generated-c')] },
  ], editability: { ...ctx, result_token: 'opaque-result', mode: 'single_table', reason: '', schema: 'main', table: 'items', metadata_revision: revision,
    columns: columns.map((column, i) => ({ result_column_id: column.id, result_index: i, name: column.name, base_column: column.name, data_type: column.data_type,
      writable: i !== 0 && i !== 4, key: i === 0, generated: i === 4, reason: i === 0 ? '定位键只读' : i === 4 ? '生成列只读' : '' })), key_column_ids: ['c0'],
    actions: { update: { supported: true, reason: '' }, delete: { supported: true, reason: '' }, insert: { supported: true, reason: '' } },
  } };
}
function makePlan(overrides = {}) { return { plan_id: randomUUID(), sql: 'UPDATE "items" SET "label"=? WHERE "id"=? AND "label"=?', binds: [text('new'), integer('1'), text('alpha')], base_revision: revision, expected_rows: 1, summary: '真实后端计划的测试回执', ...overrides }; }
function makeReceipt(plan, overrides = {}) { return { plan_id: plan.plan_id, status: 'applied', transaction: 'committed', affected: 1, verified: true, mismatches: [], sql: plan.sql, ...overrides }; }
function fixture({ input = snapshot(), preview, execute, onChanged, toast } = {}) {
  const doc = installDom(), previews = [], executions = [], plans = []; let notices = 0;
  const panel = new QueryResultPanel(input, { preview: async change => { previews.push(structuredClone(change)); const plan = preview ? await preview(change) : makePlan(); plans.push(plan); return plan; },
    execute: async id => { executions.push(id); return execute ? execute(id, plans.at(-1)) : makeReceipt(plans.at(-1)); }, onChanged: () => { ++notices; onChanged?.(); }, toast });
  doc.body.append(panel.el); return { panel, doc, grid: globalThis.__PL21_GRIDS__.at(-1), previews, executions, plans, notices: () => notices };
}
function byClass(panel, name) { return panel.el.querySelector(`.${name}`); }
function byText(panel, tag, label) { const result = panel.el.querySelectorAll(tag).find(node => node.textContent === label); assert.ok(result, `missing ${tag} ${label}`); return result; }
function consent(panel) { const input = byClass(panel, 'qr-review').querySelector('input[type="checkbox"]'); assert.ok(input); input.checked = true; input.dispatch('change'); return input; }
function stage(panel, token = 'view-a', column = 'c1', value = text('new')) { panel.selectTokens([token]); panel.stageCell(token, column, value); }
async function review(panel) { assert.equal(await panel.previewSelected(), true); consent(panel); }
function deferred() { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }

// Each test constructs the production component, not an extracted safety helper.
test('native labels, loaded scope, read-only display grid and explicit input types render', () => {
  const { panel, grid } = fixture(); assert.equal(grid.data.readOnly, false); assert.equal(grid.data.rows.length, 3);
  assert.match(byClass(panel, 'qr-status').textContent, /500000/); assert.match(byClass(panel, 'qr-note').textContent, /前缀外 499997 行保持只读/);
  assert.equal(byClass(panel, 'qr-status').getAttribute('role'), 'status'); assert.equal(byClass(panel, 'qr-error').getAttribute('role'), 'alert');
  assert.equal(panel.el.getAttribute('aria-label'), '查询结果与受控编辑'); panel.selectTokens(['view-a']);
  assert.ok(panel.el.querySelectorAll('label').some(node => node.textContent.startsWith('类型化值')));
  const editor = byClass(panel, 'qr-editor').querySelector('textarea'); assert.match(editor.getAttribute('aria-describedby'), /editor-help.*error/);
  assert.equal(editor.value, 'alpha'); assert.equal(panel.dirty, false); panel.dispose(); assert.equal(grid.disposed, true);
});

test('sorting grid indices still stages and previews the exact original server row token', async () => {
  const { panel, grid, previews, executions } = fixture(); panel.setLocalSorts([{ column_id: 'c2', ascending: true, nulls: 'last' }]);
  assert.equal(grid.data.rows[0][0], '2'); grid.callbacks.onSelectionChange([0]); grid.callbacks.onCellFocus(0, 1);
  const input = byClass(panel, 'qr-editor').querySelector('textarea'); input.value = 'sorted-original-b'; input.dispatch('input'); byText(panel, 'button', '应用到此行草稿').click();
  assert.equal(await panel.previewSelected(), true); assert.equal(previews[0].row_token, 'server-row-b'); assert.deepEqual(previews[0].changes, [['c1', text('sorted-original-b')]]);
  assert.deepEqual(Object.keys(previews[0]).sort(), ['changes', 'operation', 'result_token', 'row_token']); assert.equal(executions.length, 0);
  assert.equal(byClass(panel, 'qr-binds').textContent, JSON.stringify(makePlan().binds, null, 2)); panel.dispose();
});

test('editing while preview is pending invalidates its response and never executes it', async () => {
  const gate = deferred(), { panel, executions, previews } = fixture({ preview: () => gate.promise }); stage(panel); const pending = panel.previewSelected();
  assert.equal(panel.busy, true); assert.equal(previews.length, 1); panel.stageCell('view-a', 'c1', text('newer')); gate.resolve(makePlan());
  assert.equal(await pending, false); assert.equal(await panel.confirmPreview(), false); assert.equal(executions.length, 0);
  assert.deepEqual(panel.captureDraft().changes[0].changes, [['c1', text('newer')]]); assert.equal(panel.busy, false); panel.dispose();
});

test('editing, switching row, filtering or hiding a column after preview revokes its plan', async t => {
  for (const mutate of [panel => panel.stageCell('view-a', 'c1', text('newer')), panel => panel.selectTokens(['view-b']), panel => panel.setLocalFilters([{ logic: 'AND', conditions: [{ column_id: 'c0', operator: 'gt', value: integer('0') }] }]), panel => panel.setHiddenColumns(['c3'])]) {
    await t.test(String(mutate), async () => { const { panel, executions } = fixture(); stage(panel); await review(panel); mutate(panel); assert.equal(await panel.confirmPreview(), false); assert.equal(executions.length, 0); assert.equal(panel.dirty, true); panel.dispose(); });
  }
});

test('an un-applied typed editor blocks both preview and selection without losing raw input', async () => {
  const { panel, previews } = fixture(); panel.selectTokens(['view-a']); const input = byClass(panel, 'qr-editor').querySelector('textarea'); input.value = 'unsaved raw'; input.dispatch('input');
  assert.equal(await panel.previewSelected(), false); assert.equal(previews.length, 0); assert.throws(() => panel.selectTokens(['view-b']), /尚未应用/);
  assert.equal(panel.captureDraft().editor.raw, 'unsaved raw'); assert.equal(panel.dirty, true); panel.dispose();
});

test('confirmation requires explicit consent and a fresh preview', async () => {
  const { panel, executions } = fixture(); stage(panel); assert.equal(await panel.previewSelected(), true); assert.equal(await panel.confirmPreview(), false); assert.equal(executions.length, 0);
  assert.equal(await panel.previewSelected(), true); consent(panel); assert.equal(await panel.confirmPreview(), true); assert.equal(executions.length, 1); assert.equal(panel.captureDraft().write_evidence.state, 'confirmed'); panel.dispose();
});

test('malformed, foreign or lossy preview receipts never reach execute', async t => {
  const cases = [
    plan => { delete plan.plan_id; }, plan => { plan.plan_id = 'not-opaque-uuid'; }, plan => { plan.base_revision = 'b'.repeat(64); }, plan => { plan.expected_rows = 0; }, plan => { plan.expected_rows = 2; },
    plan => { plan.sql = ''; }, plan => { plan.binds = [integer('9223372036854775808x')]; }, plan => { plan.binds = [{ kind: 'integer', value: 9007199254740992 }]; }, plan => { plan.extra = true; }, plan => { delete plan.summary; },
  ];
  for (const [index, mutate] of cases.entries()) await t.test(`preview-${index}`, async () => { const { panel, executions } = fixture({ preview: () => { const plan = makePlan(); mutate(plan); return plan; } }); stage(panel); assert.equal(await panel.previewSelected(), false); assert.equal(await panel.confirmPreview(), false); assert.equal(executions.length, 0); assert.equal(panel.dirty, true); panel.dispose(); });
});

test('foreign or contradictory write receipts become locked Unknown and preserve the exact draft', async t => {
  const cases = [receipt => { receipt.plan_id = randomUUID(); }, receipt => { receipt.sql += '; SELECT 1'; }, receipt => { receipt.affected = 0; }, receipt => { receipt.affected = 2; }, receipt => { receipt.affected = null; },
    receipt => { receipt.verified = false; }, receipt => { receipt.transaction = 'unknown'; }, receipt => { receipt.status = 'unknown'; }, receipt => { receipt.mismatches = ['trigger changed value']; }, receipt => { receipt.extra = true; }, receipt => { delete receipt.transaction; }];
  for (const [index, mutate] of cases.entries()) await t.test(`receipt-${index}`, async () => { const { panel, executions } = fixture({ execute: (_id, plan) => { const receipt = makeReceipt(plan); mutate(receipt); return receipt; } }); stage(panel); await review(panel);
    assert.equal(await panel.confirmPreview(), false); assert.equal(executions.length, 1); const draft = panel.captureDraft(); assert.equal(draft.write_evidence.state, 'unknown'); assert.equal(draft.replay_allowed, false); assert.deepEqual(draft.changes[0].changes, [['c1', text('new')]]);
    assert.equal(panel.busy, true); assert.throws(() => panel.stageCell('view-a', 'c1', text('retry')), /未确认|未知/); assert.throws(() => panel.discardSelected(), /未确认|未知/);
    assert.equal(await panel.previewSelected(), false); assert.equal(await panel.confirmPreview(), false); assert.equal(executions.length, 1); panel.dispose(); });
});

test('lost ACK and rapid duplicate confirms execute once and retain pending evidence', async () => {
  const gate = deferred(), { panel, executions } = fixture({ execute: () => gate.promise }); stage(panel); await review(panel); const first = panel.confirmPreview();
  assert.equal(await panel.confirmPreview(), false); assert.equal(executions.length, 1); assert.equal(panel.captureDraft().write_evidence.state, 'in_flight'); assert.throws(() => panel.discardSelected(), /已发送/);
  gate.reject(new Error('network ACK lost')); assert.equal(await first, false); panel.invalidate('用户尝试刷新');
  assert.equal(panel.captureDraft().write_evidence.state, 'unknown'); assert.equal(panel.busy, true); assert.equal(await panel.previewSelected(), false); assert.equal(executions.length, 1); assert.equal(panel.dirty, true); panel.dispose();
});

test('known rollback retains drafts but releases pending status and requires explicit refresh', async () => {
  const { panel, executions } = fixture({ execute: (_id, plan) => makeReceipt(plan, { status: 'conflict', transaction: 'rolled_back', affected: 0, verified: false, mismatches: ['old-value conflict'] }) }); stage(panel); await review(panel);
  assert.equal(await panel.confirmPreview(), false); assert.equal(panel.busy, false); assert.equal(panel.dirty, true); assert.equal(panel.captureDraft().write_evidence.state, 'rolled_back'); assert.equal(await panel.previewSelected(), false); assert.equal(executions.length, 1); panel.dispose();
});

test('verified commit clears only the committed row, preserving other drafts without guessing membership', async () => {
  const { panel, grid, executions } = fixture(); stage(panel); panel.stageCell('view-b', 'c1', text('other pending')); await review(panel);
  assert.equal(await panel.confirmPreview(), true); assert.equal(executions.length, 1); assert.equal(panel.busy, false); assert.equal(panel.dirty, true);
  const draft = panel.captureDraft(); assert.equal(draft.changes.length, 1); assert.equal(draft.changes[0].row_token, 'server-row-b'); assert.match(draft.invalid_reason, /显式重新执行/);
  assert.equal(grid.data.rows[0][1], 'alpha'); assert.equal(grid.data.rows[1][1], 'other pending'); assert.throws(() => panel.stageCell('view-b', 'c1', text('replay')), /重新执行/); panel.dispose();
});

test('a backend-verified no-op accepts zero affected only for update with a known transaction', async () => {
  const { panel } = fixture({ execute: (_id, plan) => makeReceipt(plan, { status: 'unchanged', transaction: 'rolled_back', affected: 0 }) }); stage(panel); await review(panel); assert.equal(await panel.confirmPreview(), true); assert.equal(panel.dirty, false); assert.equal(panel.captureDraft().write_evidence.state, 'confirmed'); panel.dispose();
});

test('delete uses the original row token and no client keys, SQL or display reconstruction', async () => {
  const { panel, previews, executions } = fixture(); panel.selectTokens(['view-c']); panel.markSelectedDeleted(); assert.equal(await panel.previewSelected(), true);
  assert.deepEqual(previews[0], { result_token: 'opaque-result', row_token: 'server-row-c', operation: 'delete', changes: [] }); assert.equal(executions.length, 0); panel.dispose();
});

test('hidden dirty rows and columns survive filtering and are excluded from visible copy', () => {
  const { panel, grid } = fixture(); stage(panel, 'view-a', 'c1', text('HIDDEN-SECRET')); panel.setHiddenColumns(['c1', 'c4']);
  assert.match(byClass(panel, 'qr-note').textContent, /隐藏列内有未提交值/); panel.setLocalFilters([{ logic: 'AND', conditions: [{ column_id: 'c0', operator: 'gt', value: integer('1') }] }]);
  panel.selectTokens(['view-b', 'view-c']); const copied = panel.copyText('rows_with_headers'); assert.doesNotMatch(copied, /HIDDEN-SECRET|label|computed/); assert.match(copied, /9007199254740992\.999999999999999999/);
  assert.match(byClass(panel, 'qr-status').textContent, /1 行被筛选隐藏/); assert.deepEqual(panel.captureDraft().changes[0].changes, [['c1', text('HIDDEN-SECRET')]]); assert.equal(grid.data.rows.length, 2); panel.dispose();
});

test('AND/OR groups, NULL predicates, multi-sort priorities and stable exact values stay local', () => {
  const { panel, grid, previews, executions } = fixture(); panel.setLocalFilters([{ logic: 'OR', conditions: [{ column_id: 'c1', operator: 'contains', value: text('ALPHA'), case_sensitive: false }, { column_id: 'c2', operator: 'is_null' }] }, { logic: 'AND', conditions: [{ column_id: 'c0', operator: 'gte', value: integer('1') }] }]);
  panel.setLocalSorts([{ column_id: 'c2', ascending: false, nulls: 'first' }, { column_id: 'c0', ascending: false, nulls: 'last' }]); assert.deepEqual(grid.data.rows.map(row => row[0]), ['3', '1']); assert.equal(previews.length, 0); assert.equal(executions.length, 0); panel.dispose();
});

test('new rows preserve omitted DEFAULT, explicit NULL and empty text as different requests', async () => {
  const { panel, previews } = fixture(); panel.beginInsert(); assert.deepEqual(panel.captureDraft().new_row, []); assert.equal(await panel.previewSelected(), true); assert.deepEqual(previews[0].changes, []); panel.cancelPreview();
  panel.setInsertValue('c1', nil()); panel.setInsertValue('c2', text('')); panel.setInsertValue('c3', binary('AAD/')); assert.equal(await panel.previewSelected(), true);
  assert.deepEqual(previews[1], { result_token: 'opaque-result', row_token: null, operation: 'insert', changes: [['c1', nil()], ['c2', text('')], ['c3', binary('AAD/')]] }); panel.cancelPreview(); panel.setInsertValue('c1', undefined); assert.equal(panel.captureDraft().new_row.some(([column]) => column === 'c1'), false);
  assert.throws(() => panel.setInsertValue('c0', integer('7')), /定位键|权限/); assert.throws(() => panel.setInsertValue('c4', text('fake generated')), /生成/); panel.dispose();
});

test('explicit insert metadata permits non-auto keys, rejects generated/auto/NULL and never guesses from names', async () => {
  const input = snapshot(); Object.assign(input.editability.columns[0], { insertable: true, auto_increment: false, nullable: false }); Object.assign(input.editability.columns[1], { nullable: false }); Object.assign(input.editability.columns[2], { auto_increment: true, insertable: true });
  const { panel, previews } = fixture({ input }); panel.beginInsert(); panel.setInsertValue('c0', integer('9223372036854775807')); assert.throws(() => panel.setInsertValue('c0', nil()), /NULL/); assert.throws(() => panel.setInsertValue('c1', nil()), /NULL/); assert.throws(() => panel.setInsertValue('c2', decimal('1')), /自增/);
  assert.equal(await panel.previewSelected(), true); assert.deepEqual(previews[0].changes, [['c0', integer('9223372036854775807')]]); panel.dispose();
});

test('full server insert attributes reach the real panel without leaking ROWID, generated values or omitted defaults', async () => {
  const input = snapshot();
  for (const column of input.editability.columns) Object.assign(column, { insertable: !column.generated, nullable: true, auto_increment: false, default_value: null });
  Object.assign(input.editability.columns[0], { insertable: false, nullable: false, auto_increment: true });
  Object.assign(input.editability.columns[1], { insertable: true, nullable: false, default_value: "'server default'" });
  Object.assign(input.editability.columns[3], { default_value: "X'00FF'" });
  const { panel, previews, executions } = fixture({ input }); panel.beginInsert();
  assert.throws(() => panel.setInsertValue('c0', integer('9')), /自增/); assert.throws(() => panel.setInsertValue('c4', text('manual generated')), /生成/); assert.throws(() => panel.setInsertValue('c1', nil()), /NULL/);
  panel.setInsertValue('c2', nil()); panel.setInsertValue('c3', binary(''));
  assert.equal(await panel.previewSelected(), true); assert.deepEqual(previews[0], { result_token: 'opaque-result', row_token: null, operation: 'insert', changes: [['c2', nil()], ['c3', binary('')]] });
  assert.equal(previews[0].changes.some(([id]) => ['c0', 'c1', 'c4'].includes(id)), false); assert.equal(executions.length, 0); panel.dispose();
});

test('explicit non-auto composite key insert flags survive validation while existing key rows stay immutable', async () => {
  const input = snapshot();
  for (const column of input.editability.columns) Object.assign(column, { insertable: !column.generated, nullable: true, auto_increment: false, default_value: null });
  for (const column of input.editability.columns.slice(0, 2)) Object.assign(column, { key: true, writable: false, insertable: true, nullable: false, reason: '定位键只读' });
  input.editability.key_column_ids = ['c0', 'c1'];
  const { panel, previews } = fixture({ input }); panel.selectTokens(['view-a']); assert.throws(() => panel.stageCell('view-a', 'c0', integer('9')), /定位键/); assert.throws(() => panel.stageCell('view-a', 'c1', text('changed key')), /定位键/);
  panel.beginInsert(); panel.setInsertValue('c0', integer('9223372036854775807')); panel.setInsertValue('c1', text('manual key')); assert.throws(() => panel.setInsertValue('c0', nil()), /NULL/); assert.throws(() => panel.setInsertValue('c1', nil()), /NULL/);
  assert.equal(await panel.previewSelected(), true); assert.deepEqual(previews[0], { result_token: 'opaque-result', row_token: null, operation: 'insert', changes: [['c0', integer('9223372036854775807')], ['c1', text('manual key')]] }); panel.dispose();
});

test('typed field controls report invalid input, keep raw content and offer a keyboard cancel', () => {
  const { panel, doc } = fixture(); panel.selectTokens(['view-a']); const host = byClass(panel, 'qr-editor'), selects = host.querySelectorAll('select'), kind = selects[1], input = host.querySelector('textarea');
  kind.value = 'integer'; kind.dispatch('change'); input.value = '9007199254740993oops'; input.dispatch('input'); byText(panel, 'button', '应用到此行草稿').click();
  assert.equal(input.getAttribute('aria-invalid'), 'true'); assert.equal(doc.activeElement, input); assert.equal(panel.captureDraft().editor.raw, '9007199254740993oops'); assert.equal(panel.dirty, true); assert.equal(byClass(panel, 'qr-error').hidden, false);
  byText(panel, 'button', '取消字段输入').click(); assert.equal(panel.dirty, false); panel.dispose();
});

test('Escape closes review without executing and restores the preview button focus', async () => {
  const { panel, doc, executions } = fixture(); stage(panel); await review(panel); const event = byClass(panel, 'qr-review').dispatch('keydown', { key: 'Escape' }); assert.equal(event.defaultPrevented, true);
  assert.equal(executions.length, 0); assert.equal(byClass(panel, 'qr-review').hidden, true); assert.equal(doc.activeElement, panel.el.querySelector('button[aria-label="提交改动（先预览后端写入计划）"]')); assert.equal(panel.dirty, true); panel.dispose();
});

test('closing during a pending preview ignores late evidence without dropping the local draft', async () => {
  const gate = deferred(), { panel, grid, notices, executions } = fixture({ preview: () => gate.promise }); stage(panel); const pending = panel.previewSelected(); panel.dispose(); const before = notices(), updates = grid.updates; gate.resolve(makePlan());
  assert.equal(await pending, false); assert.equal(notices(), before); assert.equal(grid.updates, updates); assert.equal(executions.length, 0); assert.equal(panel.captureDraft().changes.length, 1); assert.equal(await panel.confirmPreview(), false);
});

test('closing after send preserves Unknown even when a successful receipt arrives later', async () => {
  const gate = deferred(), { panel, grid, notices, plans, executions } = fixture({ execute: () => gate.promise }); stage(panel); await review(panel); const pending = panel.confirmPreview(); panel.dispose(); const before = notices(), updates = grid.updates;
  gate.resolve(makeReceipt(plans[0])); assert.equal(await pending, false); assert.equal(notices(), before); assert.equal(grid.updates, updates); assert.equal(executions.length, 1); assert.equal(panel.captureDraft().write_evidence.state, 'unknown'); assert.equal(panel.captureDraft().changes.length, 1); assert.equal(panel.captureDraft().replay_allowed, false);
});

test('host notification failures cannot turn acknowledged commit into unconfirmed transaction evidence', async () => {
  const { panel, executions } = fixture({ onChanged: () => { throw new Error('host notification'); }, toast: () => { throw new Error('host toast'); } }); stage(panel); await review(panel); assert.equal(await panel.confirmPreview(), true); assert.equal(executions.length, 1); assert.equal(panel.captureDraft().write_evidence.state, 'confirmed'); panel.dispose();
});

test('readonly source, generated columns and missing row tokens never reach a write port', async () => {
  const input = snapshot(); input.editability = { ...ctx, result_token: null, mode: 'read_only', reason: 'JOIN 查询缺少单表来源证明', schema: null, table: null, metadata_revision: null, columns: [], key_column_ids: [], actions: Object.fromEntries(['update', 'delete', 'insert'].map(action => [action, { supported: false, reason: 'JOIN 只读' }])) }; input.rows.forEach(row => { delete row.row_token; });
  const { panel, previews, executions } = fixture({ input }); panel.selectTokens(['view-a']); assert.throws(() => panel.stageCell('view-a', 'c1', text('bad')), error => error.message === input.editability.reason); assert.throws(() => panel.beginInsert(), error => error.message === input.editability.reason); assert.equal(await panel.previewSelected(), false); assert.equal(previews.length, 0); assert.equal(executions.length, 0); assert.match(byClass(panel, 'qr-note').textContent, /JOIN/); panel.dispose();
});

test('expired and already consumed opaque plans cannot be sent, even if a later preview reuses their id', async () => {
  const plan = makePlan(), { panel, executions } = fixture({ preview: () => structuredClone(plan), execute: (_id, item) => makeReceipt(item, { status: 'conflict', transaction: 'rolled_back', affected: 0, verified: false, mismatches: ['rejected after once'] }) });
  stage(panel); await review(panel); const now = Date.now; try { const started = now(); Date.now = () => started + 110_001; assert.equal(await panel.confirmPreview(), false); } finally { Date.now = now; }
  assert.equal(executions.length, 0); await review(panel); assert.equal(await panel.confirmPreview(), false); assert.equal(executions.length, 1); assert.equal(await panel.previewSelected(), false); assert.equal(executions.length, 1); panel.dispose();
});

test('a foreign generation or column mapping is rejected at component construction', () => {
  for (const mutate of [input => { input.editability.generation = 'old-generation'; }, input => { input.editability.columns[1].result_column_id = 'client-renamed'; }, input => { input.rows[1].row_token = input.rows[0].row_token; }]) { const input = snapshot(); mutate(input); assert.throws(() => fixture({ input })); }
});

test('generated and key update targets plus rows outside the typed token prefix are read only', async () => {
  const input = snapshot(); delete input.rows[2].row_token; const { panel, previews, executions } = fixture({ input }); panel.selectTokens(['view-a']);
  assert.throws(() => panel.stageCell('view-a', 'c0', integer('2')), /定位键/); assert.throws(() => panel.stageCell('view-a', 'c4', text('forged')), /生成/);
  panel.selectTokens(['view-c']); assert.throws(() => panel.stageCell('view-c', 'c1', text('outside')), /令牌/); assert.throws(() => panel.markSelectedDeleted(), /令牌/); assert.equal(await panel.previewSelected(), false); assert.equal(previews.length, 0); assert.equal(executions.length, 0); panel.dispose();
});

test('raw unapplied field content cannot be lost by hiding its column or changing the field selector', () => {
  const { panel, grid } = fixture(); panel.selectTokens(['view-a']); const host = byClass(panel, 'qr-editor'), input = host.querySelector('textarea'), selector = host.querySelector('select');
  input.value = 'raw-unapplied-sensitive-value'; input.dispatch('input'); assert.throws(() => panel.setHiddenColumns(['c1']), /尚未应用/);
  const visibility = byClass(panel, 'qr-columns').querySelectorAll('input')[1]; visibility.checked = false; visibility.dispatch('change'); assert.equal(visibility.checked, true);
  selector.value = 'c2'; selector.dispatch('change'); assert.equal(selector.value, 'c1'); assert.equal(panel.captureDraft().editor.column, 'c1'); assert.equal(panel.captureDraft().editor.raw, 'raw-unapplied-sensitive-value'); assert.equal(panel.dirty, true); assert.equal(grid.data.columns.some(column => column.name === 'label'), true);
  byText(panel, 'button', '应用到此行草稿').click(); panel.setHiddenColumns(['c1']); assert.deepEqual(panel.captureDraft().changes[0].changes, [['c1', text('raw-unapplied-sensitive-value')]]); assert.match(byClass(panel, 'qr-note').textContent, /隐藏列内有未提交值/); panel.dispose();
});

test('at least one typed column stays visible and copy cannot expose a hidden focused column', () => {
  const { panel, grid } = fixture(); panel.selectTokens(['view-a']); grid.callbacks.onCellFocus(0, 1);
  assert.throws(() => panel.setHiddenColumns(['c0', 'c1', 'c2', 'c3', 'c4']), /至少保留/);
  panel.setHiddenColumns(['c1', 'c2', 'c3', 'c4']); assert.equal(panel.copyText('cell'), '1');
  assert.equal(panel.copyText('rows'), '1'); panel.dispose();
});

test('typed editor parser preserves large numbers, NULL, bytes and date precision without display reconstruction', () => {
  assert.deepEqual(parseQueryPanelValue('integer', '9223372036854775807'), integer('9223372036854775807')); assert.deepEqual(parseQueryPanelValue('decimal', '9007199254740993.000000000000000001'), decimal('9007199254740993.000000000000000001'));
  assert.deepEqual(parseQueryPanelValue('null', 'ignored'), nil()); assert.equal(parseQueryPanelValue('default', 'ignored'), undefined); assert.deepEqual(parseQueryPanelValue('text', ''), text('')); assert.deepEqual(parseQueryPanelValue('binary', 'AAD/'), binary('AAD/'));
  assert.deepEqual(parseQueryPanelValue('float', '1.25e-3'), { kind: 'float', value: 0.00125 }); assert.deepEqual(parseQueryPanelValue('bool', 'false'), { kind: 'bool', value: false });
  assert.deepEqual(parseQueryPanelValue('timestamptz', '2024-02-29T15:30:00.123456789+05:30'), { kind: 'timestamptz', value: '2024-02-29T15:30:00.123456789+05:30' });
  for (const [kind, value] of [['integer', '9007199254740993x'], ['decimal', '1.2.3'], ['binary', 'not base64'], ['date', '2023-02-29'], ['float', ''], ['float', 'NaN'], ['bool', '1'], ['unknown', 'a']]) assert.throws(() => parseQueryPanelValue(kind, value));
});

test('column sidebar separates repeated aliases by id and editable index still uses the original server token', async () => {
  const input = snapshot(); input.columns[0].name = 'same_alias'; input.columns[1].name = 'same_alias';
  input.editability.columns[0].name = 'same_alias'; input.editability.columns[1].name = 'same_alias';
  const { panel, grid, previews, executions, doc } = fixture({ input });
  const trigger = panel.el.querySelectorAll('button').find(node => node.getAttribute('aria-label') === '选择显示字段');
  assert.ok(trigger); assert.equal(trigger.getAttribute('aria-expanded'), 'false'); trigger.click();
  const sidebar = byClass(panel, 'column-sidebar'); assert.equal(sidebar.hidden, false); assert.equal(trigger.getAttribute('aria-controls'), sidebar.id);
  const boxes = byClass(panel, 'qr-columns').querySelectorAll('input'); boxes[0].checked = false; boxes[0].dispatch('change');
  assert.deepEqual(panel.captureDraft().hidden_columns, ['c0']); assert.equal(boxes[1].checked, true); assert.equal(grid.data.columns[0].name, 'same_alias');
  grid.callbacks.onSelectionChange([1]); grid.callbacks.onCellFocus(1, 0);
  const field = byClass(panel, 'qr-editor').querySelector('textarea'); field.value = 'edited alias'; field.dispatch('input'); byText(panel, 'button', '应用到此行草稿').click();
  assert.equal(await panel.previewSelected(), true); assert.deepEqual(previews[0].changes, [['c1', text('edited alias')]]); assert.equal(previews[0].row_token, 'server-row-b'); assert.equal(executions.length, 0);
  sidebar.dispatch('keydown', { key: 'Escape' }); assert.equal(sidebar.hidden, true); assert.equal(doc.activeElement, trigger); assert.equal(trigger.getAttribute('aria-expanded'), 'false'); panel.dispose();
});

test('empty query results keep a large searchable column sidebar and one initial visible column', () => {
  const columns = Array.from({ length: 220 }, (_, i) => ({ id: `empty-${i}`, name: `column_${i}`, data_type: 'TEXT', hidden: true }));
  const { panel, grid, previews, executions } = fixture({ input: { ...ctx, columns, rows: [], total_rows: 0 } });
  assert.equal(grid.data.rows.length, 0); assert.equal(grid.data.readOnly, true); assert.equal(grid.data.columns.length, 1);
  const trigger = panel.el.querySelectorAll('button').find(node => node.getAttribute('aria-label') === '选择显示字段'); assert.equal(trigger.disabled, false); trigger.click();
  assert.equal(byClass(panel, 'qr-columns').querySelectorAll('input').length, 220);
  const sidebar = byClass(panel, 'column-sidebar'), search = sidebar.querySelector('input[type="search"]'); search.value = 'column_21'; search.dispatch('input'); assert.equal(byClass(panel, 'qr-columns').querySelectorAll('input').length, 11);
  byText(panel, 'button', '全选 / 恢复全部字段').click(); assert.equal(panel.captureDraft().hidden_columns.length, 0); assert.equal(grid.data.columns.length, 220);
  assert.equal(previews.length, 0); assert.equal(executions.length, 0); panel.dispose();
});
