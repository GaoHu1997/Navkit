import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

// Execute the actual TreePanel class with a minimal DOM; no copied drag/drop logic.
const source = fs.readFileSync(new URL('../../src/ui/TreePanel.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source.replace(/^import .*;$/gm, '').replace('export class TreePanel', 'class TreePanel') + '\nglobalThis.TreePanel = TreePanel;', {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None },
}).outputText;

class Element {
  constructor(tag = 'div') {
    this.tagName = tag; this.children = []; this.dataset = {}; this.style = {}; this.attributes = {};
    this.listeners = {}; this.className = ''; this.value = ''; this.disabled = false;
    this.classList = {
      add: (...xs) => { this.className = [...new Set([...this.className.split(' ').filter(Boolean), ...xs])].join(' '); },
      remove: (...xs) => { this.className = this.className.split(' ').filter(x => !xs.includes(x)).join(' '); },
      contains: x => this.className.split(' ').includes(x),
      toggle: (x, force) => { (force ?? !this.classList.contains(x)) ? this.classList.add(x) : this.classList.remove(x); },
    };
  }
  set innerHTML(value) { this.children.forEach(c => { c.parentElement = null; }); this.children = []; this._html = value; }
  get innerHTML() { return this._html ?? ''; }
  get firstChild() { return this.children[0] ?? null; }
  appendChild(child) { child.parentElement = this; this.children.push(child); return child; }
  setAttribute(k, v) { this.attributes[k] = v; }
  removeAttribute(k) { delete this.attributes[k]; }
  addEventListener(name, fn) { (this.listeners[name] ??= []).push(fn); }
  fire(name, props = {}) {
    const event = { target: this, defaultPrevented: false, stopped: false,
      preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, ...props };
    for (const fn of this.listeners[name] ?? []) fn(event);
    return event;
  }
  matches(selector) {
    const classes = [...selector.matchAll(/\.([\w-]+)/g)].map(m => m[1]);
    const tag = selector.match(/^([\w]+)/)?.[1];
    return (!tag || tag === this.tagName) && classes.every(c => this.classList.contains(c));
  }
  closest(selector) { return this.matches(selector) ? this : this.parentElement?.closest(selector) ?? null; }
  contains(element) { return !!element && (this === element || this.children.some(c => c.contains(element))); }
  querySelectorAll(selector) { return this.children.flatMap(c => [...(c.matches(selector) ? [c] : []), ...c.querySelectorAll(selector)]); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  getBoundingClientRect() { return { left: 0, right: 250, top: 30, bottom: 54 }; }
  focus() { this.ownerDocument.activeElement = this; this.fire('focus'); }
  select() {}
}
const plain = value => JSON.parse(JSON.stringify(value));
const settle = async () => { await Promise.resolve(); await Promise.resolve(); };
function harness(callback) {
  const document = { activeElement: null, createElement(tag) { const el = new Element(tag); el.ownerDocument = this; return el; } };
  const context = vm.createContext({ exports: {}, document, Element, svg: () => '', iconSvgByName: () => '', kindIcon: () => 'database', ICON_SEARCH: '', ICON_TREE_ARROW: '' });
  vm.runInContext(compiled, context);
  const moves = [];
  const panel = new context.TreePanel({ onMoveConnection: (...args) => { moves.push(args); return callback?.(...args); } });
  const connections = [{ id: 'a', name: 'A', group: '', kind: 'sqlite' }, { id: 'b', name: 'B', group: '', kind: 'sqlite' },
    { id: 'c', name: 'C', group: 'folder', kind: 'sqlite' }, { id: 'd', name: 'D', group: 'folder', kind: 'sqlite' }];
  const nodes = context.TreePanel.buildGroups(connections, ['folder', 'empty']);
  const folder = nodes.find(n => n.id === 'group:folder'); folder.expanded = true;
  folder.children[0].expanded = true;
  folder.children[0].children = [{ id: 'conn:c/db/main', label: 'main', type: 'database', icon: 'database', expanded: false, expandable: false, children: [] }];
  panel.setNodes(nodes);
  const scroll = panel.el.querySelector('.tree-scroll');
  const row = id => scroll.querySelectorAll('.tree-node').find(el => el.dataset.id === id);
  const transfer = () => ({ types: [], values: {}, setData(k, v) { this.values[k] = v; this.types.push(k); }, getData(k) { return this.values[k] ?? ''; } });
  const start = id => { const dataTransfer = transfer(); row(`conn:${id}`).fire('dragstart', { dataTransfer }); return dataTransfer; };
  const over = (target, dataTransfer, y = 35) => scroll.fire('dragover', { target, dataTransfer, clientY: y });
  const drop = (target, dataTransfer, y = 35) => scroll.fire('drop', { target, dataTransfer, clientY: y });
  return { panel, nodes, connections, moves, row, scroll, document, transfer, start, over, drop };
}

test('双击同一节点只选择一次，展开/收起仍触发，回到树时恢复详情', () => {
  const h = harness(), selected = [], opened = [];
  h.panel.cb.onSelect = node => selected.push(node.id);
  h.panel.cb.onOpen = node => opened.push(node.id);
  const row = h.row('conn:c/db/main');
  row.fire('mousedown', { button: 0 }); row.focus();
  row.fire('mousedown', { button: 0 }); row.fire('dblclick');
  assert.deepEqual(selected, ['conn:c/db/main']); assert.deepEqual(opened, ['conn:c/db/main']);
  row.fire('mousedown', { button: 0 }); row.fire('dblclick');
  assert.equal(selected.length, 1); assert.equal(opened.length, 2);
  h.document.activeElement = new Element('input');
  row.fire('mousedown', { button: 0 }); assert.equal(selected.length, 2);
  row.fire('contextmenu', { clientX: 1, clientY: 1 }); assert.equal(selected.length, 3);
});

test('tree panel renders no operation hint rows', () => {
  const h = harness();
  assert.equal(h.panel.el.querySelector('.tree-root-drop'), null);
  assert.equal(h.panel.el.querySelector('.tree-move-status'), null);
});

test('group folder, label/icon descendants and blank root all move', async () => {
  for (const destination of ['group:folder', 'group:empty', 'blank']) {
    const h = harness(); const dataTransfer = h.start('a');
    const target = destination === 'blank' ? h.scroll : h.row(destination).querySelector('.tree-label');
    assert.equal(h.over(target, dataTransfer).defaultPrevented, true);
    h.drop(target, dataTransfer);
    await settle();
    assert.deepEqual(plain(h.moves), [['a', destination.startsWith('group:') ? destination.slice(6) : '', null, null]]);
    assert.equal(h.scroll.classList.contains('drop-root'), false);
    assert.equal(h.scroll.attributes['aria-busy'], undefined);
  }
});

test('connection upper/lower half has distinct before/after and adopts target group', async () => {
  for (const [targetId, y, position, group] of [['b', 35, 'before', ''], ['b', 50, 'after', ''], ['d', 35, 'before', 'folder'], ['c', 50, 'after', 'folder']]) {
    const h = harness(); const dataTransfer = h.start('a'); const target = h.row(`conn:${targetId}`).querySelector('.tree-icon');
    assert.equal(h.over(target, dataTransfer, y).defaultPrevented, true);
    assert.equal(h.row(`conn:${targetId}`).classList.contains(`drop-${position}`), true);
    h.drop(target, dataTransfer, y); await settle();
    assert.deepEqual(plain(h.moves), [['a', group, targetId, position]]);
    assert.equal(h.row(`conn:${targetId}`).classList.contains(`drop-${position}`), false);
  }
});

test('self, database/subfolders, external MIME, malformed payload and stale source cannot move', () => {
  for (const scenario of ['self', 'database', 'external', 'wrong-payload', 'stale']) {
    const h = harness(); const dataTransfer = scenario === 'external' ? h.transfer() : h.start('a');
    if (scenario === 'external') dataTransfer.setData('application/x-navkit-connection', 'a');
    if (scenario === 'wrong-payload') dataTransfer.values['application/x-navkit-connection'] = 'missing';
    if (scenario === 'stale') h.panel.setNodes(h.nodes.filter(n => n.id !== 'conn:a'));
    const target = scenario === 'self' ? h.row('conn:a') : scenario === 'database' ? h.row('conn:c/db/main') : h.row('group:folder');
    const event = h.drop(target, dataTransfer);
    assert.equal(event.defaultPrevented, true, 'reject defaults such as external file navigation');
    assert.equal(h.moves.length, 0, scenario);
  }
});

test('drag leave/end clear indicators and prevent a later unrelated drop', () => {
  const h = harness(); const dataTransfer = h.start('a');
  h.over(h.row('conn:b'), dataTransfer);
  h.scroll.fire('dragleave', { relatedTarget: null });
  assert.equal(h.row('conn:b').classList.contains('drop-before'), false);
  h.over(h.row('group:folder'), dataTransfer);
  h.row('conn:a').fire('dragend');
  assert.equal(h.row('conn:a').classList.contains('dragging'), false);
  assert.equal(h.row('group:folder').classList.contains('drop-target'), false);
  h.drop(h.scroll, dataTransfer);
  assert.equal(h.moves.length, 0);
});

test('first row is Tab reachable, arrow navigation selects, and focus survives render', () => {
  const h = harness();
  assert.equal(h.row('conn:a').tabIndex, 0);
  h.row('conn:a').focus();
  h.row('conn:a').fire('keydown', { key: 'ArrowDown' });
  assert.equal(h.document.activeElement.dataset.id, 'conn:b');
  assert.equal(h.panel.selectedNodeId, 'conn:b');
  h.panel.render();
  assert.equal(h.document.activeElement, h.row('conn:b'));
  assert.equal(h.row('conn:b').tabIndex, 0);
});

test('Alt arrow same-group ordering and Ctrl X/V cross-group before/after/root are keyboard alternatives', async () => {
  for (const [key, id, expected] of [['ArrowDown', 'a', ['a', '', 'b', 'after']], ['ArrowUp', 'b', ['b', '', 'a', 'before']]]) {
    const h = harness(); const event = h.row(`conn:${id}`).fire('keydown', { key, altKey: true });
    await settle(); assert.equal(event.stopped, true); assert.deepEqual(plain(h.moves[0]), expected);
  }
  for (const [destination, shift, expected] of [['group:empty', false, ['a', 'empty', null, null]], ['conn:c', false, ['a', 'folder', 'c', 'before']], ['conn:d', true, ['a', 'folder', 'd', 'after']]]) {
    const h = harness();
    h.row('conn:a').fire('keydown', { key: 'x', ctrlKey: true });
    assert.equal(h.row('conn:a').classList.contains('move-picked'), true);
    h.row(destination).fire('keydown', { key: 'v', ctrlKey: true, shiftKey: shift });
    await settle(); assert.deepEqual(plain(h.moves[0]), expected);
    assert.equal(h.row('conn:a').classList.contains('move-picked'), false);
  }
  const h = harness(); h.row('conn:a').fire('keydown', { key: 'x', ctrlKey: true });
  h.panel.el.fire('keydown', { key: 'Escape' });
  h.row('group:folder').fire('keydown', { key: 'v', ctrlKey: true });
  assert.equal(h.moves.length, 0);
});

test('pending save blocks duplicate operations; rejection keeps keyboard source retryable', async () => {
  let reject;
  const h = harness(() => new Promise((_, fail) => { reject = fail; }));
  h.row('conn:a').fire('keydown', { key: 'x', ctrlKey: true });
  h.row('conn:b').fire('keydown', { key: 'v', ctrlKey: true });
  assert.equal(h.scroll.attributes['aria-busy'], 'true');
  h.row('group:folder').fire('keydown', { key: 'v', ctrlKey: true });
  assert.equal(h.moves.length, 1);
  reject(new Error('disk read-only')); await settle();
  assert.equal(h.row('conn:a').classList.contains('move-picked'), true);
  assert.equal(h.scroll.attributes['aria-busy'], undefined);
});

test('filtered trees preserve target membership and block ambiguous Alt ordering', async () => {
  const h = harness();
  const search = h.panel.el.querySelector('input'); search.value = 'D'; search.fire('input');
  assert.equal(h.row('conn:d').attributes['aria-level'], '2');
  h.row('conn:d').fire('keydown', { key: 'ArrowUp', altKey: true });
  assert.equal(h.moves.length, 0);
  h.panel.keyboardConnectionId = 'a';
  h.row('conn:d').fire('keydown', { key: 'v', ctrlKey: true, shiftKey: true }); await settle();
  assert.deepEqual(plain(h.moves[0]), ['a', 'folder', 'd', 'after']);
});

test('buildGroups preserves the backend vector order rather than sorting names', () => {
  const h = harness();
  const connections = [h.connections[1], h.connections[0], h.connections[3], h.connections[2]];
  const nodes = h.panel.constructor.buildGroups(connections, ['folder', 'empty']);
  assert.deepEqual(plain(nodes.filter(n => n.type === 'connection').map(n => n.id)), ['conn:b', 'conn:a']);
  assert.deepEqual(plain(nodes.find(n => n.id === 'group:folder').children.map(n => n.id)), ['conn:d', 'conn:c']);
  assert.ok(nodes.find(n => n.id === 'group:empty'));
});

test('IPC forwards optional ordering arguments and legacy moves use explicit nulls', async () => {
  const source = fs.readFileSync(new URL('../../src/core/api.ts', import.meta.url), 'utf8');
  const calls = [];
  const parsed = ts.createSourceFile('api.ts', source, ts.ScriptTarget.ES2021, true);
  const call = parsed.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === 'call');
  const api = parsed.statements.find(node => ts.isVariableStatement(node) && node.declarationList.declarations.some(d => d.name.getText(parsed) === 'api'));
  assert.ok(call && api);
  const stripped = call.getText(parsed) + '\n' + api.getText(parsed).replace(/^export\s+/, '');
  const context = vm.createContext({ window: { __PREVIEW_MOCK__: { move_connection(args) { calls.push(args); return []; } } }, invoke() { throw Error('unexpected real IPC'); } });
  vm.runInContext(ts.transpileModule(stripped + '\nglobalThis.api = api;', { compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None } }).outputText, context);
  await context.api.moveConnection('a', 'folder', 'c', 'before');
  await context.api.moveConnection('b', '');
  assert.deepEqual(plain(calls), [{ id: 'a', group: 'folder', targetId: 'c', position: 'before' }, { id: 'b', group: '', targetId: null, position: null }]);
});

test('App move/refresh preserve expanded loaded children, open pools/databases and selected connection', async () => {
  const h = harness();
  const source = fs.readFileSync(new URL('../../src/core/App.ts', import.meta.url), 'utf8');
  const parsed = ts.createSourceFile('App.ts', source, ts.ScriptTarget.ES2021, true);
  const appClass = parsed.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === 'App');
  // refreshTree 现在会调用 isRefreshableContainer / reloadExpandedContainers，需一并取出。
  const methods = ['moveConnectionToGroup', 'refreshTree', 'syncConnIcons', 'isRefreshableContainer', 'reloadExpandedContainers'].map(name => appClass.members.find(node => ts.isMethodDeclaration(node) && node.name.getText(parsed) === name).getText(parsed));
  const requests = []; const responses = [];
  const api = { async moveConnection(...args) { requests.push(args); const response = responses.shift(); if (response instanceof Error) throw response; return response; } };
  const context = vm.createContext({ api, TreePanel: h.panel.constructor, DB_META: { sqlite: { icon: 'sqlite' } } });
  vm.runInContext(ts.transpileModule(`class TreeApp { ${methods.join('\n')} }\nglobalThis.TreeApp = TreeApp;`, { compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None } }).outputText, context);
  const app = new context.TreeApp();
  const notices = [];
  const openConnections = new Set(['c']); const openDbs = new Set(['c/main']);
  Object.assign(app, { connections: h.connections, groups: ['folder', 'empty'], treeNodes: h.nodes, tree: h.panel, openConnections, openDbs, refreshQueryGroups() {}, toast(...args) { notices.push(args); } });
  let operation;
  h.panel.cb.onMoveConnection = (...args) => operation = app.moveConnectionToGroup(...args);
  const oldConnection = h.panel.find('conn:c'); oldConnection.loaded = true;
  const oldChildren = oldConnection.children;
  const next = [h.connections[0], { ...h.connections[2], group: '' }, h.connections[1], h.connections[3]];
  responses.push(next);
  const dataTransfer = h.start('c'); h.drop(h.row('conn:b'), dataTransfer, 35); await operation; await settle();
  assert.deepEqual(plain(requests), [['c', '', 'b', 'before']]);
  assert.equal(app.connections, next);
  const moved = app.tree.find('conn:c');
  assert.equal(moved.expanded, true); assert.equal(moved.loaded, true); assert.equal(moved.children, oldChildren);
  assert.equal(moved.connected, true); assert.equal(moved.icon, 'sqlite_open');
  assert.equal(moved.children[0].expandable, true); assert.equal(moved.children[0].icon, 'info_pane_database');
  assert.equal(app.openConnections, openConnections); assert.equal(app.openDbs, openDbs);
  assert.deepEqual([...openConnections], ['c']); assert.deepEqual([...openDbs], ['c/main']);
  assert.equal(app.tree.find('group:folder').expanded, true);
  assert.equal(h.panel.selectedNodeId, 'conn:c'); assert.equal(h.document.activeElement.dataset.id, 'conn:c');
  assert.match(notices[0][1], /顺序已保存/);

  const previousTree = app.treeNodes;
  responses.push(new Error('磁盘只读'));
  await assert.rejects(() => app.moveConnectionToGroup('c', 'empty'), /磁盘只读/);
  assert.equal(app.connections, next); assert.equal(app.treeNodes, previousTree);
  assert.equal(app.openConnections, openConnections); assert.equal(app.openDbs, openDbs);
  const count = requests.length;
  for (const args of [['missing', ''], ['c', 'deleted'], ['c', '', 'b'], ['c', '', 'c', 'before'], ['c', 'folder', 'b', 'after'], ['c', '', 'b', 'invalid']]) {
    await assert.rejects(() => app.moveConnectionToGroup(...args));
  }
  assert.equal(requests.length, count, 'invalid operations never reach IPC');
});

test('Windows HTML5 drop is enabled by disabling Tauri native drag interception', () => {
  const config = JSON.parse(fs.readFileSync(new URL('../../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
  assert.equal(config.app.windows[0].dragDropEnabled, false);
});
