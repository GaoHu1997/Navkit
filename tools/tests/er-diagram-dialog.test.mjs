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

const root = fileURLToPath(new URL('../../', import.meta.url));
const plugins = [{ name: 'isolate-independent-editor', setup(b) {
  b.onResolve({ filter: /(?:^|\/)ErTableEditor$/ }, () => ({ path: 'editor', namespace: 'fixture' }));
  b.onLoad({ filter: /^editor$/, namespace: 'fixture' }, () => ({ loader: 'js', contents: `
    export class ErTableEditor {
      constructor(opts, host) { this.opts = opts; this.host = host; this.el = document.createElement('div'); window.editors.push(this); }
      activate() {} destroy() { this.disposed = true; } draftDirty() { return false; }
      async save() { await window.saveEditor(this); }
    }` }));
  b.onResolve({ filter: /^\.\/SyncDialog$/ }, () => ({ path: 'a11y', namespace: 'fixture' }));
  b.onLoad({ filter: /^a11y$/, namespace: 'fixture' }, () => ({ loader: 'js', contents: `export function accessibleDialog(dlg) { dlg.el.setAttribute('role', 'dialog'); }` }));
} }];
const result = await build({ stdin: { contents: `export { openErDiagramDialog } from './src/dialogs/ErDiagramDialog'; export * from './src/dialogs/erDiagramRendering';`, resolveDir: root, loader: 'ts' },
  bundle: true, write: false, format: 'cjs', platform: 'node', loader: { '.css': 'empty' }, plugins, logLevel: 'silent', metafile: true });
assert.ok(Object.keys(result.metafile.inputs).some(p => p.endsWith('src/core/api.ts')));
assert.ok(Object.keys(result.metafile.inputs).some(p => p.endsWith('src/core/erDiagramMetadata.ts')));

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
    } };
  const localStorage = { getItem: k => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v), removeItem: k => storage.delete(k) };
  window.localStorage = localStorage;
  const module = { exports: {} };
  const context = vm.createContext({ module, exports: module.exports, window, document, localStorage, HTMLElement: Element,
    ResizeObserver: class { constructor(cb) { this.cb = cb; observers.push(this); } observe(el) { this.el = el; this.cb([{ contentRect: { width: 1000, height: 700 } }]); } disconnect() { this.disconnected = true; } },
    navigator: { clipboard: { writeText: async () => {} } }, console, URL, Error, setTimeout: f => { const id = ++seq; timers.set(id, f); return id; }, clearTimeout: id => timers.delete(id) });
  vm.runInContext(result.outputFiles[0].text, context);
  const h = { module: module.exports, document, window, calls, frames, timers, listeners, observers, storage,
    emit(n, e) { for (const f of listeners.get(n) ?? []) f(e); },
    async settle() { for (let i = 0; i < 30; i++) await Promise.resolve(); },
    frame() { const fs = [...frames.values()]; frames.clear(); fs.forEach(f => f()); },
    async flush() { await this.settle(); this.frame(); await this.settle(); },
    open(id = 'a', db = 'db') { return this.module.openErDiagramDialog([connection('a'), connection('b')], connection(id), db); },
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

test('真实Chrome+原CSS：卡片高度、拖动徽标、缓存force、1000表裁剪及工程视口', { timeout: 45000 }, async t => {
  const browserBundle = await build({ stdin: { resolveDir: root, loader: 'ts', contents: `import { openErDiagramDialog } from './src/dialogs/ErDiagramDialog'; window.openGraph = () => window.graphDialog = openErDiagramDialog(window.graphConnections, window.graphConnections[0], 'db');` },
    bundle: true, write: false, format: 'iife', platform: 'browser', loader: { '.css': 'empty' }, plugins, logLevel: 'silent' });
  const css = (await readFile(path.join(root, 'src/styles/base.css'), 'utf8')) + '\n' + (await readFile(path.join(root, 'src/styles/er-diagram.css'), 'utf8'));
  const install = `window.graphConnections=${JSON.stringify([connection('a')])}; window.editors=[]; window.saveEditor=async()=>{}; window.graphCalls=0; window.graphData=${JSON.stringify(relatedFixture())}; window.__PREVIEW_MOCK__={list_databases:()=>[['db','mysql']], get_diagram_metadata:()=>{window.graphCalls++; return structuredClone(window.graphData);}};`;
  const server = createServer((req, res) => { if (req.url === '/css') { res.setHeader('Content-Type', 'text/css'); res.end(css); } else if (req.url === '/js') { res.setHeader('Content-Type', 'text/javascript'); res.end(browserBundle.outputFiles[0].text); } else { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(`<!doctype html><meta charset="utf-8"><title>ER fixture: mock metadata; not native database acceptance</title><link rel="stylesheet" href="/css"><body><script>${install}</script><script src="/js"></script></body>`); } });
  let browser;
  try {
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
    const executablePath = [process.env.NAVKIT_TEST_BROWSER, process.env.BROWSER_EXE, 'C:/Program Files/Google/Chrome/Application/chrome.exe', 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', 'C:/Program Files/Microsoft/Edge/Application/msedge.exe', '/usr/bin/chromium', '/usr/bin/chromium-browser'].filter(Boolean).find(existsSync);
    browser = await chromium.launch({ headless: true, ...(executablePath ? { executablePath } : {}) });
    const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } }); const errors = []; page.on('pageerror', e => errors.push(e.message)); page.setDefaultTimeout(5000);
    await page.goto(`http://127.0.0.1:${server.address().port}/`); await page.evaluate(() => window.openGraph()); await page.waitForFunction(() => document.querySelectorAll('.er-card').length === 4);
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
