// 执行真实历史窗口、Dialog、可访问辅助函数及 CodeMirror 状态；只替换编辑器的浏览器布局层。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { webcrypto } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import ts from 'typescript';
import { transform } from 'esbuild';
import * as cmState from '@codemirror/state';
import * as cmView from '@codemirror/view';
import * as cmLanguage from '@codemirror/language';
import * as cmSql from '@codemirror/lang-sql';
import * as lezerHighlight from '@lezer/highlight';

const files = ['src/dialogs/HistoryLogDialog.ts', 'src/dialogs/toolDialog.ts', 'src/ui/Dialog.ts', 'src/core/log-format.ts', 'src/core/execution.ts', 'src/core/sql-kind.ts'];
const sources = new Map(await Promise.all(files.map(async file => [file, await readFile(new URL('../../' + file, import.meta.url), 'utf8')])));
const plain = value => JSON.parse(JSON.stringify(value));
class Element {
  constructor(tag = 'div') {
    this.tagName = tag.toUpperCase(); this.children = []; this.parentNode = null;
    this.style = {}; this.dataset = {}; this.attributes = new Map(); this.listeners = new Map();
    this.className = ''; this.id = ''; this._value = ''; this.checked = false;
    this.disabled = false; this.hidden = false; this.tabIndex = -1; this.scrollTop = 0;
    this._text = ''; this._html = '';
    this.classList = {
      contains: name => this.className.split(/\s+/).includes(name),
      add: (...names) => { this.className = [...new Set([...this.className.split(/\s+/).filter(Boolean), ...names])].join(' '); },
      remove: (...names) => { this.className = this.className.split(/\s+/).filter(name => !names.includes(name)).join(' '); },
      toggle: (name, force) => { const yes = force ?? !this.classList.contains(name); yes ? this.classList.add(name) : this.classList.remove(name); return yes; },
    };
  }
  get value() {
    if (this.tagName !== 'SELECT') return this._value;
    return this.children.find(option => option.selected)?.value ?? (this.explicitSelection ? '' : this.children[0]?.value ?? '');
  }
  set value(value) {
    this._value = String(value);
    if (this.tagName === 'SELECT') {
      this.explicitSelection = true;
      for (const option of this.children) option.selected = option.value === this._value;
    }
  }
  set textContent(value) { this._text = String(value); this._html = ''; this.children = []; }
  get textContent() { return this._text + this._html.replace(/<[^>]*>/g, '') + this.children.map(child => child.textContent).join(''); }
  set innerHTML(value) { this._html = String(value); this._text = ''; this.children = []; }
  get innerHTML() { return this._html; }
  appendChild(child) { child.remove(); this.children.push(child); child.parentNode = this; return child; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  insertBefore(child, before) {
    child.remove(); const index = this.children.indexOf(before);
    this.children.splice(index < 0 ? this.children.length : index, 0, child); child.parentNode = this;
  }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); this.parentNode = null; }
  get firstChild() { return this.children[0] ?? null; }
  get isConnected() { return this === this.ownerDocument?.body || Boolean(this.parentNode?.isConnected); }
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  getAttribute(name) {
    if (this.attributes.has(name)) return this.attributes.get(name);
    if (name.startsWith('data-')) return this.dataset[name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] ?? null;
    if (name === 'disabled' || name === 'hidden') return this[name] ? '' : null;
    if (name === 'id') return this.id || null;
    if (name === 'tabindex') return String(this.tabIndex);
    if (name === 'type') return this.type ?? null;
    return null;
  }
  removeAttribute(name) { this.attributes.delete(name); }
  addEventListener(name, handler) {
    const handlers = this.listeners.get(name) ?? []; handlers.push(handler); this.listeners.set(name, handlers);
  }
  async emit(name, detail = {}) {
    const event = { target: this, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...detail };
    for (const handler of this.listeners.get(name) ?? []) await handler(event);
    return event;
  }
  getClientRects() {
    for (let node = this; node; node = node.parentNode) if (node.hidden || node.style.display === 'none') return [];
    return this.isConnected ? [{}] : [];
  }
  focus() { if (this.getClientRects().length) this.ownerDocument.activeElement = this; }
  matches(selector) {
    return selector.split(',').some(part => {
      let candidate = part.trim();
      const excluded = [...candidate.matchAll(/:not\(([^)]+)\)/g)].map(match => match[1]);
      if (excluded.some(item => this.matches(item))) return false;
      candidate = candidate.replace(/:not\([^)]+\)/g, '');
      if (candidate.includes(':disabled') && !this.disabled) return false;
      candidate = candidate.replace(/:disabled/g, '');
      const tag = candidate.match(/^[\w-]+/); if (tag && this.tagName !== tag[0].toUpperCase()) return false;
      if ([...candidate.matchAll(/\.([\w-]+)/g)].some(match => !this.classList.contains(match[1]))) return false;
      const id = candidate.match(/#([\w-]+)/); if (id && this.id !== id[1]) return false;
      for (const match of candidate.matchAll(/\[([\w-]+)(?:=['"]?([^'"\]]+)['"]?)?\]/g)) {
        const value = this.getAttribute(match[1]);
        if (value === null || match[2] !== undefined && value !== match[2]) return false;
      }
      return true;
    });
  }
  querySelectorAll(selector) {
    const result = [];
    const visit = element => { for (const child of element.children) { if (child.matches(selector)) result.push(child); visit(child); } };
    visit(this); return result;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}
// 延用设置窗口的原生节点模拟，补足历史窗口的节点与捕获阶段行为。
Element.prototype.replaceChildren = function (...children) { for (const child of this.children) child.parentNode = null; this.children = []; this._text = ''; this.append(...children); };
Element.prototype.prepend = function (...children) { for (const child of children.reverse()) this.insertBefore(child, this.firstChild); };
Element.prototype.contains = function (node) { return this === node || this.children.some(child => child.contains(node)); };
Object.defineProperty(Element.prototype, 'parentElement', { get() { return this.parentNode; } });
Object.defineProperty(Element.prototype, 'lastElementChild', { get() { return this.children.at(-1) ?? null; } });
Element.prototype.addEventListener = function (name, handler, capture = false) { const handlers = this.listeners.get(name) ?? []; handlers.push({ handler, capture }); this.listeners.set(name, handlers); };
Element.prototype.removeEventListener = function (name, handler) { this.listeners.set(name, (this.listeners.get(name) ?? []).filter(item => item.handler !== handler)); };
Element.prototype.emit = async function (name, detail = {}) {
  const event = { target: this, defaultPrevented: false, stopped: false, preventDefault() { this.defaultPrevented = true; }, stopImmediatePropagation() { this.stopped = true; }, ...detail };
  const handlers = [...this.listeners.get(name) ?? []].sort((a, b) => Number(b.capture) - Number(a.capture));
  for (const { handler } of handlers) { await handler(event); if (event.stopped) break; }
  return event;
};
Element.prototype.focus = function () { for (let node = this; node; node = node.parentNode) if (node.inert) return; if (!this.disabled && this.getClientRects().length) this.ownerDocument.activeElement = this; };

function harness(clipboard = async () => {}) {
  const document = new Element('document');
  document.createElement = tag => { const element = new Element(tag); element.ownerDocument = document; return element; };
  document.body = document.createElement('body'); document.activeElement = document.body;
  const microtasks = [], editors = [], cache = new Map();
  class EditorView {
    static editable = cmView.EditorView.editable;
    static contentAttributes = cmView.EditorView.contentAttributes;
    static decorations = cmView.EditorView.decorations;
    static lineWrapping = cmView.EditorView.lineWrapping;
    constructor({ state, parent }) {
      this.dom = document.createElement('div'); this.dom.className = 'cm-editor';
      this.contentDOM = document.createElement('div'); this.contentDOM.className = 'cm-content'; this.contentDOM.tabIndex = 0;
      this.dom.append(this.contentDOM); parent.append(this.dom); this.setState(state); editors.push(this);
    }
    setState(state) { assert.ok(!this.destroyed, '关闭后不能更新已销毁编辑器'); this.state = state; this.contentDOM.textContent = state.doc.toString(); this.updates = (this.updates ?? 0) + 1; }
    destroy() { this.destroyed = true; this.destroyCalls = (this.destroyCalls ?? 0) + 1; }
  }
  const mocks = {
    '@codemirror/state': cmState, '@codemirror/view': { ...cmView, EditorView },
    '@codemirror/language': cmLanguage, '@codemirror/lang-sql': cmSql, '@lezer/highlight': lezerHighlight,
    '../core/api': { api: { listSqlLog: async () => [], clearSqlLog: async () => {} } },
    '../ui/icons': { ICON_CROSS: {}, ICON_COPY: {}, ICON_REFRESH: {}, svg: () => '<svg aria-hidden="true"></svg>' },
    '../styles/history-log-dialog.css': {},
  };
  const globals = { document, HTMLElement: Element, Node: Element, crypto: webcrypto, Error, queueMicrotask: callback => microtasks.push(callback), navigator: { clipboard: { writeText: clipboard } }, window: { innerWidth: 1280, innerHeight: 900, addEventListener() {}, removeEventListener() {} } };
  function load(file) {
    if (cache.has(file)) return cache.get(file);
    const exports = {}; cache.set(file, exports);
    const require = name => {
      if (name in mocks) return mocks[name];
      if (name === './icons') return mocks['../ui/icons'];
      if (name === '../ui/Dialog') return load('src/ui/Dialog.ts');
      if (name === './toolDialog') return load('src/dialogs/toolDialog.ts');
      if (name === '../core/log-format') return load('src/core/log-format.ts');
      if (name === '../core/sql-kind') return load('src/core/sql-kind.ts');
      if (name === '../core/execution') return load('src/core/execution.ts');
      throw new Error('未配置测试依赖：' + name);
    };
    vm.runInNewContext(ts.transpile(sources.get(file), { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS }), { ...globals, exports, require }, { filename: file });
    return exports;
  }
  const settle = async () => { while (microtasks.length) microtasks.shift()(); for (let turn = 0; turn < 8; turn++) await Promise.resolve(); };
  return { ...load(files[0]), document, editors, settle };
}
const entry = (overrides = {}) => ({ ts: '2026-11-08 12:41:09', connection_id: 'c1', connection_name: '本地连接', kind: 'mysql', database: '业务库', sql: 'SELECT 1;', ok: true, elapsed_ms: 1234, rows: 1, message: '', origin: 'query', ...overrides });
function fixture(records = [entry()], overrides = {}) {
  const calls = [];
  return { calls, source: { async listSqlLog(limit) { calls.push(['list', limit]); return records; }, async clearSqlLog() { calls.push(['clear']); }, ...overrides } };
}
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const control = (dialog, name) => dialog.el.querySelector('.history-log-' + name);
const status = dialog => control(dialog, 'status').textContent;
const dialogFor = h => h.document.body.querySelector('[role="alertdialog"]');
const change = async (dialog, name, value) => { const element = control(dialog, name); element.value = value; await element.emit(['outcome', 'connection', 'database', 'kind'].includes(name) ? 'change' : 'input'); };
const openConfirmation = async (h, dialog) => { await control(dialog, 'more').emit('click'); await control(dialog, 'clear').emit('click'); await h.settle(); return dialogFor(h); };
const confirmButton = confirmation => confirmation.querySelectorAll('button').find(button => button.textContent === '清空 SQL 日志');

test('SQL 原文保留 CRLF、缩进、空行、特殊字符和超过摘要长度的文本', () => {
  const h = harness();
  const sql = '\tSELECT "<script>&中文\\路径</script>"\r\n  FROM users\n\nWHERE name = \'a&b\';' + ' x'.repeat(15000);
  const record = entry({ sql }); const row = h.sqlLogRows([record])[0];
  assert.equal(row.detail.sql, sql); assert.equal(row.message, sql);
  assert.equal(row.raw, '[2026-11-08 12:41:09.000][本地连接][业务库][MYSQL]\n' + sql + '\nTime: 1.234s');
  assert.equal(record.sql, sql); assert.ok(row.raw.includes(' x'.repeat(15000)));
});

test('连续正文按元信息、SQL、Time、错误与空行排列，CRLF 不错位', () => {
  const h = harness();
  const rows = h.sqlLogRows([entry({ sql: 'SELECT 1,\r\n  2;\r\n' }), entry({ ok: false, rows: -1, elapsed_ms: 0, message: '<img src=x>\n  错误详情' })]);
  const result = h.historyLogDocument(rows);
  assert.equal(result.text, rows.map(row => row.raw).join('\n\n').replace(/\r\n?/g, '\n'));
  assert.deepEqual(plain(result.annotations), [{ line: 1, kind: 'meta' }, { line: 5, kind: 'time' }, { line: 7, kind: 'meta' }, { line: 9, kind: 'time' }, { line: 10, kind: 'error' }, { line: 11, kind: 'error' }]);
  assert.match(result.text, /Time: 0\.000s\nError: <img src=x>\n  错误详情$/);
});

test('结果筛选与全文搜索覆盖时间、连接、数据库、SQL 尾部、错误和执行来源', () => {
  const h = harness(); const rows = h.sqlLogRows([entry({ sql: 'SELECT ' + 'a'.repeat(500) + 'TailNeedle' }), entry({ connection_name: '远程连接', ok: false, message: 'Permission DENIED', origin: 'monitor' })]);
  for (const text of ['2026-11-08', '本地连接', '业务库', ' tailneedle ']) assert.ok(h.filterHistoryLog(rows, text).some(row => row.outcome === 'success'));
  for (const text of ['远程连接', 'permission denied', 'monitor']) assert.equal(h.filterHistoryLog(rows, text, 'all', 'error').length, 1);
  assert.equal(h.filterHistoryLog(rows, 'denied', 'all', 'success').length, 0);
  assert.equal(h.filterHistoryLog(rows, '', 'all', 'success').length, 1);
  assert.equal(h.filterHistoryLog(rows, '', 'SQL', 'error').length, 1);
});

test('窗口只请求 SQL 来源并限制 2000 条，不生成来源选择器、页签、表格或详情', async () => {
  const h = harness(), f = fixture([entry()], { readLog() { assert.fail('不能读取运行日志'); }, listJobHistory() { assert.fail('不能读取任务历史'); } });
  const dialog = h.openHistoryLogDialog(f.source); await h.settle();
  assert.deepEqual(f.calls, [['list', 2000]]); assert.equal(dialog.el.querySelectorAll('select').length, 4);
  assert.equal(dialog.el.querySelectorAll('table,[role="tab"],[role="tablist"]').length, 0);
  assert.doesNotMatch(dialog.el.textContent, /运行日志|自动运行历史|日志来源/);
  const editor = h.editors[0]; assert.equal(editor.state.readOnly, true); assert.equal(editor.state.facet(cmView.EditorView.editable), false);
  assert.match(editor.state.doc.toString(), /SELECT 1;\nTime: 1\.234s/);
  const attributes = Object.assign({}, ...editor.state.facet(cmView.EditorView.contentAttributes));
  assert.equal(attributes['aria-readonly'], 'true'); assert.equal(attributes.tabindex, '0');
  for (const element of dialog.el.querySelectorAll('input,select')) assert.ok(dialog.el.querySelectorAll('label').some(label => label.htmlFor === element.id));
  assert.match(control(dialog, 'note').textContent, /2000 条 · 30 天/); assert.match(control(dialog, 'note').textContent, /截断.*无法恢复/);
});

test('数据源、数据库与 SQL 类型筛选串行生效，并可组合结果筛选', async () => {
  const h = harness();
  const records = [
    entry({ sql: 'SELECT * FROM a' }),
    entry({ sql: 'UPDATE a SET x = 1', database: '另一个库', connection_id: 'c2', connection_name: '远程连接' }),
    entry({ sql: 'DROP TABLE a', database: '另一个库', connection_id: 'c2', connection_name: '远程连接' }),
  ];
  const dialog = h.openHistoryLogDialog(fixture(records).source, [{ id: 'c1', name: '本地连接', kind: 'mysql' }, { id: 'c2', name: '远程连接', kind: 'postgres' }]);
  await h.settle();
  assert.match(control(dialog, 'count').textContent, /3 \/ 3 条/);
  await change(dialog, 'connection', 'c2');
  assert.match(control(dialog, 'count').textContent, /2 \/ 3 条/);
  await change(dialog, 'database', '另一个库');
  await change(dialog, 'kind', 'update');
  assert.match(control(dialog, 'count').textContent, /1 \/ 3 条/);
  assert.match(h.editors[0].state.doc.toString(), /UPDATE a SET x = 1/);
  await change(dialog, 'kind', 'ddl');
  assert.match(h.editors[0].state.doc.toString(), /DROP TABLE a/);
  await change(dialog, 'kind', 'all'); await change(dialog, 'database', 'all'); await change(dialog, 'connection', 'all');
  assert.match(control(dialog, 'count').textContent, /3 \/ 3 条/);
});

test('WITH 与多语句按顶层动词归类，筛选能命中删除与结构变更', async () => {
  const h = harness();
  const rows = h.sqlLogRows([
    entry({ sql: 'WITH c AS (SELECT 1) DELETE FROM t' }),
    entry({ sql: "SELECT 'DELETE FROM x'; UPDATE t SET a = 1" }),
    entry({ sql: 'WITH recent AS (SELECT id FROM t) SELECT * FROM recent' }),
  ]);
  assert.deepEqual(Array.from(rows[0].sqlKinds), ['delete']);
  assert.deepEqual(Array.from(rows[1].sqlKinds), ['query', 'update']);
  assert.deepEqual(Array.from(rows[2].sqlKinds), ['query']);
  assert.equal(h.filterHistoryLog(rows, '', 'all', 'all', { kind: 'delete' }).length, 1);
  assert.equal(h.filterHistoryLog(rows, '', 'all', 'all', { kind: 'update' }).length, 1);
  const quoted = h.sqlLogRows([entry({ sql: "SELECT 'a;b'; UPDATE t SET x = 1" })]);
  assert.deepEqual(Array.from(quoted[0].sqlKinds), ['query', 'update']);
});

test('真实库名恰好为 all 时仍能单独筛选，不与「全部数据库」混淆', async () => {
  const h = harness();
  const dialog = h.openHistoryLogDialog(fixture([
    entry({ sql: 'SELECT 1', database: 'all' }),
    entry({ sql: 'SELECT 2', database: '业务库' }),
  ]).source);
  await h.settle();
  const option = [...control(dialog, 'database').children].find(o => o.textContent === 'all');
  assert.ok(option); assert.notEqual(option.value, 'all');
  await change(dialog, 'database', option.value);
  assert.match(control(dialog, 'count').textContent, /1 \/ 2 条/);
  assert.match(h.editors[0].state.doc.toString(), /SELECT 1/);
});

test('无记录、无匹配都有就地提示，正文原文不进入 innerHTML', async () => {
  let h = harness(), dialog = h.openHistoryLogDialog(fixture([]).source); await h.settle();
  assert.match(control(dialog, 'empty').textContent, /暂无 SQL/);
  h = harness();
  dialog = h.openHistoryLogDialog(fixture([entry({ sql: '<script>alert(1)</script>', ok: false, message: '<img onerror=x>' })]).source); await h.settle();
  await change(dialog, 'search', 'does-not-exist'); assert.match(control(dialog, 'empty').textContent, /没有符合/);
  for (const node of [dialog.el, ...dialog.el.querySelectorAll('div,span,p')]) assert.doesNotMatch(node.innerHTML, /alert\(1\)|onerror=x/);
});

test('读取失败保留真实失败状态，刷新恢复；后续失败保留上次数据', async () => {
  const h = harness(); let fail = true;
  const dialog = h.openHistoryLogDialog(fixture([], { async listSqlLog() { if (fail) throw new Error('磁盘读取失败'); return [entry()]; } }).source); await h.settle();
  assert.match(status(dialog), /磁盘读取失败/); assert.match(control(dialog, 'empty').textContent, /读取失败/);
  fail = false; await control(dialog, 'refresh').emit('click'); await h.settle(); assert.match(h.editors[0].state.doc.toString(), /SELECT 1/);
  fail = true; await control(dialog, 'refresh').emit('click'); await h.settle();
  assert.match(status(dialog), /读取失败.*保留上次/); assert.match(h.editors[0].state.doc.toString(), /SELECT 1/); assert.equal(control(dialog, 'editor').hidden, false);
});

test('刷新不重叠，窗口关闭后迟到响应不写正文，清理监听并恢复焦点', async () => {
  const h = harness(), gate = deferred(); let calls = 0;
  const trigger = h.document.createElement('button'); h.document.body.append(trigger); trigger.focus();
  const dialog = h.openHistoryLogDialog(fixture([], { listSqlLog() { calls++; return gate.promise; } }).source); await h.settle();
  await control(dialog, 'refresh').emit('click'); await control(dialog, 'refresh').emit('click'); assert.equal(calls, 1);
  assert.equal(control(dialog, 'viewer').getAttribute('aria-busy'), 'true'); assert.match(status(dialog), /正在读取/);
  dialog.close(); const updates = h.editors[0].updates, previous = status(dialog);
  gate.resolve([entry({ sql: 'LATE SQL' })]); await h.settle();
  assert.equal(h.editors[0].updates, updates); assert.equal(h.editors[0].destroyCalls, 1); assert.equal(status(dialog), previous);
  assert.equal(h.document.activeElement, trigger); assert.equal(h.document.listeners.get('pointerdown').length, 0);
});

test('更多菜单的 Escape 先收起菜单；确认是 alertdialog，取消默认焦点、Tab 陷阱和回焦', async () => {
  const h = harness(), f = fixture(), dialog = h.openHistoryLogDialog(f.source); await h.settle();
  await control(dialog, 'more').emit('click'); assert.equal(control(dialog, 'more-menu').hidden, false);
  await dialog.el.emit('keydown', { key: 'Escape' }); assert.equal(dialog.el.isConnected, true); assert.equal(control(dialog, 'more-menu').hidden, true);
  const confirmation = await openConfirmation(h, dialog);
  assert.equal(confirmation.getAttribute('aria-modal'), 'true'); assert.ok(confirmation.getAttribute('aria-describedby'));
  assert.equal(dialog.el.inert, true); assert.equal(h.document.activeElement.textContent, '取消');
  const close = confirmation.querySelector('.dialog-title-close'), last = confirmation.querySelectorAll('button').at(-1);
  close.focus(); await confirmation.emit('keydown', { key: 'Tab', shiftKey: true }); assert.equal(h.document.activeElement, last);
  await confirmation.emit('keydown', { key: 'Tab' }); assert.equal(h.document.activeElement, close);
  await confirmation.emit('keydown', { key: 'Escape' }); assert.equal(confirmation.isConnected, false); assert.equal(dialog.el.inert, false);
  assert.equal(h.document.activeElement, control(dialog, 'more')); assert.deepEqual(f.calls, [['list', 2000]]);
});

test('清空失败不能伪装成功，确认框与正文提示错误，重试成功才清除', async () => {
  const h = harness(); let fail = true, clears = 0;
  const dialog = h.openHistoryLogDialog(fixture([entry()], { async clearSqlLog() { clears++; if (fail) throw new Error('删除被拒绝'); } }).source); await h.settle();
  const confirmation = await openConfirmation(h, dialog); await confirmButton(confirmation).emit('click'); await h.settle();
  assert.equal(confirmation.isConnected, true); assert.match(status(dialog), /清空 SQL 日志失败.*删除被拒绝/);
  assert.equal(confirmation.querySelector('.history-log-confirm-status').getAttribute('role'), 'alert');
  assert.match(h.editors[0].state.doc.toString(), /SELECT 1/);
  fail = false; await confirmButton(confirmation).emit('click'); await h.settle();
  assert.equal(clears, 2); assert.equal(confirmation.isConnected, false); assert.equal(h.editors[0].state.doc.toString(), '');
  assert.match(status(dialog), /已清空/); assert.equal(control(dialog, 'refresh').disabled, false);
});

test('确认和清空期间刷新被串行化，重复清空只有一次；Escape 后仍等真实操作完成', async () => {
  const h = harness(), gate = deferred(); let lists = 0, clears = 0;
  const dialog = h.openHistoryLogDialog(fixture([entry()], { async listSqlLog() { lists++; return [entry()]; }, clearSqlLog() { clears++; return gate.promise; } }).source); await h.settle();
  const confirmation = await openConfirmation(h, dialog); await control(dialog, 'refresh').emit('click'); assert.equal(lists, 1);
  const pending = confirmButton(confirmation).emit('click'); await h.settle();
  await confirmButton(confirmation).emit('click'); await control(dialog, 'refresh').emit('click'); assert.equal(clears, 1); assert.equal(lists, 1);
  await confirmation.emit('keydown', { key: 'Escape' }); assert.equal(h.document.activeElement, control(dialog, 'more'));
  assert.equal(control(dialog, 'refresh').disabled, true); assert.match(h.editors[0].state.doc.toString(), /SELECT 1/);
  await control(dialog, 'refresh').emit('click'); assert.equal(lists, 1);
  gate.resolve(); await pending; await h.settle(); assert.match(status(dialog), /已清空/);
  await control(dialog, 'refresh').emit('click'); await h.settle(); assert.equal(lists, 2);
});

test('清空期间关闭整个窗口会关闭确认框，迟到清空不能改写关闭后的状态', async () => {
  const h = harness(), gate = deferred(), dialog = h.openHistoryLogDialog(fixture([entry()], { clearSqlLog: () => gate.promise }).source); await h.settle();
  const confirmation = await openConfirmation(h, dialog), pending = confirmButton(confirmation).emit('click'); await h.settle();
  dialog.close(); const previous = status(dialog), updates = h.editors[0].updates; assert.equal(confirmation.isConnected, false);
  gate.resolve(); await pending; await h.settle(); assert.equal(status(dialog), previous); assert.equal(h.editors[0].updates, updates);
});


test('即使注入源返回超范围记录，也只显示 2000 条；原记录与原文没有被更改', async () => {
  const h = harness(), records = Array.from({ length: 2001 }, (_, index) => entry({ sql: 'SELECT ' + index }));
  const dialog = h.openHistoryLogDialog(fixture(records).source); await h.settle();
  assert.match(control(dialog, 'count').textContent, /2000 \/ 2000 条/);
  assert.ok(h.editors[0].state.doc.toString().includes('SELECT 1999')); assert.ok(!h.editors[0].state.doc.toString().includes('SELECT 2000'));
  assert.equal(records.length, 2001); assert.equal(records[2000].sql, 'SELECT 2000');
});

test('局部样式可解析、令牌存在，640 高上限、13px 等宽/20px 行高且无动画', async () => {
  const css = await readFile(new URL('../../src/styles/history-log-dialog.css', import.meta.url), 'utf8');
  const base = await readFile(new URL('../../src/styles/base.css', import.meta.url), 'utf8');
  assert.deepEqual((await transform(css, { loader: 'css', logLevel: 'silent' })).warnings, []);
  const tokens = new Set([...base.matchAll(/(--[\w-]+)\s*:/g)].map(match => match[1]));
  for (const match of css.matchAll(/var\((--[\w-]+)/g)) assert.ok(tokens.has(match[1]), match[1]);
  for (const match of css.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{/g)) assert.ok(match[1].trim().startsWith('.dialog.history-log-dialog') || match[1].trim().startsWith('@media'), match[1]);
  assert.doesNotMatch(css, /#[0-9a-f]{3,8}\b|@keyframes|transition\s*:/i);
  assert.match(css, /max-height:\s*640px/); assert.match(css, /font-size:\s*var\(--fs-13\)/); assert.match(css, /line-height:\s*20px/);
  assert.match(css, /font-family:\s*var\(--font-mono\)/); assert.match(css, /overflow-wrap:\s*anywhere/); assert.match(css, /min-height:\s*0/);
});

// 可选真实浏览器验证：PowerShell 中设置 $env:HISTORY_BROWSER='1' 后运行同一测试文件。
test('真实浏览器：固定样例、只读原文、软换行、确认焦点、小视口和 2000 条虚拟渲染', { skip: process.env.HISTORY_BROWSER !== '1' }, async t => {
  const { createServer } = await import('node:http');
  const { build } = await import('esbuild');
  const { chromium } = await import('playwright-core');
  const root = fileURLToPath(new URL('../..', import.meta.url));
  const bundle = await build({ stdin: { contents: 'export { openHistoryLogDialog, sqlLogRows } from "./src/dialogs/HistoryLogDialog.ts"; export { EditorView } from "@codemirror/view";', resolveDir: root, loader: 'ts' }, bundle: true, write: false, format: 'iife', globalName: 'HistoryFixture', loader: { '.css': 'empty' }, logLevel: 'silent' });
  const styles = (await Promise.all(['src/styles/base.css', 'src/styles/history-log-dialog.css'].map(file => readFile(new URL('../../' + file, import.meta.url), 'utf8')))).join('\n');
  const fixtures = await readFile(new URL('./dialog-fixtures.js', import.meta.url), 'utf8');
  const html = '<!doctype html><html lang="zh-CN"><meta charset="utf-8"><style>' + styles + '</style><body><button id="trigger">打开历史</button><script>' + bundle.outputFiles[0].text.replace(/<\/script/gi, '<\\/script') + '\n' + fixtures + '</script></body></html>';
  const server = createServer((_, response) => { response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }); response.end(html); });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  let browser;
  try {
    browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
    const page = await browser.newPage({ viewport: { width: 1400, height: 900 } }); page.setDefaultTimeout(10000);
    const errors = []; page.on('pageerror', error => errors.push(error.message));
    await page.goto('http://127.0.0.1:' + server.address().port);
    await page.evaluate(() => {
      window.__records = window.NAVKIT_DESIGN.records; window.__calls = []; window.__copied = [];
      Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(text) { window.__copied.push(text); } } });
      window.__source = { async listSqlLog(limit) { window.__calls.push(['list', limit]); return window.__records; }, async clearSqlLog() { window.__calls.push(['clear']); window.__records = []; }, readLog() { throw new Error('禁止其他来源'); }, listJobHistory() { throw new Error('禁止其他来源'); } };
      document.querySelector('#trigger').focus(); window.__dialog = HistoryFixture.openHistoryLogDialog(window.__source);
      window.__view = () => HistoryFixture.EditorView.findFromDOM(document.querySelector('.history-log-editor .cm-content'));
    });
    await page.waitForFunction(() => document.querySelector('.history-log-count').textContent.includes('4 / 4'));
    assert.equal(await page.evaluate(() => window.__view().state.doc.lines), 31);
    assert.deepEqual(await page.evaluate(() => window.__calls), [['list', 2000]]);
    const metrics = await page.evaluate(() => {
      const dialog = document.querySelector('.history-log-dialog'), viewer = dialog.querySelector('.history-log-viewer'), scroller = dialog.querySelector('.cm-scroller');
      return { width: dialog.getBoundingClientRect().width, height: viewer.getBoundingClientRect().height, font: getComputedStyle(scroller).fontSize, lineHeight: getComputedStyle(scroller).lineHeight, overflow: scroller.scrollWidth - scroller.clientWidth, keyword: getComputedStyle(dialog.querySelector('.history-log-keyword')).color, readOnly: dialog.querySelector('.cm-content').getAttribute('contenteditable') };
    });
    assert.ok(metrics.width <= 1240); assert.ok(metrics.height <= 640); assert.equal(metrics.font, '13px'); assert.equal(metrics.lineHeight, '20px'); assert.ok(metrics.overflow <= 1); assert.equal(metrics.keyword, 'rgb(43, 98, 217)'); assert.equal(metrics.readOnly, 'false');
    const before = await page.evaluate(() => window.__view().state.doc.toString());
    await page.locator('.cm-content').focus(); await page.keyboard.type('禁止写入'); assert.equal(await page.evaluate(() => window.__view().state.doc.toString()), before);
    await page.locator('.history-log-kind').selectOption('query');
    await page.waitForFunction(() => /3 \/ 4/.test(document.querySelector('.history-log-count').textContent));
    const queryOnly = await page.evaluate(() => window.__view().state.doc.toString());
    assert.ok(queryOnly.includes('SELECT missing_column'));
    assert.ok(queryOnly.includes('SHOW CREATE TABLE'));
    assert.ok(!queryOnly.includes('UPDATE navkit_accept.t_user'));
    await page.locator('.history-log-kind').selectOption('all');
    await page.waitForFunction(() => /4 \/ 4/.test(document.querySelector('.history-log-count').textContent));
    await page.locator('.history-log-more').click(); await page.locator('.history-log-clear').click();
    await page.getByRole('alertdialog', { name: '清空 SQL 日志', exact: true }).waitFor(); assert.equal(await page.evaluate(() => document.activeElement.textContent), '取消');
    await page.keyboard.press('Escape'); assert.equal(await page.getByRole('alertdialog').count(), 0); assert.ok(await page.locator('.history-log-more').evaluate(element => element === document.activeElement));
    await page.setViewportSize({ width: 720, height: 420 });
    const small = await page.locator('.history-log-dialog').evaluate(dialog => ({ width: dialog.getBoundingClientRect().width, height: dialog.getBoundingClientRect().height, viewer: dialog.querySelector('.history-log-viewer').getBoundingClientRect().height, overflow: dialog.scrollWidth - dialog.clientWidth }));
    assert.ok(small.width <= 688); assert.ok(small.height <= 388); assert.ok(small.viewer > 30 && small.viewer < 640); assert.ok(small.overflow <= 1);
    await page.setViewportSize({ width: 1400, height: 900 });
    await page.evaluate(() => { const base = window.__records[0]; window.__records = Array.from({ length: 2000 }, (_, index) => ({ ...base, sql: 'SELECT \'<script>&中文\' AS value,\n  ' + 'x'.repeat(1200) + '\nFROM sample\nWHERE id = ' + index + ';' })); });
    await page.locator('.history-log-refresh').click(); await page.waitForFunction(() => document.querySelector('.history-log-count').textContent.includes('2000 / 2000'));
    const large = await page.evaluate(() => ({ lines: window.__view().state.doc.lines, nodes: document.querySelectorAll('.history-log-viewer .cm-line').length, containsLast: window.__view().state.doc.toString().includes('WHERE id = 1999;'), overflow: document.querySelector('.cm-scroller').scrollWidth - document.querySelector('.cm-scroller').clientWidth, scripts: document.querySelectorAll('.history-log-viewer script').length }));
    assert.ok(large.lines > 10000); assert.ok(large.nodes < 500); assert.equal(large.containsLast, true); assert.ok(large.overflow <= 1); assert.equal(large.scripts, 0);
    await page.locator('.cm-scroller').evaluate(scroller => { scroller.scrollTop = scroller.scrollHeight; });
    await page.waitForFunction(() => document.querySelector('.cm-content').textContent.includes('WHERE id = 1999;'));
    assert.ok(await page.locator('.cm-line').count() < 500); assert.deepEqual(errors, []);
    await page.evaluate(() => window.__dialog.close()); assert.ok(await page.locator('#trigger').evaluate(element => element === document.activeElement));
    t.diagnostic(JSON.stringify({ metrics, small, large }));
  } finally {
    await browser?.close(); await new Promise(resolve => server.close(resolve));
  }
});
