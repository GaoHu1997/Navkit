// Production dialog + model + cache + API, deterministic VM/DOM. Only the independent
// table designer and accessibility helper are stubbed; preview IPC is explicit mock data.
// This suite is not native/database acceptance and makes no database throughput claim.
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';

const root = fileURLToPath(new URL('../../', import.meta.url));
const graphPlugins = (realEditor = false) => [{ name: 'isolate-independent-editor', setup(b) {
  if (!realEditor) {
  b.onResolve({ filter: /(?:^|\/)ErTableEditor$/ }, () => ({ path: 'editor', namespace: 'fixture' }));
  b.onLoad({ filter: /^editor$/, namespace: 'fixture' }, () => ({ loader: 'js', contents: `
    export class ErTableEditor {
      constructor(opts, host) { this.opts = opts; this.host = host; this.el = document.createElement('div'); window.editors.push(this); }
      activate() { this.activated = true; this.mountedAtActivation = this.el.isConnected; }
      reload() { this.reloads = (this.reloads ?? 0) + 1; }
      saveReason() { return null; } destroy() { this.disposed = true; } draftDirty() { return false; }
      async save() { await window.saveEditor(this); }
    }` }));
  }
  b.onResolve({ filter: /^\.\/SyncDialog$/ }, () => ({ path: 'a11y', namespace: 'fixture' }));
  b.onLoad({ filter: /^a11y$/, namespace: 'fixture' }, () => ({ loader: 'js', contents: `export function accessibleDialog(dlg) { dlg.el.setAttribute('role', 'dialog'); }` }));
} }];
const plugins = graphPlugins();
const result = await build({ stdin: { contents: `export { openErDiagramDialog } from './src/dialogs/ErDiagramDialog'; export * from './src/dialogs/erDiagramRendering';`, resolveDir: root, loader: 'ts' },
  bundle: true, write: false, format: 'cjs', platform: 'node', loader: { '.css': 'empty' }, plugins, logLevel: 'silent', metafile: true });
assert.ok(Object.keys(result.metafile.inputs).some(p => p.endsWith('src/core/api.ts')));
assert.ok(Object.keys(result.metafile.inputs).some(p => p.endsWith('src/core/erDiagramMetadata.ts')));
// 元数据状态回归使用真实右侧编辑器、完整性断言、API 与受控执行器。
const realResult = await build({ stdin: { contents: `export { openErDiagramDialog } from './src/dialogs/ErDiagramDialog'; export { ErTableEditor } from './src/ui/ErTableEditor';`, resolveDir: root, loader: 'ts' },
  bundle: true, write: false, format: 'cjs', platform: 'node', loader: { '.css': 'empty' }, plugins: graphPlugins(true), logLevel: 'silent', metafile: true });
assert.ok(Object.keys(realResult.metafile.inputs).some(p => p.endsWith('src/ui/ErTableEditor.ts')));
assert.ok(Object.keys(realResult.metafile.inputs).some(p => p.endsWith('src/core/table-design-draft.ts')));

class Element {
  constructor(tag = 'div') {
    this.tagName = tag.toUpperCase(); this.children = []; this.parentNode = null;
    this.style = {}; this.dataset = {}; this.attributes = new Map(); this.listeners = new Map();
    this.className = ''; this._value = ''; this._text = ''; this._html = ''; this.hidden = false;
    this.clientWidth = 1000; this.clientHeight = 700; this.writes = [];
    this.classList = { contains: n => this.className.split(/\s+/).includes(n),
      add: (...ns) => { this.className = [...new Set([...this.className.split(/\s+/).filter(Boolean), ...ns])].join(' '); },
      remove: (...ns) => { this.className = this.className.split(/\s+/).filter(n => !ns.includes(n)).join(' '); },
      toggle: (n, on) => { on ??= !this.classList.contains(n); on ? this.classList.add(n) : this.classList.remove(n); return on; } };
  }
  get value() { return this.tagName === 'SELECT' ? this.children.find(o => o.selected)?.value ?? this.children[0]?.value ?? '' : this._value; }
  set value(v) { this._value = String(v); if (this.tagName === 'SELECT') for (const o of this.children) o.selected = o.value === this._value; }
  set textContent(v) { this._text = String(v); this._html = ''; this.replaceChildren(); }
  get textContent() { return this._text + this._html.replace(/<[^>]*>/g, '') + this.children.map(c => c.textContent).join(''); }
  set innerHTML(v) { this._html = String(v); this._text = ''; this.replaceChildren(); }
  get innerHTML() { return this._html; }
  appendChild(c) { c.remove(); this.children.push(c); c.parentNode = this; return c; }
  append(...cs) { cs.forEach(c => this.appendChild(c)); }
  replaceChildren(...cs) { for (const c of this.children) c.parentNode = null; this.children = []; this.append(...cs); }
  insertBefore(c, before) { c.remove(); const i = this.children.indexOf(before); this.children.splice(i < 0 ? this.children.length : i, 0, c); c.parentNode = this; }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(c => c !== this); this.parentNode = null; }
  get parentElement() { return this.parentNode; }
  get firstChild() { return this.children[0] ?? null; }
  get firstElementChild() { return this.firstChild; }
  get nextSibling() { if (!this.parentNode) return null; return this.parentNode.children[this.parentNode.children.indexOf(this) + 1] ?? null; }
  get options() { return this.children; }
  get isConnected() { return this === this.ownerDocument.body || Boolean(this.parentNode?.isConnected); }
  setAttribute(n, v) { this.attributes.set(n, String(v)); this.writes.push(n); }
  getAttribute(n) { return this.attributes.get(n) ?? null; }
  removeAttribute(n) { this.attributes.delete(n); }
  addEventListener(n, f) { const fs = this.listeners.get(n) ?? []; fs.push(f); this.listeners.set(n, fs); }
  removeEventListener(n, f) { this.listeners.set(n, (this.listeners.get(n) ?? []).filter(x => x !== f)); }
  emit(n, props = {}) { const e = { target: this, preventDefault() {}, stopPropagation() {}, ...props }; for (const f of this.listeners.get(n) ?? []) f(e); }
  matches(selector) { return selector.split(',').some(s => {
    const tag = s.match(/^[\w-]+/); if (tag && this.tagName !== tag[0].toUpperCase()) return false;
    if ([...s.matchAll(/\.([\w-]+)/g)].some(m => !this.classList.contains(m[1]))) return false;
    if ([...s.matchAll(/\[([\w-]+)(?:=['"]?([^'"\]]+)['"]?)?\]/g)].some(m => this.getAttribute(m[1]) === null || m[2] != null && this.getAttribute(m[1]) !== m[2])) return false;
    return true;
  }); }
  closest(s) { for (let p = this; p; p = p.parentNode) if (p.matches(s)) return p; return null; }
  querySelectorAll(s) { const r = []; const visit = e => { for (const c of e.children) { if (c.matches(s)) r.push(c); visit(c); } }; visit(this); return r; }
  querySelector(s) { return this.querySelectorAll(s)[0] ?? null; }
  focus() { this.ownerDocument.activeElement = this; }
  getBoundingClientRect() { return { left: 0, top: 0, width: 1000, height: 700 }; }
}
const connection = id => ({ id, name: id, kind: 'mysql', host: id, port: 3306, user: 'u', password: '', read_only: false });
const column = (name = 'id', primary_key = false) => ({ name, type: 'int', nullable: false, primary_key, auto_increment: false, comment: null, length: null });
const table = (name, namespace = 'db', columns = [column('id', true)]) => ({ table: name, database: namespace, columns, indexes: [], foreign_keys: [], comment: null, row_count: null });
const metadata = tables => ({ tables, failures: [], limitations: [] });
const designSnapshot = ({ schema = 'db', table: name = 'items', ownerId = 'fixture-owner' } = {}) => {
  const feature = { supported: true, reason: null };
  return { schema: table(name, schema), objects: { completeness: 'complete', checks: [], triggers: [], limitations: [] }, options: {},
    design: { schema, table: name, columns: [{ name: 'id', orig_name: 'id', data_type: 'int', length: null, scale: null, nullable: false,
      default_value: null, auto_increment: false, unsigned: false, zerofill: false, comment: '' }], primary_key: ['id'], indexes: [], foreign_keys: [], checks: [], triggers: [], options: {}, comment: '' },
    revision: 'fixture-r1', source_fingerprint: 'fixture-f1', owner_id: ownerId, completeness: 'complete',
    capabilities: { checks: feature, triggers: feature, options: feature, rebuild: feature, clear: feature, copy_structure: feature, copy_data: feature, supported_options: [], limitations: [] } };
};
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
function harness(options = {}) {
  const document = { createElement(tag) { const e = new Element(tag); e.ownerDocument = document; return e; }, createElementNS(ns, tag) { return this.createElement(tag); } };
  document.body = document.createElement('body');
  const frames = new Map(), timers = new Map(), listeners = new Map(), calls = [], observers = [], storage = new Map();
  let seq = 0;
  const window = { innerWidth: 1280, innerHeight: 900, editors: [], saveEditor: async () => {},
    addEventListener(n, f) { const fs = listeners.get(n) ?? []; fs.push(f); listeners.set(n, fs); },
    removeEventListener(n, f) { listeners.set(n, (listeners.get(n) ?? []).filter(x => x !== f)); },
    requestAnimationFrame(f) { const id = ++seq; frames.set(id, f); return id; }, cancelAnimationFrame(id) { frames.delete(id); },
    __PREVIEW_MOCK__: {
      list_databases: a => { calls.push(['list', a]); return options.list?.(a) ?? [['db', 'mysql'], ['other', 'mysql']]; },
      get_diagram_metadata: a => { calls.push(['metadata', a]); return options.load?.(a) ?? metadata([table('items', a.schemaName)]); },
      execute_ddl: a => { calls.push(['ddl', a]); return options.ddl?.(a) ?? 0; },
      read_table_design_snapshot: a => { calls.push(['design', a]); return options.design?.(a) ?? designSnapshot(a); },
      list_tables: a => { calls.push(['reference-tables', a]); return options.refTables?.(a) ?? [['parents', 'table']]; },
      get_table_schema: a => { calls.push(['reference-columns', a]); return options.refColumns?.(a) ?? { columns: [column('parent_id')] }; },
      preview_table_design: a => { calls.push(['preview', a]); return options.preview?.(a) ?? { request_sql: 'fixture-bound-payload', revision: 'fixture-r1', steps: [{ label: '修改字段', sql: 'ALTER TABLE items ...' }], destructive: false, limitations: [] }; },
      issue_execution_grant: a => { calls.push(['grant', a]); return options.grant?.(a) ?? { ...a.request, execution_grant: 'fixture-only' }; },
      execute_table_design: a => { calls.push(['execute', a]); return options.execute?.(a); },
    } };
  const localStorage = { getItem: k => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v), removeItem: k => storage.delete(k) };
  window.localStorage = localStorage;
  const module = { exports: {} };
  const context = vm.createContext({ module, exports: module.exports, window, document, localStorage, HTMLElement: Element,
    ResizeObserver: class { constructor(cb) { this.cb = cb; observers.push(this); } observe(el) { this.el = el; this.cb([{ contentRect: { width: 1000, height: 700 } }]); } disconnect() { this.disconnected = true; } },
    navigator: { clipboard: { writeText: async () => {} } }, crypto: { randomUUID }, structuredClone,
    Option: function (text, value) { const option = document.createElement('option'); option.textContent = text; option.value = value; return option; },
    console, URL, Error, setTimeout: f => { const id = ++seq; timers.set(id, f); return id; }, clearTimeout: id => timers.delete(id) });
  vm.runInContext((options.realEditor ? realResult : result).outputFiles[0].text, context);
  const h = { module: module.exports, document, window, calls, frames, timers, listeners, observers, storage,
    emit(n, e) { for (const f of listeners.get(n) ?? []) f(e); },
    async settle() { for (let i = 0; i < 30; i++) await Promise.resolve(); },
    frame() { const fs = [...frames.values()]; frames.clear(); fs.forEach(f => f()); },
    async flush() { await this.settle(); this.frame(); await this.settle(); },
    open(id = 'a', db = 'db') { const connections = options.connections ?? [connection('a'), connection('b')]; return this.module.openErDiagramDialog(connections, connections.find(c => c.id === id), db); },
    button(label) { return document.body.querySelectorAll('button').find(b => b.getAttribute('aria-label') === label || b.textContent === label); },
    cards() { return document.body.querySelectorAll('.er-card'); } };
  return h;
}
function relatedFixture() {
  const a = table('a', 'db', [column('id', true), column('b_id')]), b = table('b'), c = table('c'), d = table('d');
  a.foreign_keys = [{ name: 'a_b', columns: ['b_id'], ref_table: 'b', ref_schema: 'db', ref_columns: ['id'] }];
  c.foreign_keys = [{ name: 'c_d', columns: ['id'], ref_table: 'd', ref_schema: 'db', ref_columns: ['id'] }];
  return metadata([a, b, c, d]);
}

const editorElement = h => h.document.body.querySelector('.er-te');
const syncButton = h => h.document.body.querySelector('.er-toolbar-write').querySelector('.primary');
const callsOf = (h, kind) => h.calls.filter(c => c[0] === kind);

test('各数据库完整读取挂起时字段/索引/跨模式外键摘要可读，失败仍保留且不可保存', async () => {
  for (const kind of ['tidb', 'mysql', 'mariadb', 'postgresql', 'opengauss', 'sqlite', 'sqlserver']) {
    const pending = deferred();
    const summary = table('items', 'db', [column('summary_id', true)]);
    summary.indexes = [{ name: 'idx_summary', columns: ['summary_id'], unique: true, primary: false }];
    summary.foreign_keys = [{ name: 'fk_summary', columns: ['summary_id'], ref_table: 'parents', ref_schema: 'other', ref_columns: ['parent_id'] }];
    const h = harness({ realEditor: true, connections: [{ ...connection('a'), kind }], load: () => metadata([summary]), design: () => pending.promise });
    const dlg = h.open(); await h.flush();
    assert.match(editorElement(h).querySelector('.er-te-summary').textContent, /summary_id/);
    assert.equal(editorElement(h).querySelectorAll('input').length, 0, kind + ' 摘要只包含文本');
    assert.equal(syncButton(h).disabled, true);
    h.button('索引').emit('click'); assert.match(editorElement(h).textContent, /idx_summary.*唯一索引/);
    h.button('外键').emit('click'); assert.match(editorElement(h).textContent, /fk_summary.*other.parents/);
    assert.equal(callsOf(h, 'reference-tables').length, 0, '摘要不查询引用候选');
    pending.reject(new Error('fixture metadata timeout')); await h.flush();
    assert.equal(editorElement(h).dataset.loadState, 'error'); assert.match(editorElement(h).textContent, /fk_summary/);
    assert.ok(h.button('重试读取表设计')); assert.equal(callsOf(h, 'preview').length, 0); dlg.close();
  }
});

test('快速切表最多两个真实在途请求，撤销中间排队，切回在途/成功表不重复读', async () => {
  const pending = new Map();
  const h = harness({ realEditor: true, load: () => metadata(['a', 'b', 'c', 'd', 'e'].map(n => table(n))),
    design: a => { const p = deferred(); pending.set(a.table, { ...p, args: a }); return p.promise; } });
  const dlg = h.open(); await h.flush();
  const select = async name => { h.cards().find(c => c.dataset.table === name).emit('click'); await h.flush(); };
  await select('b'); await select('c'); await select('d'); await select('e');
  assert.deepEqual(callsOf(h, 'design').map(([, a]) => a.table), ['a', 'b']);
  assert.match(editorElement(h).textContent, /排队/); assert.match(editorElement(h).querySelector('.er-te-summary').textContent, /只读/);
  await select('a'); assert.equal(callsOf(h, 'design').length, 2);
  await select('e'); pending.get('a').resolve(designSnapshot(pending.get('a').args)); await h.flush();
  assert.deepEqual(callsOf(h, 'design').map(([, a]) => a.table), ['a', 'b', 'e']);
  for (const name of ['b', 'e']) pending.get(name).resolve(designSnapshot(pending.get(name).args)); await h.flush();
  const ready = editorElement(h); await select('b'); await select('e');
  assert.equal(editorElement(h), ready); assert.equal(callsOf(h, 'design').length, 3); dlg.close();
});

test('慢提示只更新反馈、不释放实际在途槽位或自动重试；关闭撤销排队和定时器', async () => {
  const pending = new Map();
  const h = harness({ realEditor: true, load: () => metadata(['a', 'b', 'c'].map(n => table(n))),
    design: a => { const p = deferred(); pending.set(a.table, { ...p, args: a }); return p.promise; } });
  const dlg = h.open(); await h.flush(); h.cards().find(c => c.dataset.table === 'b').emit('click'); await h.flush();
  for (const [id, fire] of [...h.timers]) { h.timers.delete(id); fire(); }
  assert.match(editorElement(h).textContent, /读取较慢/); assert.equal(h.button('重试读取表设计'), undefined);
  h.cards().find(c => c.dataset.table === 'c').emit('click'); await h.flush(); assert.equal(callsOf(h, 'design').length, 2);
  dlg.close(); assert.equal(h.timers.size, 0);
  for (const p of pending.values()) p.resolve(designSnapshot(p.args)); await h.flush();
  assert.equal(callsOf(h, 'design').length, 2); assert.equal(h.document.body.children.length, 0);
});

test('在途读取期间刷新不接受旧快照、不并发重读，结束后读取最新代次', async () => {
  const old = deferred(), fresh = deferred(); const requests = [];
  const h = harness({ realEditor: true, design: a => { requests.push(a); return requests.length === 1 ? old.promise : fresh.promise; } });
  const dlg = h.open(); await h.flush(); h.button('刷新').emit('click'); h.button('刷新').emit('click'); await h.flush();
  assert.equal(requests.length, 1);
  const stale = designSnapshot(requests[0]); stale.design.columns[0].name = 'stale_id'; old.resolve(stale); await h.flush();
  assert.equal(requests.length, 2); assert.equal(editorElement(h).dataset.loadState, 'loading');
  assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]'), null, '旧回执不转换草稿');
  const current = designSnapshot(requests[1]); current.design.columns[0].name = 'fresh_id'; fresh.resolve(current); await h.flush();
  assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]').value, 'fresh_id'); dlg.close();
});

test('刷新同时失效未选中的干净设计器，切回按新代次读取；脏草稿保持原样', async () => {
  const h = harness({ realEditor: true, load: () => metadata(['a', 'b', 'c'].map(n => table(n))) }); const dlg = h.open(); await h.flush();
  h.cards().find(c => c.dataset.table === 'b').emit('click'); await h.flush();
  const input = editorElement(h).querySelector('[aria-label="字段名称"]'); input.value = 'draft_b'; input.emit('input');
  h.cards().find(c => c.dataset.table === 'c').emit('click'); await h.flush(); h.button('刷新').emit('click'); await h.flush();
  assert.equal(callsOf(h, 'design').length, 4);
  h.cards().find(c => c.dataset.table === 'a').emit('click'); await h.flush(); assert.equal(callsOf(h, 'design').length, 5);
  h.cards().find(c => c.dataset.table === 'b').emit('click'); await h.flush(); assert.equal(callsOf(h, 'design').length, 5);
  assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]'), input); assert.equal(input.value, 'draft_b'); dlg.close();
});

test('完整回执的 owner/模式/表名不匹配时拒绝，不能将其它表误转成基线', async () => {
  for (const mutate of [s => { s.owner_id = 'other-owner'; }, s => { s.design.schema = 'other'; }, s => { s.design.table = 'other'; }]) {
    const h = harness({ realEditor: true, design: a => { const s = designSnapshot(a); mutate(s); return s; } }); const dlg = h.open(); await h.flush();
    assert.equal(editorElement(h).dataset.loadState, 'error'); assert.match(editorElement(h).textContent, /归属不一致/);
    assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]'), null); assert.ok(editorElement(h).querySelector('.er-te-summary')); dlg.close();
  }
});

test('固定两个工具栏排，写操作与画布工具分组；编辑器挂载后激活', async () => {
  const h = harness(); const dlg = h.open(); await h.flush();
  const toolbar = h.document.body.querySelector('.er-toolbar');
  assert.equal(toolbar.children.length, 2);
  assert.equal(toolbar.children[0].getAttribute('aria-label'), '数据源与浏览');
  assert.equal(h.button('刷新').parentElement, toolbar.children[0]);
  assert.equal(h.button('＋ 新建表').parentElement.getAttribute('aria-label'), '数据库写操作');
  assert.equal(h.button('自动布局').parentElement.getAttribute('aria-label'), '画布工具');
  assert.equal(h.button('放大').parentElement.getAttribute('aria-label'), '统计与缩放');
  assert.ok(h.window.editors[0].activated && h.window.editors[0].mountedAtActivation);
  dlg.close();
});

test('真实编辑器激活发起完整设计读取，重复激活不并发，成功才开放编辑', async () => {
  const pending = deferred(); let args;
  const h = harness({ realEditor: true, design: a => { args = a; return pending.promise; } });
  const dlg = h.open(); await h.flush();
  assert.equal(callsOf(h, 'design').length, 1);
  assert.equal(editorElement(h).dataset.loadState, 'loading');
  assert.equal(editorElement(h).getAttribute('aria-busy'), 'false', '已有摘要不被读屏 busy 阻塞');
  assert.match(editorElement(h).textContent, /正在后台补齐完整表设计/);
  assert.match(editorElement(h).querySelector('.er-te-summary').textContent, /只读/);
  assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]'), null, '摘要不伪造可编辑草稿');
  assert.equal(syncButton(h).disabled, true);
  h.button('工程 ER 图').emit('click'); h.button('表结构图').emit('click'); await h.flush();
  assert.equal(callsOf(h, 'design').length, 1);
  pending.resolve(designSnapshot(args)); await h.flush();
  assert.equal(editorElement(h).dataset.loadState, 'ready');
  assert.equal(editorElement(h).getAttribute('aria-busy'), 'false');
  assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]').disabled, false);
  assert.match(editorElement(h).querySelector('.er-te-status').textContent, /完整表设计已读取/);
  assert.equal(syncButton(h).disabled, true, '未编辑基线不能同步');
  dlg.close();
});

test('真实读取失败显示诊断与重试；重渲染不吞错误、不自动反复请求', async () => {
  let count = 0; const pending = deferred(); let args;
  const h = harness({ realEditor: true, design: a => { args = a; if (++count === 1) throw { message: '<metadata denied>', code: 'ACL_DENIED', retryable: true }; return pending.promise; } });
  const dlg = h.open(); await h.flush();
  const error = editorElement(h).querySelector('[role="alert"]');
  assert.match(error.textContent, /<metadata denied>.*\n\[.*ACL_DENIED/);
  assert.equal(error.innerHTML, '', '错误通过 textContent 显示');
  assert.equal(editorElement(h).dataset.loadState, 'error');
  assert.equal(editorElement(h).querySelectorAll('.er-fedit').length, 0);
  h.button('工程 ER 图').emit('click'); h.button('表结构图').emit('click'); await h.flush();
  assert.equal(count, 1); assert.equal(editorElement(h).dataset.loadState, 'error');
  const retry = h.button('重试读取表设计'); retry.emit('click'); retry.emit('click'); await h.flush();
  assert.equal(count, 2); assert.equal(editorElement(h).dataset.loadState, 'loading');
  pending.resolve(designSnapshot(args)); await h.flush();
  assert.equal(editorElement(h).dataset.loadState, 'ready');
  assert.equal(editorElement(h).querySelector('[role="alert"]'), null);
  dlg.close();
});

test('partial、未知对象、缺失版本或指纹均拒绝，绝不由关系图表壳补成完整基线', async () => {
  for (const mutate of [s => { s.completeness = 'partial'; }, s => { s.objects.completeness = 'unknown'; }, s => { s.revision = ''; }, s => { s.source_fingerprint = ''; }]) {
    const h = harness({ realEditor: true, design: a => { const s = designSnapshot(a); mutate(s); return s; } });
    const dlg = h.open(); await h.flush();
    assert.equal(h.cards().length, 1);
    assert.equal(editorElement(h).dataset.loadState, 'error');
    assert.match(editorElement(h).textContent, /元数据不完整或未知/);
    assert.equal(editorElement(h).querySelectorAll('.er-fedit').length, 0);
    assert.equal(syncButton(h).disabled, true);
    assert.equal(callsOf(h, 'preview').length, 0);
    dlg.close();
  }
});

test('刷新重读干净表，失败仍保留上次基线并禁用写入，显式重试可以恢复', async () => {
  let fail = false;
  const h = harness({ realEditor: true, design: a => { if (fail) throw new Error('readback offline'); return designSnapshot(a); } });
  const dlg = h.open(); await h.flush(); fail = true;
  h.button('刷新').emit('click'); await h.flush();
  assert.equal(callsOf(h, 'design').length, 2);
  assert.equal(editorElement(h).dataset.loadState, 'error');
  assert.match(editorElement(h).textContent, /readback offline/);
  assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]').value, 'id');
  assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]').disabled, true);
  fail = false; h.button('重试读取表设计').emit('click'); await h.flush();
  assert.equal(editorElement(h).dataset.loadState, 'ready');
  assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]').disabled, false);
  dlg.close();
});

test('刷新与切表保留未保存草稿，切回使用同一编辑器、owner及字段输入', async () => {
  const h = harness({ realEditor: true, load: () => metadata([table('a'), table('b')]) }); const dlg = h.open(); await h.flush();
  const original = editorElement(h), input = original.querySelector('[aria-label="字段名称"]');
  input.value = 'draft_id'; input.emit('input'); assert.equal(syncButton(h).disabled, false);
  const firstOwner = callsOf(h, 'design')[0][1].ownerId;
  h.button('刷新').emit('click'); await h.flush();
  assert.equal(callsOf(h, 'design').length, 1); assert.equal(input.value, 'draft_id');
  h.cards().find(c => c.dataset.table === 'b').emit('click'); await h.flush();
  assert.equal(callsOf(h, 'design').length, 2);
  h.cards().find(c => c.dataset.table === 'a').emit('click'); await h.flush();
  assert.equal(editorElement(h), original);
  assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]'), input);
  assert.equal(input.value, 'draft_id'); assert.equal(callsOf(h, 'design')[0][1].ownerId, firstOwner);
  assert.equal(callsOf(h, 'design').length, 2); dlg.close();
});

test('SQLite 空模式仍激活真实表设计读取，读取命名空间不被改造', async () => {
  const cfg = { ...connection('a'), kind: 'sqlite' };
  const h = harness({ realEditor: true, connections: [cfg], list: () => [], load: () => metadata([table('items', '')]) });
  const dlg = h.open('a', ''); await h.flush();
  assert.equal(callsOf(h, 'design').length, 1); assert.equal(callsOf(h, 'design')[0][1].schema, '');
  assert.equal(editorElement(h).dataset.loadState, 'ready'); dlg.close();
});

test('首次关系图加载失败覆盖层限于画布，提供可操作的重试按钮', async () => {
  let fail = true; const h = harness({ load: () => { if (fail) throw new Error('diagram offline'); return metadata([table('items')]); } });
  const dlg = h.open(); await h.flush(); const overlay = h.document.body.querySelector('.er-overlay');
  assert.equal(overlay.parentElement.className, 'er-canvas');
  assert.match(overlay.textContent, /diagram offline/); assert.equal(overlay.getAttribute('role'), 'alert');
  fail = false; h.button('重试加载关系图').emit('click'); await h.flush();
  assert.equal(overlay.hidden, true); assert.equal(h.cards().length, 1); dlg.close();
});

test('引用表和引用字段读取失败显示局部错误及重试，重新渲染不形成读取循环', async () => {
  let failTables = true, failColumns = true;
  const h = harness({ realEditor: true, refTables: () => { if (failTables) throw new Error('tables denied'); return [['parents', 'table']]; },
    refColumns: () => { if (failColumns) throw new Error('columns denied'); return { columns: [column('parent_id')] }; } });
  const dlg = h.open(); await h.flush(); h.button('外键').emit('click'); await h.flush();
  assert.match(editorElement(h).textContent, /tables denied/); assert.equal(callsOf(h, 'reference-tables').length, 1);
  h.button('字段').emit('click'); h.button('外键').emit('click'); await h.flush();
  assert.equal(callsOf(h, 'reference-tables').length, 1);
  failTables = false; h.button('重试读取引用表').emit('click'); await h.flush();
  const target = editorElement(h).querySelector('[aria-label="引用表"]'); target.value = 'parents'; target.emit('change'); await h.flush();
  assert.match(editorElement(h).textContent, /columns denied/);
  assert.equal(callsOf(h, 'reference-columns').length, 1);
  failColumns = false; h.button('重试读取引用字段').emit('click'); await h.flush();
  const ref = editorElement(h).querySelector('[aria-label="引用字段"]');
  assert.equal(ref.disabled, false); assert.ok(ref.children.some(o => o.value === 'parent_id'));
  assert.equal(callsOf(h, 'reference-columns').length, 2); dlg.close();
});

test('关闭与切库后的设计成功或失败晚返回不触发旧编辑器 DOM 或 host 通知', async () => {
  for (const failed of [false, true]) {
    const pending = deferred(); const h = harness({ realEditor: true, design: () => pending.promise });
    const holder = h.document.createElement('div'); h.document.body.appendChild(holder); let changes = 0;
    const editor = new h.module.ErTableEditor({ connectionId: 'a', schema: 'db', table: 'items', ownerId: 'owner' },
      { connection: () => connection('a'), writeReason: () => null, changed: () => changes++, toast() { throw new Error('disposed toast'); } }, holder);
    editor.activate(); await h.flush(); const before = changes, children = [...holder.children]; editor.destroy();
    failed ? pending.reject(new Error('late failure')) : pending.resolve(designSnapshot()); await h.flush();
    assert.equal(changes, before); assert.deepEqual(holder.children, children);
  }
  const pending = deferred(); const h = harness({ realEditor: true, design: a => a.schema === 'db' ? pending.promise : designSnapshot(a) });
  const dlg = h.open(); await h.flush(); const select = h.document.body.querySelector('[aria-label="数据库"]');
  select.value = 'other'; select.emit('change'); await h.flush(); const current = editorElement(h);
  pending.reject(new Error('old namespace error')); await h.flush();
  assert.equal(editorElement(h), current); assert.equal(current.dataset.loadState, 'ready');
  assert.doesNotMatch(h.document.body.textContent, /old namespace error/); dlg.close();
});

test('只读连接允许读取，禁用并阻止新建、删除和同步写入口', async () => {
  const cfg = { ...connection('a'), read_only: true }; const h = harness({ realEditor: true, connections: [cfg] });
  const dlg = h.open(); await h.flush(); assert.equal(editorElement(h).dataset.loadState, 'ready');
  for (const button of [h.button('＋ 新建表'), h.button('删除表'), syncButton(h)]) {
    assert.equal(button.disabled, true); button.emit('click');
  }
  await h.flush(); assert.equal(callsOf(h, 'ddl').length, 0); assert.equal(callsOf(h, 'preview').length, 0);
  assert.match(editorElement(h).textContent, /只读/); dlg.close();
});

test('新建确认期间连接改为只读，发送 DDL 前再次检查权限', async () => {
  const cfg = connection('a'); const h = harness({ connections: [cfg] }); const dlg = h.open(); await h.flush();
  h.button('＋ 新建表').emit('click'); const prompt = h.document.body.querySelectorAll('.dialog').at(-1);
  prompt.querySelector('input').value = 'created'; cfg.read_only = true;
  prompt.querySelectorAll('button').find(b => b.textContent === '确定').emit('click'); await h.flush();
  assert.equal(callsOf(h, 'ddl').length, 0); assert.match(h.document.body.querySelector('.er-insp-message').textContent, /只读/); dlg.close();
});

test('表设计预览晚返回后关闭不再打开同步确认，也不发送 grant 或执行', async () => {
  const pending = deferred(); const h = harness({ realEditor: true, preview: () => pending.promise }); const dlg = h.open(); await h.flush();
  const input = editorElement(h).querySelector('[aria-label="字段名称"]'); input.value = 'draft_id'; input.emit('input');
  syncButton(h).emit('click'); await h.flush(); assert.equal(callsOf(h, 'preview').length, 1); dlg.close();
  pending.resolve({ request_sql: 'fixture', revision: 'fixture-r1', steps: [], destructive: false, limitations: [] }); await h.flush();
  assert.equal(h.document.body.children.length, 0); assert.equal(callsOf(h, 'grant').length, 0); assert.equal(callsOf(h, 'execute').length, 0);
});

test('旧基线引用字段响应不能覆盖重新读取后的候选，失败状态也不重新请求', async () => {
  const old = deferred(); let count = 0;
  const h = harness({ realEditor: true, refColumns: () => ++count === 1 ? old.promise : { columns: [column('fresh_id')] } });
  const dlg = h.open(); await h.flush(); h.button('外键').emit('click'); await h.flush();
  let target = editorElement(h).querySelector('[aria-label="引用表"]'); target.value = 'parents'; target.emit('change'); await h.flush();
  h.button('刷新').emit('click'); await h.flush();
  target = editorElement(h).querySelector('[aria-label="引用表"]'); target.value = 'parents'; target.emit('change'); await h.flush();
  assert.equal(count, 2); old.resolve({ columns: [column('stale_id')] }); await h.flush();
  const columns = editorElement(h).querySelector('[aria-label="引用字段"]').children.map(o => o.value);
  assert.ok(columns.includes('fresh_id')); assert.ok(!columns.includes('stale_id')); dlg.close();
});

test('预览期间草稿变化不能执行旧载荷，保留新草稿并明确显示失败', async () => {
  const pending = deferred(); const h = harness({ realEditor: true, preview: () => pending.promise }); const dlg = h.open(); await h.flush();
  const input = editorElement(h).querySelector('[aria-label="字段名称"]'); input.value = 'first_draft'; input.emit('input');
  syncButton(h).emit('click'); await h.flush(); input.value = 'new_draft'; input.emit('input');
  pending.resolve({ request_sql: 'fixture', revision: 'fixture-r1', steps: [], destructive: false, limitations: [] }); await h.flush();
  assert.match(editorElement(h).textContent, /预览期间草稿发生变化/);
  assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]').value, 'new_draft');
  assert.equal(callsOf(h, 'grant').length, 0); assert.equal(callsOf(h, 'execute').length, 0); dlg.close();
});

function designResponse({ request, design }, verified) {
  const snapshot = designSnapshot({ schema: design.schema, table: design.table, ownerId: request.owner_id });
  snapshot.design = { ...snapshot.design, ...structuredClone(design), columns: design.columns.map(c => ({ ...c, orig_name: c.name })) };
  snapshot.revision = 'fixture-r2'; snapshot.source_fingerprint = 'fixture-f2';
  return { outcome: { execution_id: request.execution_id, state: 'succeeded', resources_pending: false, transaction_state: 'none', error: null },
    receipt: { verified, unverified: !verified, mismatches: verified ? [] : ['readback incomplete'], steps: [{ status: 'applied' }], transaction: 'committed', snapshot } };
}

test('真实受控同步只有验证回执才清草稿；未验证回执显示失败、保留草稿与错误', async () => {
  for (const verified of [true, false]) {
    let persisted;
    const h = harness({ realEditor: true, design: a => persisted ? { ...structuredClone(persisted), owner_id: a.ownerId } : designSnapshot(a),
      execute: a => { const response = designResponse(a, verified); if (verified) persisted = response.receipt.snapshot; return response; } });
    const dlg = h.open(); await h.flush();
    const input = editorElement(h).querySelector('[aria-label="字段名称"]'); input.value = 'draft_id'; input.emit('input');
    syncButton(h).emit('click'); await h.flush(); h.button('执行同步（1）').emit('click'); await h.flush();
    assert.equal(callsOf(h, 'grant').length, 1); assert.equal(callsOf(h, 'execute').length, 1);
    assert.equal(editorElement(h).querySelector('[aria-label="字段名称"]').value, 'draft_id');
    if (verified) {
      assert.equal(syncButton(h).disabled, true); assert.ok(persisted); assert.equal(editorElement(h).querySelector('[role="alert"]'), null);
    } else {
      assert.match(editorElement(h).querySelector('[role="alert"]').textContent, /未证实完整成功.*readback incomplete/);
      assert.match(editorElement(h).querySelector('.er-te-status').textContent, /未保存/);
      h.button('刷新').emit('click'); await h.flush();
      assert.equal(callsOf(h, 'execute').length, 1); assert.match(editorElement(h).textContent, /readback incomplete/);
    }
    dlg.close();
  }
});

test('受控执行晚返回后关闭不改写旧草稿或触发成功通知', async () => {
  const pending = deferred(); let args; const h = harness({ realEditor: true, execute: a => { args = a; return pending.promise; } });
  const holder = h.document.createElement('div'); h.document.body.appendChild(holder); let changes = 0; const notices = [];
  const editor = new h.module.ErTableEditor({ connectionId: 'a', schema: 'db', table: 'items', ownerId: 'owner' },
    { connection: () => connection('a'), writeReason: () => null, changed: () => changes++, toast: (...n) => notices.push(n) }, holder);
  editor.activate(); await h.flush(); const input = holder.querySelector('[aria-label="字段名称"]'); input.value = 'draft_id'; input.emit('input');
  const saved = editor.save(); await h.flush(); h.button('执行同步（1）').emit('click'); await h.flush();
  assert.equal(callsOf(h, 'execute').length, 1); const before = changes, children = [...holder.children]; editor.destroy();
  pending.resolve(designResponse(args, true)); await saved; await h.flush();
  assert.equal(changes, before); assert.deepEqual(holder.children, children); assert.deepEqual(notices, []); assert.equal(editor.draftDirty(), true);
});

test('缓存第二次开窗不发metadata IPC；刷新按钮force并保留拖动位置', async () => {
  const h = harness(); const first = h.open(); await h.flush(); assert.equal(h.cards().length, 1);
  const card = h.cards()[0], header = card.querySelector('.er-card-header');
  header.emit('mousedown', { clientX: 0, clientY: 0 }); h.emit('mousemove', { clientX: 120, clientY: 40 }); h.frame(); h.emit('mouseup', {});
  const position = card.style.transform; first.close();
  const second = h.open(); await h.flush(); assert.equal(h.calls.filter(c => c[0] === 'metadata').length, 1); assert.equal(h.cards()[0].style.transform, position);
  h.button('刷新').emit('click'); await h.flush(); assert.equal(h.calls.filter(c => c[0] === 'metadata').length, 2); assert.equal(h.cards()[0].style.transform, position); second.close();
});

test('切连接重新列数据库，旧metadata晚返回不能覆盖新scope', async () => {
  const old = deferred(); const h = harness({ list: a => [[a.id === 'a' ? 'old' : 'new', 'mysql']], load: a => a.id === 'a' ? old.promise : metadata([table('new_table', 'new')]) });
  const dlg = h.open('a', 'old'); await h.settle();
  const select = h.document.body.querySelector('[aria-label="连接"]'); select.value = 'b'; select.emit('change'); await h.flush();
  assert.equal(h.cards()[0].dataset.table, 'new_table'); assert.equal(h.document.body.querySelector('[aria-label="数据库"]').value, 'new');
  old.resolve(metadata([table('stale', 'old')])); await h.flush(); assert.equal(h.cards()[0].dataset.table, 'new_table');
  assert.deepEqual(h.calls.filter(c => c[0] === 'list').map(c => c[1].id), ['a', 'b']); dlg.close();
});

test('同scope强刷晚返回普通请求不覆盖新快照，缓存深clone隔离原metadata', async () => {
  const old = deferred(); const fresh = metadata([table('fresh')]); let loads = 0;
  const h = harness({ load: () => ++loads === 1 ? old.promise : fresh }); const dlg = h.open(); await h.settle();
  h.button('刷新').emit('click'); await h.flush(); assert.equal(h.cards()[0].dataset.table, 'fresh'); fresh.tables[0].table = 'mutated_source';
  old.resolve(metadata([table('stale')])); await h.flush(); assert.equal(h.cards()[0].dataset.table, 'fresh'); dlg.close();
  const second = h.open(); await h.flush(); assert.equal(h.cards()[0].dataset.table, 'fresh'); assert.equal(loads, 2); second.close();
});

test('旧数据库目录晚返回也不能覆盖最新目录；空库正确提交空图', async () => {
  const old = deferred(); const h = harness({ list: a => a.id === 'a' ? old.promise : [['new', 'mysql']], load: () => metadata([]) });
  const dlg = h.open(); const select = h.document.body.querySelector('[aria-label="连接"]'); select.value = 'b'; select.emit('change'); await h.flush();
  old.resolve([['old', 'mysql']]); await h.flush(); assert.equal(h.document.body.querySelector('[aria-label="数据库"]').value, 'new'); assert.equal(h.cards().length, 0); dlg.close();
});

test('拖动合并rAF、保持DOM identity，只写邻接路径且基数徽标同帧移动', async () => {
  const h = harness({ load: relatedFixture }); const dlg = h.open(); await h.flush();
  const card = h.cards().find(c => c.dataset.table === 'a');
  const edges = h.document.body.querySelectorAll('path.er-edge'); const related = edges.find(e => e.dataset.source === 'a'), unrelated = edges.find(e => e.dataset.source === 'c');
  const badge = related.nextSibling.firstChild, before = badge.getAttribute('x'), path = related.getAttribute('d'); unrelated.writes = [];
  card.querySelector('.er-card-header').emit('mousedown', { clientX: 0, clientY: 0 });
  for (let i = 0; i < 80; i++) h.emit('mousemove', { clientX: i, clientY: i });
  assert.equal(h.frames.size, 1); assert.equal(related.getAttribute('d'), path); h.frame();
  assert.ok(h.cards().includes(card)); assert.notEqual(related.getAttribute('d'), path); assert.notEqual(badge.getAttribute('x'), before); assert.deepEqual(unrelated.writes, []);
  h.emit('mouseup', {}); h.frame(); dlg.close();
});

test('schema/ref_schema/indexes保留，跨scope FK不误绑同名表，unique索引基数为1', async () => {
  const a = table('a', 'db', [column('id', true)]), b = table('b');
  a.foreign_keys = [{ name: 'external', columns: ['id'], ref_table: 'b', ref_schema: 'other', ref_columns: ['id'] }];
  a.indexes = [{ columns: ['id'], unique: true, primary: false }];
  const h = harness({ load: () => metadata([a, b]) }); const dlg = h.open(); await h.flush(); assert.equal(h.document.body.querySelectorAll('path.er-edge').length, 0);
  a.foreign_keys[0].ref_schema = 'db'; h.button('刷新').emit('click'); await h.flush();
  assert.equal(h.document.body.querySelectorAll('path.er-edge').length, 1); assert.deepEqual(h.document.body.querySelectorAll('.er-card-badge').map(b => b.textContent), ['1', '1']); dlg.close();
});

test('partial失败保留表壳并提示，刷新失败保留已显示图且允许重试', async () => {
  let fail = false; const h = harness({ load: () => { if (fail) throw new Error('offline'); return { tables: [table('shell', 'db', [])], failures: [{ table: 'shell', error: 'columns denied' }], limitations: ['keys unavailable'] }; } });
  const dlg = h.open(); await h.flush(); assert.equal(h.cards()[0].dataset.table, 'shell');
  assert.match(h.document.body.querySelector('.er-insp-message').textContent, /columns denied.*keys unavailable/);
  fail = true; h.button('刷新').emit('click'); await h.flush(); assert.equal(h.cards().length, 1); assert.match(h.document.body.querySelector('.er-insp-message').textContent, /offline/); dlg.close();
});

test('大图>200裁剪且高列表只挂11行，平移重用存留DOM不重新layout', async () => {
  const data = metadata(Array.from({ length: 450 }, (_, i) => table(`t${String(i).padStart(3, '0')}`, 'db', Array.from({ length: 100 }, (_, j) => column(`c${j}`, j === 0)))));
  const h = harness({ load: () => data }); const dlg = h.open(); await h.flush();
  const before = h.cards(); assert.ok(before.length > 0 && before.length < 450); assert.equal(before[0].querySelectorAll('.er-card-row').length, 11);
  const canvas = h.document.body.querySelector('.er-canvas'); canvas.emit('mousedown', { clientX: 0, clientY: 0 }); h.emit('mousemove', { clientX: -100, clientY: -100 }); h.frame();
  assert.ok(h.cards().includes(before[0])); assert.equal(h.calls.filter(c => c[0] === 'metadata').length, 1); h.emit('mouseup', {}); dlg.close();
});

test('工程>2000图元裁剪，平移不会重建可见SVG，搜索/模式保持可用', async () => {
  const data = metadata(Array.from({ length: 180 }, (_, i) => table(`t${i}`, 'db', Array.from({ length: 12 }, (_, j) => column(`c${j}`)))));
  const h = harness({ load: () => data }); const dlg = h.open(); await h.flush(); h.button('工程 ER 图').emit('click'); await h.flush();
  // 工程按钮沿用自动fit，放大至阈值以上再观察局部可见区域。
  for (let i = 0; i < 12; i++) h.button('放大').emit('click'); h.frame();
  const visible = h.document.body.querySelectorAll('.er-eng-attr'); assert.ok(visible.length < 2160);
  const canvas = h.document.body.querySelector('.er-canvas'); canvas.emit('mousedown', { clientX: 0, clientY: 0 }); h.emit('mousemove', { clientX: 1, clientY: 1 }); h.frame();
  assert.ok(visible.every(e => !e.isConnected || h.document.body.querySelectorAll('.er-eng-attr').includes(e))); h.emit('mouseup', {});
  const search = h.document.body.querySelector('[aria-label="搜索表"]'); search.value = 't123'; search.emit('input'); for (const f of h.timers.values()) f(); h.timers.clear(); h.frame(); assert.equal(h.document.body.querySelectorAll('.er-eng-entity').length, 1);
  h.button('表结构图').emit('click'); h.frame(); assert.equal(h.document.body.querySelectorAll('.er-eng-attr').length, 0); assert.equal(h.cards().length, 1); dlg.close();
});

test('关闭撤销监听、observer、rAF、搜索和剪贴板timer；pending返回不触DOM', async () => {
  const wait = deferred(); const h = harness({ load: () => wait.promise }); const dlg = h.open(); await h.settle();
  h.document.body.querySelector('[aria-label="搜索表"]').emit('input'); h.button('复制 SQL').emit('click'); await h.settle();
  assert.ok(h.frames.size > 0); assert.ok(h.timers.size > 0); dlg.close();
  assert.equal(h.frames.size, 0); assert.equal(h.timers.size, 0); assert.equal(h.listeners.get('mousemove').length, 0); assert.equal(h.listeners.get('mouseup').length, 0); assert.ok(h.observers.every(o => o.disconnected));
  wait.resolve(metadata([table('too_late')])); await h.flush(); assert.equal(h.document.body.children.length, 0); assert.equal(h.window.editors.length, 0);
});

test('新建DDL成功必须失效缓存并force重读；切scope后的旧DDL只失效旧范围', async () => {
  const pending = deferred(); const h = harness({ ddl: () => pending.promise }); const dlg = h.open(); await h.flush();
  h.button('＋ 新建表').emit('click'); const dialogs = h.document.body.querySelectorAll('.dialog');
  const prompt = dialogs.at(-1); prompt.querySelector('input').value = 'created'; prompt.querySelectorAll('button').find(b => b.textContent === '确定').emit('click'); await h.settle();
  assert.equal(h.calls.filter(c => c[0] === 'ddl').length, 1);
  const select = h.document.body.querySelector('[aria-label="连接"]'); select.value = 'b'; select.emit('change'); await h.flush();
  pending.resolve(0); await h.flush(); assert.equal(h.calls.filter(c => c[0] === 'metadata').length, 2); assert.equal(h.window.editors.at(-1).opts.connectionId, 'b');
  // 切回旧范围：被DDL失效的旧缓存必须真的重读，而不是复用开窗快照。
  select.value = 'a'; select.emit('change'); await h.flush(); assert.equal(h.calls.filter(c => c[0] === 'metadata').length, 3); dlg.close();
});

test('drag-only邻接边越界先卸载后重挂，SVG增长且跨视口离屏端点保留穿线', async () => {
  const a = table('a'), b = table('b'); a.foreign_keys = [{ name: 'ab', columns: ['id'], ref_table: 'b', ref_schema: 'db', ref_columns: ['id'] }];
  const data = metadata([a, b, ...Array.from({ length: 210 }, (_, i) => table(`z${i}`))]);
  const h = harness({ load: () => data }); h.storage.set('navkit:diagram:positions:v1:a:db', JSON.stringify({ a: { x: 1500, y: 100 }, b: { x: 5000, y: 100 } }));
  const dlg = h.open(); await h.flush(); const card = h.cards().find(c => c.dataset.table === 'a'); assert.ok(card); assert.equal(h.document.body.querySelectorAll('path.er-edge').length, 1);
  card.querySelector('.er-card-header').emit('mousedown', { clientX: 0, clientY: 0 }); h.emit('mousemove', { clientX: 1000, clientY: 0 }); h.frame(); assert.equal(h.document.body.querySelectorAll('path.er-edge').length, 0);
  h.emit('mousemove', { clientX: 0, clientY: 0 }); h.frame(); const edge = h.document.body.querySelector('path.er-edge'); assert.ok(edge?.getAttribute('d')); assert.notEqual(edge.nextSibling.firstChild.getAttribute('x'), '0');
  const width = Number(h.document.body.querySelector('.er-edges').getAttribute('width')); h.emit('mousemove', { clientX: 6000, clientY: 0 }); h.frame(); assert.ok(Number(h.document.body.querySelector('.er-edges').getAttribute('width')) > width);
  h.emit('mouseup', {}); dlg.close();
  const crossing = harness({ load: () => data }); crossing.storage.set('navkit:diagram:positions:v1:a:db', JSON.stringify({ a: { x: -2000, y: 100 }, b: { x: 5000, y: 100 } })); const other = crossing.open(); await crossing.flush();
  assert.ok(!crossing.cards().some(c => ['a', 'b'].includes(c.dataset.table))); assert.equal(crossing.document.body.querySelectorAll('path.er-edge').length, 1); other.close();
});

test('模式安排的fit微任务关闭后不能测量旧DOM', async () => {
  const h = harness(); const dlg = h.open(); await h.flush(); const canvas = h.document.body.querySelector('.er-canvas'); let reads = 0; canvas.getBoundingClientRect = () => { reads++; return { width: 1000, height: 700 }; };
  h.button('工程 ER 图').emit('click'); dlg.close(); await h.flush(); assert.equal(reads, 0); assert.equal(h.frames.size, 0);
});

test('真实Chrome+原CSS：两排工具栏无溢出、卡片高度、拖动徽标、缓存force及大图裁剪', { timeout: 45000 }, async t => {
  const browserBundle = await build({ stdin: { resolveDir: root, loader: 'ts', contents: `import { openErDiagramDialog } from './src/dialogs/ErDiagramDialog'; window.openGraph = () => window.graphDialog = openErDiagramDialog(window.graphConnections, window.graphConnections[0], 'db');` },
    bundle: true, write: false, format: 'iife', platform: 'browser', loader: { '.css': 'empty' }, plugins, logLevel: 'silent' });
  const css = (await readFile(path.join(root, 'src/styles/base.css'), 'utf8')) + '\n' + (await readFile(path.join(root, 'src/styles/er-diagram.css'), 'utf8'));
  const install = `window.graphConnections=${JSON.stringify([connection('a')])}; window.editors=[]; window.saveEditor=async()=>{}; window.graphCalls=0; window.graphData=${JSON.stringify(relatedFixture())}; window.__PREVIEW_MOCK__={list_databases:()=>[['db','mysql']], get_diagram_metadata:()=>{window.graphCalls++; return structuredClone(window.graphData);}};`;
  const server = createServer((req, res) => { if (req.url === '/css') { res.setHeader('Content-Type', 'text/css'); res.end(css); } else if (req.url === '/js') { res.setHeader('Content-Type', 'text/javascript'); res.end(browserBundle.outputFiles[0].text); } else { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(`<!doctype html><meta charset="utf-8"><title>ER fixture: mock metadata; not native database acceptance</title><link rel="stylesheet" href="/css"><body><script>${install}</script><script src="/js"></script></body>`); } });
  let browser;
  try {
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
    const executablePath = [process.env.NAVKIT_TEST_BROWSER, process.env.BROWSER_EXE, 'C:/Program Files/Google/Chrome/Application/chrome.exe', 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', 'C:/Program Files/Microsoft/Edge/Application/msedge.exe', '/usr/bin/chromium', '/usr/bin/chromium-browser'].filter(Boolean).find(existsSync);
    browser = await chromium.launch({ headless: true, args: [`--explicitly-allowed-ports=${server.address().port}`], ...(executablePath ? { executablePath } : {}) });
    const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } }); const errors = []; page.on('pageerror', e => errors.push(e.message)); page.setDefaultTimeout(5000);
    await page.goto(`http://127.0.0.1:${server.address().port}/`); await page.evaluate(() => window.openGraph()); await page.waitForFunction(() => document.querySelectorAll('.er-card').length === 4);
    for (const width of [1600, 1100, 900, 720, 480, 360]) {
      await page.setViewportSize({ width, height: 1000 });
      const toolbar = await page.evaluate(() => {
        const el = document.querySelector('.er-toolbar'), bounds = el.getBoundingClientRect();
        const rows = [...el.children].map(row => ({ display: getComputedStyle(row).display, top: row.getBoundingClientRect().top, bottom: row.getBoundingClientRect().bottom }));
        const controls = [...el.querySelectorAll('button, select, input')].map(control => {
          const r = control.getBoundingClientRect(); return { name: control.getAttribute('aria-label') ?? control.textContent, left: r.left, right: r.right, width: r.width };
        });
        return { client: el.clientWidth, scroll: el.scrollWidth, left: bounds.left, right: bounds.right, rows, controls };
      });
      assert.equal(toolbar.rows.length, 2, `${width}px 固定两排容器`);
      assert.ok(toolbar.rows.every(row => row.display === 'grid'));
      assert.ok(toolbar.rows[1].top >= toolbar.rows[0].bottom, `${width}px 分排明确`);
      assert.ok(toolbar.scroll <= toolbar.client + 1, `${width}px 工具栏溢出: ${JSON.stringify(toolbar)}`);
      assert.ok(toolbar.controls.every(c => c.width > 0 && c.left >= toolbar.left - 1 && c.right <= toolbar.right + 1), `${width}px 控件应全部可见: ${JSON.stringify(toolbar.controls)}`);
    }
    await page.setViewportSize({ width: 1600, height: 1000 });
    const drag = await page.evaluate(async () => {
      const card = document.querySelector('.er-card[data-table="a"]'), edge = document.querySelector('path[data-source="a"]'), badge = edge.nextSibling.firstChild;
      const before = [card.style.transform, edge.getAttribute('d'), badge.getAttribute('x')];
      card.querySelector('.er-card-header').dispatchEvent(new MouseEvent('mousedown', { clientX: 0, clientY: 0, bubbles: true }));
      for (let i = 0; i < 100; i++) window.dispatchEvent(new MouseEvent('mousemove', { clientX: i, clientY: i }));
      await new Promise(requestAnimationFrame); const after = [card.style.transform, edge.getAttribute('d'), badge.getAttribute('x')]; window.dispatchEvent(new MouseEvent('mouseup'));
      return { before, after, stable: card === document.querySelector('.er-card[data-table="a"]') };
    });
    assert.ok(drag.stable); assert.notEqual(drag.before[0], drag.after[0]); assert.notEqual(drag.before[1], drag.after[1]); assert.notEqual(drag.before[2], drag.after[2]);
    await page.evaluate(() => { window.graphDialog.close(); window.openGraph(); }); await page.waitForFunction(() => document.querySelectorAll('.er-card').length === 4); assert.equal(await page.evaluate(() => window.graphCalls), 1);
    const large = metadata(Array.from({ length: 1000 }, (_, i) => table(`t${String(i).padStart(4, '0')}`, 'db', Array.from({ length: 40 }, (_, j) => column(`c${j}`, j === 0)))));
    await page.evaluate(data => { window.graphData = data; document.querySelector('button[aria-label="刷新"]').click(); }, large); await page.waitForFunction(() => document.querySelector('.er-stats')?.textContent.startsWith('1000 张表'));
    const state = await page.evaluate(async () => { await new Promise(requestAnimationFrame); const card = document.querySelector('.er-card'); return { cards: document.querySelectorAll('.er-card').length, rows: card.querySelectorAll('.er-card-row').length, height: card.getBoundingClientRect().height, calls: window.graphCalls }; });
    assert.ok(state.cards > 0 && state.cards < 1000); assert.equal(state.rows, 11); assert.equal(state.height, 276); assert.equal(state.calls, 2);
    const pan = await page.evaluate(async () => { const cards = [...document.querySelectorAll('.er-card')]; const canvas = document.querySelector('.er-canvas'); canvas.dispatchEvent(new MouseEvent('mousedown', { clientX: 0, clientY: 0, bubbles: true })); window.dispatchEvent(new MouseEvent('mousemove', { clientX: -1, clientY: -1 })); await new Promise(requestAnimationFrame); window.dispatchEvent(new MouseEvent('mouseup')); return cards.filter(c => c.isConnected).length; }); assert.ok(pan > 0);
    await page.getByRole('button', { name: '工程 ER 图', exact: true }).click();
    // 模式交互自动fit维持不变；连续放大到上限只为确定性局部裁剪断言。
    await page.evaluate(async () => { await new Promise(requestAnimationFrame); for (let i = 0; i < 20; i++) document.querySelector('button[aria-label="放大"]').click(); await new Promise(requestAnimationFrame); });
    const eng = await page.evaluate(() => ({ attrs: document.querySelectorAll('.er-eng-attr').length, entities: document.querySelectorAll('.er-eng-entity').length })); assert.ok(eng.attrs < 12000); assert.ok(eng.entities < 1000);
    await page.evaluate(() => window.graphDialog.close()); assert.equal(await page.locator('.er-dialog').count(), 0); assert.deepEqual(errors, []);
    t.diagnostic(`mock 1000表/40000列: 初始挂载${state.cards}卡/${state.rows}行每卡; 卡片实际高度${state.height}px; 工程zoom放大挂载${eng.entities}实体/${eng.attrs}属性。非原生DB/IPC性能数据。`);
  } finally { await browser?.close(); if (server.listening) await new Promise(resolve => server.close(resolve)); }
});

test('世界坐标裁剪使用zoom反变换及800 overscan，端点离屏穿过视口的线可见', () => {
  const h = harness(); const b = h.module.diagramViewport({ x: -100, y: -200, zoom: 0.5 }, 1000, 700);
  assert.deepEqual(JSON.parse(JSON.stringify(b)), { left: -600, top: -400, right: 3000, bottom: 2600 });
  assert.ok(h.module.intersectsDiagramViewport(h.module.edgeBounds({ x1: -10000, y1: 0, x2: 10000, y2: 0 }), { left: 0, top: -10, right: 100, bottom: 10 }));
});

test('真实Chrome+真实编辑器：读取失败可重试，刷新/切表保护草稿，窄窗错误完整可读', { timeout: 45000 }, async t => {
  const browserBundle = await build({ stdin: { resolveDir: root, loader: 'ts', contents: `import { openErDiagramDialog } from './src/dialogs/ErDiagramDialog'; window.openGraph = () => window.graphDialog = openErDiagramDialog(window.graphConnections, window.graphConnections[0], 'db');` },
    bundle: true, write: false, format: 'iife', platform: 'browser', loader: { '.css': 'empty' }, plugins: graphPlugins(true), logLevel: 'silent' });
  const css = (await Promise.all(['src/styles/base.css', 'src/styles/er-diagram.css', 'src/styles/er-table-editor.css'].map(file => readFile(path.join(root, file), 'utf8')))).join('\n');
  const snapshots = Object.fromEntries(['a', 'b'].map(name => [name, designSnapshot({ table: name })]));
  const install = `window.graphConnections=${JSON.stringify([connection('a')])}; window.graphData=${JSON.stringify(metadata([table('a'), table('b')]))}; window.snapshots=${JSON.stringify(snapshots)}; window.designMode='pending'; window.designCalls=[];
    window.__PREVIEW_MOCK__={list_databases:()=>[['db','mysql']], get_diagram_metadata:()=>structuredClone(window.graphData), read_table_design_snapshot:a=>{
      window.designCalls.push(a); if(window.designMode==='pending') return new Promise((resolve,reject)=>{window.pendingDesign={resolve,reject};});
      if(window.designMode==='error') throw {message:'没有读取完整表设计的权限，请检查当前连接。详细诊断：字段/约束读取失败。', code:'METADATA_DENIED', retryable:true};
      const s=structuredClone(window.snapshots[a.table]); s.owner_id=a.ownerId; return s;
    }};`;
  const server = createServer((req, res) => {
    if (req.url === '/css') { res.setHeader('Content-Type', 'text/css'); res.end(css); }
    else if (req.url === '/js') { res.setHeader('Content-Type', 'text/javascript'); res.end(browserBundle.outputFiles[0].text); }
    else { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(`<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>关系图编辑器显式 mock IPC 回归</title><link rel="stylesheet" href="/css"><body><script>${install}</script><script src="/js"></script></body></html>`); }
  });
  let browser;
  try {
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
    const executablePath = [process.env.NAVKIT_TEST_BROWSER, process.env.BROWSER_EXE, 'C:/Program Files/Google/Chrome/Application/chrome.exe', 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', 'C:/Program Files/Microsoft/Edge/Application/msedge.exe', '/usr/bin/chromium', '/usr/bin/chromium-browser'].filter(Boolean).find(existsSync);
    browser = await chromium.launch({ headless: true, args: [`--explicitly-allowed-ports=${server.address().port}`], ...(executablePath ? { executablePath } : {}) });
    const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } }); const errors = []; page.on('pageerror', e => errors.push(e.message)); page.setDefaultTimeout(6000);
    await page.goto(`http://127.0.0.1:${server.address().port}/`); await page.evaluate(() => window.openGraph());
    await page.waitForSelector('.er-te[data-load-state="loading"] .er-te-summary');
    assert.match(await page.locator('.er-te-summary').innerText(), /只读/);
    assert.equal(await page.locator('.er-te input').count(), 0, '完整快照挂起时不构造可写草稿');
    assert.equal(await page.getByRole('button', { name: '同步到数据库', exact: true }).isEnabled(), false);
    await page.getByRole('tab', { name: '索引', exact: true }).click();
    assert.equal(await page.evaluate(() => document.activeElement?.textContent), '索引');
    await page.getByRole('tab', { name: '字段', exact: true }).click();
    await page.setViewportSize({ width: 480, height: 1000 });
    const summaryBounds = await page.locator('.er-te-summary').evaluate(el => ({ client: el.clientWidth, scroll: el.scrollWidth }));
    assert.ok(summaryBounds.scroll <= summaryBounds.client + 1, '摘要在窄窗完整换行');
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.evaluate(() => window.pendingDesign.reject({ message: '没有读取完整表设计的权限，请检查当前连接。详细诊断：字段/约束读取失败。', code: 'METADATA_DENIED', retryable: true }));
    await page.waitForSelector('.er-te[data-load-state="error"]');
    assert.match(await page.locator('.er-te [role="alert"]').innerText(), /METADATA_DENIED/);
    assert.equal(await page.getByRole('button', { name: '重试读取表设计', exact: true }).isVisible(), true);
    await page.evaluate(() => { window.designMode = 'complete'; });
    await page.getByRole('button', { name: '重试读取表设计', exact: true }).click(); await page.waitForSelector('.er-te[data-load-state="ready"]');
    await page.getByLabel('字段名称', { exact: true }).fill('draft_id');
    await page.getByRole('button', { name: '刷新', exact: true }).click();
    await page.waitForFunction(() => document.querySelector('.er-overlay').hidden);
    assert.equal(await page.getByLabel('字段名称', { exact: true }).inputValue(), 'draft_id');
    assert.equal(await page.evaluate(() => window.designCalls.length), 2);
    await page.locator('.er-card[data-table="b"]').click(); await page.waitForFunction(() => document.querySelector('.er-te [aria-label="表名"]').value === 'b' && document.querySelector('.er-te').dataset.loadState === 'ready');
    await page.locator('.er-card[data-table="a"]').click(); assert.equal(await page.getByLabel('字段名称', { exact: true }).inputValue(), 'draft_id');
    await page.locator('.er-card[data-table="b"]').click(); await page.setViewportSize({ width: 480, height: 1000 });
    await page.evaluate(() => { window.designMode = 'error'; }); await page.getByRole('button', { name: '刷新', exact: true }).click(); await page.waitForSelector('.er-te[data-load-state="error"]');
    const notice = await page.locator('.er-te [role="alert"]').evaluate(el => ({ text: el.textContent, client: el.clientWidth, scroll: el.scrollWidth, nowrap: getComputedStyle(el).whiteSpace }));
    assert.match(notice.text, /METADATA_DENIED/); assert.ok(notice.scroll <= notice.client + 1); assert.equal(notice.nowrap, 'pre-wrap');
    assert.equal(await page.getByRole('button', { name: '重试读取表设计', exact: true }).isVisible(), true);
    await page.evaluate(() => { window.designMode = 'complete'; }); await page.getByRole('button', { name: '重试读取表设计', exact: true }).click(); await page.waitForSelector('.er-te[data-load-state="ready"]');
    await page.evaluate(() => window.graphDialog.close()); assert.deepEqual(errors, []);
    t.diagnostic('真实 Chrome、关系图、右侧编辑器与主题 CSS；显式 mock IPC，不连接真实数据库。');
  } finally { await browser?.close(); if (server.listening) await new Promise(resolve => server.close(resolve)); }
});
