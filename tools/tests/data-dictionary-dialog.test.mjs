// 数据字典导出向导（需求 1.1）：步骤流转 / 对象勾选 / 格式约束 / 文件名校验 / 后台执行。
// 使用项目既有的 esbuild + VM + 最小 DOM 方式；IPC 走 __PREVIEW_MOCK__，不访问真实网络。
import test from 'node:test';
import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import { build } from 'esbuild';

const root = fileURLToPath(new URL('../..', import.meta.url));
const built = await build({
  stdin: {
    contents: 'export { openDataDictionaryDialog } from "./src/dialogs/DataDictionaryDialog.ts"; export { openBackupDialog } from "./src/dialogs/BackupDialog.ts"; export { taskCenter } from "./src/core/backgroundTasks.ts";',
    resolveDir: root, loader: 'ts',
  },
  bundle: true, write: false, platform: 'node', format: 'cjs', loader: { '.css': 'empty' }, logLevel: 'silent',
});

// ---- 最小 DOM（同 settings-dialog-design.test.mjs 的桩）----
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
    return this.children.find(option => option.selected)?.value ?? this.children[0]?.value ?? '';
  }
  set value(value) {
    this._value = String(value);
    if (this.tagName === 'SELECT') {
      for (const option of this.children) option.selected = option.value === this._value;
    }
  }
  set textContent(value) { this._text = String(value); this._html = ''; this.children = []; }
  get textContent() { return this._text + this._html.replace(/<[^>]*>/g, '') + this.children.map(child => child.textContent).join(''); }
  set innerHTML(value) { this._html = String(value); this._text = ''; this.children = []; }
  get innerHTML() { return this._html; }
  appendChild(child) { child.remove(); this.children.push(child); child.parentNode = this; return child; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  prepend(...children) { for (const child of [...children].reverse()) { child.remove(); this.children.unshift(child); child.parentNode = this; } }
  replaceChildren(...children) { for (const child of [...this.children]) child.parentNode = null; this.children = []; for (const child of children) this.appendChild(child); }
  insertBefore(child, before) {
    child.remove(); const index = this.children.indexOf(before);
    this.children.splice(index < 0 ? this.children.length : index, 0, child); child.parentNode = this;
  }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); this.parentNode = null; }
  get firstChild() { return this.children[0] ?? null; }
  get isConnected() { return this === this.ownerDocument?.body || Boolean(this.parentNode?.isConnected); }
  get options() { return this.tagName === 'SELECT' ? this.children : []; }
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  hasAttribute(name) { return this.attributes.has(name) || ['disabled', 'hidden'].includes(name) && Boolean(this[name]); }
  getAttribute(name) {
    if (this.attributes.has(name)) return this.attributes.get(name);
    if (name === 'disabled' || name === 'hidden') return this[name] ? '' : null;
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

function metadataFixture() {
  return {
    name: 'navkit_demo', doc_title: 'navkit_demo数据库设计文档',
    tables: [
      { name: 'sys_user', comment: '用户表', db_type: 'MySQL', columns: [{ order: 1, name: 'id', type_name: 'bigint', length: '', primary_key: true, auto_increment: true, nullable: false, default_value: '', comment: '主键' }] },
      { name: 'sys_role', comment: '', db_type: 'MySQL', columns: [{ order: 1, name: 'id', type_name: 'int', length: '', primary_key: true, auto_increment: false, nullable: false, default_value: '', comment: '' }] },
    ],
    views: [{ name: 'v_user', comment: '用户视图', script: 'CREATE VIEW v_user AS SELECT 1;', db_type: 'MySQL' }],
    routines: [{ name: 'sp_reset', comment: '', script: 'CREATE PROCEDURE sp_reset() BEGIN SELECT 1; END', db_type: 'MySQL' }],
  };
}

function harness(exportResult = 2048) {
  const calls = [], exports = [];
  const mock = {
    list_tables: () => metadataFixture().tables.map(table => [table.name, "table"]),
    list_routines: () => [],
    list_databases: () => { calls.push(['list_databases']); return [['navkit_demo', 'mysql']]; },
    dictionary_metadata: args => { calls.push(['dictionary_metadata', args.id, args.schemaName]); return metadataFixture(); },
    dictionary_export: args => { exports.push(args); calls.push(['dictionary_export', args.opts.format, args.filePath, args.opts.excel_sheet_mode]); return exportResult; },
  };
  const document = { activeElement: null, createElement(tag) { const element = new Element(tag); element.ownerDocument = document; return element; } };
  document.body = document.createElement('body'); document.activeElement = document.body;
  const microtasks = [], module = { exports: {} };
  const context = vm.createContext({
    module, exports: module.exports, document, HTMLElement: Element, crypto: webcrypto, Error,
    queueMicrotask: callback => microtasks.push(callback),
    setTimeout: callback => microtasks.push(callback),
    clearTimeout: () => {},
    console,
    window: { innerWidth: 1280, innerHeight: 900, addEventListener() {}, removeEventListener() {}, __PREVIEW_MOCK__: mock },
  });
  vm.runInContext(built.outputFiles[0].text, context, { filename: 'data-dictionary-dialog.cjs' });
  const settle = async () => {
    while (microtasks.length) microtasks.shift()();
    for (let turn = 0; turn < 6; turn++) await Promise.resolve();
  };
  return { ...module.exports, document, settle, calls, exports };
}

const connections = [{ id: 'c1', name: '本地 MySQL', kind: 'mysql', database: 'navkit_demo' }];
const nextBtn = dlg => dlg.footer.children[0].children[1];
const backBtn = dlg => dlg.footer.children[0].children[0];
const backgroundBtn = dlg => dlg.footer.children[dlg.footer.children.length - 1];
const steps = dlg => dlg.body.children[0].children.filter(step => step.classList.contains('wizard-step'));
const activeStepIndex = dlg => steps(dlg).findIndex(step => step.className.includes('active'));


const labelledInput = (dlg, label) => dlg.body.querySelector('input[aria-label="' + label + '"]');
const chooseFormat = async (dlg, format, settle) => {
  await dlg.body.querySelectorAll('.dict-fmt-card').find(card => card.dataset.format === format).emit('click');
  await settle();
};
const objectStep = async fixture => {
  const dlg = fixture.openDataDictionaryDialog(connections, connections[0], 'navkit_demo', () => {});
  await fixture.settle(); await nextBtn(dlg).emit('click'); await fixture.settle();
  return dlg;
};
const exportStep = async (dlg, settle) => {
  while (activeStepIndex(dlg) < 3) { await nextBtn(dlg).emit('click'); await settle(); }
  labelledInput(dlg, '输出路径').value = 'D:/fixture-only';
  await nextBtn(dlg).emit('click'); await settle();
  assert.equal(activeStepIndex(dlg), 4);
};

test('章节复选框控制真实导出 DTO、选项与汇总，不清除对象页勾选', async () => {
  const fixture = harness(), dlg = await objectStep(fixture);
  for (const name of ['v_user', 'sp_reset']) { labelledInput(dlg, name).checked = true; await labelledInput(dlg, name).emit('change'); }
  await nextBtn(dlg).emit('click'); await fixture.settle();
  for (const label of ['视图', '存储过程 / 函数']) { labelledInput(dlg, label).checked = false; await labelledInput(dlg, label).emit('change'); }
  assert.equal(labelledInput(dlg, 'v_user').checked, true);
  assert.equal(labelledInput(dlg, 'sp_reset').checked, true);
  await exportStep(dlg, fixture.settle); await nextBtn(dlg).emit('click'); await fixture.settle();
  const result = fixture.exports[0]; assert.ok(result);
  assert.equal(result.dto.tables.length, 2); assert.equal(result.dto.views.length, 0); assert.equal(result.dto.routines.length, 0);
  assert.equal(result.opts.include_views, false); assert.equal(result.opts.include_routines, false);
  assert.match(dlg.body.textContent, /表 2、视图 0、过程 0/);
  assert.doesNotMatch(dlg.body.querySelector('.transfer-progress').textContent, /35%/);
});

test('章节选择在支持格式之间以及 Word 往返时保留，能力只改变 disabled', async () => {
  const fixture = harness(), dlg = await objectStep(fixture);
  await nextBtn(dlg).emit('click'); await fixture.settle();
  const views = labelledInput(dlg, '视图'), routines = labelledInput(dlg, '存储过程 / 函数');
  views.checked = false; await views.emit('change');
  await chooseFormat(dlg, 'pdf', fixture.settle);
  assert.equal(views.checked, false); assert.equal(routines.checked, true);
  await chooseFormat(dlg, 'word', fixture.settle);
  assert.equal(views.disabled, true); assert.equal(routines.disabled, true);
  assert.equal(views.checked, false); assert.equal(routines.checked, true);
  await chooseFormat(dlg, 'markdown', fixture.settle);
  assert.equal(views.disabled, false); assert.equal(routines.disabled, false);
  assert.equal(views.checked, false); assert.equal(routines.checked, true);
});

for (const [format, objectName, kind] of [['pdf', 'v_user', 'views'], ['markdown', 'sp_reset', 'routines']]) {
  test(format + ' 允许仅导出' + kind + '，实际 DTO 不含表', async () => {
    const fixture = harness(), dlg = await objectStep(fixture);
    for (const name of ['sys_user', 'sys_role', 'v_user', 'sp_reset']) labelledInput(dlg, name).checked = name === objectName;
    await labelledInput(dlg, objectName).emit('change');
    await nextBtn(dlg).emit('click'); await fixture.settle(); await chooseFormat(dlg, format, fixture.settle);
    await exportStep(dlg, fixture.settle); await nextBtn(dlg).emit('click'); await fixture.settle();
    const result = fixture.exports[0]; assert.ok(result);
    assert.equal(result.opts.format, format); assert.equal(result.dto.tables.length, 0);
    assert.equal(result.dto[kind].length, 1); assert.equal(result.opts['include_' + kind], true);
    assert.equal(result.dto[kind][0].name, objectName);
  });
}

test('Word 拒绝仅视图；Markdown 取消唯一有效章节后也不能空导出', async () => {
  const fixture = harness(), dlg = await objectStep(fixture);
  for (const name of ['sys_user', 'sys_role', 'sp_reset']) labelledInput(dlg, name).checked = false;
  labelledInput(dlg, 'v_user').checked = true;
  await nextBtn(dlg).emit('click'); await fixture.settle(); await chooseFormat(dlg, 'word', fixture.settle);
  await exportStep(dlg, fixture.settle); await nextBtn(dlg).emit('click'); await fixture.settle();
  assert.equal(fixture.exports.length, 0); assert.match(dlg.body.textContent, /至少需要选择一张表/);
  await chooseFormat(dlg, 'markdown', fixture.settle);
  labelledInput(dlg, '视图').checked = false; await labelledInput(dlg, '视图').emit('change');
  await nextBtn(dlg).emit('click'); await fixture.settle();
  assert.equal(fixture.exports.length, 0); assert.match(dlg.body.textContent, /当前格式与文档章节允许导出的对象/);
});

for (const succeeds of [true, false]) {
  test('字典共享进度：未知总量无伪百分比，' + (succeeds ? '成功才显示完成' : '失败不冒充完成'), async () => {
    let resolve, reject;
    const pending = new Promise((yes, no) => { resolve = yes; reject = no; });
    const fixture = harness(pending), dlg = await objectStep(fixture);
    await exportStep(dlg, fixture.settle);
    const execution = nextBtn(dlg).emit('click'); await fixture.settle();
    const progress = dlg.body.querySelector('.transfer-progress'), bar = progress.querySelector('[role="progressbar"]');
    assert.equal(progress.dataset.state, 'running'); assert.equal(bar.getAttribute('aria-valuenow'), null);
    assert.equal(dlg.body.querySelectorAll('.transfer-progress').length, 1);
    assert.match(progress.textContent, /无法预估剩余量/);
    if (succeeds) resolve(2048); else reject(new Error('fixture export rejected'));
    await execution; await fixture.settle();
    assert.equal(progress.dataset.state, succeeds ? 'done' : 'failed');
    assert.equal(bar.getAttribute('aria-valuenow'), succeeds ? '100' : null);
  });
}

test('备份保留空根并使用独立原生折叠/全选按钮，不吞子控件键盘', async () => {
  const fixture = harness(), dlg = fixture.openBackupDialog(connections[0], 'navkit_demo', () => {});
  await fixture.settle();
  const tree = dlg.body.querySelector('.backup-tree'), groups = tree.querySelectorAll('.backup-tree-group');
  assert.equal(tree.getAttribute('role'), 'group'); assert.equal(groups.length, 3);
  assert.equal(tree.querySelectorAll('[role="treeitem"]').length, 0);
  const table = groups[0], root = table.querySelector('.backup-tree-row'), children = table.querySelector('.backup-tree-children');
  const [expand, toggle] = root.querySelectorAll('button');
  assert.equal(expand.tagName, 'BUTTON'); assert.equal(toggle.tagName, 'BUTTON');
  assert.equal(expand.parentNode, toggle.parentNode); assert.equal(expand.querySelectorAll('button').length, 0);
  assert.equal(expand.getAttribute('aria-controls'), children.id); assert.equal(expand.getAttribute('aria-expanded'), 'true');
  // 模拟原生按钮激活前的冒泡 keydown：容器不能 preventDefault 或折叠。
  for (const key of [' ', 'Enter']) {
    const event = await root.emit('keydown', { target: toggle, key });
    assert.equal(event.defaultPrevented, false); assert.equal(children.hidden, false);
    await toggle.emit('click', { stopPropagation() {} });
    const checks = table.querySelectorAll('input[type="checkbox"]');
    assert.equal(checks.every(input => input.checked), key === 'Enter');
  }
  await expand.emit('click'); assert.equal(children.hidden, true); assert.equal(expand.getAttribute('aria-expanded'), 'false');
  await expand.emit('click'); assert.equal(children.hidden, false);
  for (const group of groups.slice(1)) {
    assert.equal(group.hidden, false); assert.equal(group.querySelector('.backup-tree-empty').hidden, false);
    assert.equal(group.querySelectorAll('button')[1].hidden, true);
  }
});

test('向导五步齐全，默认进入数据源步', async () => {
  const { openDataDictionaryDialog, settle, document } = harness();
  const dlg = openDataDictionaryDialog(connections, connections[0], 'navkit_demo', () => {});
  await settle();
  const labels = steps(dlg).map(step => step.textContent);
  assert.deepEqual(labels.filter(Boolean), ['1数据源', '2对象', '3格式', '4文件', '5导出']);
  assert.equal(activeStepIndex(dlg), 0);
  // 连接下拉已填充
  const connSelect = dlg.body.querySelectorAll('select')[0];
  assert.equal(connSelect.children.length, 1);
  document.body.appendChild(dlg.el);
});

test('进入对象步自动加载元数据，表默认勾选、视图与过程不勾选', async () => {
  const { openDataDictionaryDialog, settle, calls } = harness();
  const dlg = openDataDictionaryDialog(connections, connections[0], 'navkit_demo', () => {});
  await settle();
  await nextBtn(dlg).emit('click');
  await settle();
  assert.equal(activeStepIndex(dlg), 1);
  assert.ok(calls.some(c => c[0] === 'list_databases'), '进入数据源步时读库列表');
  assert.ok(calls.some(c => c[0] === 'dictionary_metadata' && c[1] === 'c1' && c[2] === 'navkit_demo'));
  // 对象勾选框（按 aria-label 限定到对象行）：2 表默认勾选，视图与过程不勾选
  const objectNames = ['sys_user', 'sys_role', 'v_user', 'sp_reset'];
  const objectInputs = dlg.body.querySelectorAll('input[type="checkbox"]')
    .filter(input => objectNames.includes(input.getAttribute('aria-label')));
  assert.equal(objectInputs.length, 4);
  assert.deepEqual(objectInputs.filter(input => input.checked).map(input => input.getAttribute('aria-label')).sort(),
    ['sys_role', 'sys_user'], '表默认勾选，视图与过程不勾选');
});

test('不勾选任何对象时不能进入下一步', async () => {
  const { openDataDictionaryDialog, settle } = harness();
  const dlg = openDataDictionaryDialog(connections, connections[0], 'navkit_demo', () => {});
  await settle();
  await nextBtn(dlg).emit('click');
  await settle();
  // 全部取消勾选
  for (const input of dlg.body.querySelectorAll('input[type="checkbox"]').filter(i => i.type === 'checkbox')) input.checked = false;
  await nextBtn(dlg).emit('click');
  await settle();
  assert.equal(activeStepIndex(dlg), 1, '校验失败应留在对象步');
  // 勾选一张表后可通过
  for (const input of dlg.body.querySelectorAll('input[type="checkbox"]').filter(i => i.type === 'checkbox')) input.checked = input.getAttribute('aria-label') === 'sys_user';
  await nextBtn(dlg).emit('click');
  await settle();
  assert.equal(activeStepIndex(dlg), 2);
});

test('格式卡：Word 禁用视图/过程勾选；Excel 显示 sheet 模式', async () => {
  const { openDataDictionaryDialog, settle } = harness();
  const dlg = openDataDictionaryDialog(connections, connections[0], 'navkit_demo', () => {});
  await settle();
  await nextBtn(dlg).emit('click');
  await settle();
  await nextBtn(dlg).emit('click'); // 到格式步
  await settle();
  const cards = dlg.body.querySelectorAll('.dict-fmt-card');
  assert.equal(cards.length, 5);
  const wordCard = cards.find(card => card.dataset.format === 'word');
  const excelCard = cards.find(card => card.dataset.format === 'excel');
  const sectionInputs = () => dlg.body.querySelectorAll('input[type="checkbox"]').filter(input => !input.disabled || input.disabled);
  // 切到 Word：视图/过程勾选被禁用
  await wordCard.emit('click');
  await settle();
  const disabledAfterWord = dlg.body.querySelectorAll('input[type="checkbox"]').filter(i => i.disabled).length;
  assert.ok(disabledAfterWord >= 2, `Word 模式应禁用视图/过程勾选，实际禁用 ${disabledAfterWord}`);
  // 切到 Excel：显示工作表方式（两个 radio）
  await excelCard.emit('click');
  await settle();
  const radios = dlg.body.querySelectorAll('input[type="radio"]');
  assert.equal(radios.length, 2, 'Excel sheet 模式两个选项');
  assert.equal(radios.filter(r => r.checked).length, 1);
  // 单 sheet 默认选中
  assert.equal(radios[0].value, 'single');
});

test('文件名校验：非法字符被拦截，填写路径后放行', async () => {
  const { openDataDictionaryDialog, settle } = harness();
  const dlg = openDataDictionaryDialog(connections, connections[0], 'navkit_demo', () => {});
  await settle();
  for (let i = 0; i < 3; i += 1) { await nextBtn(dlg).emit('click'); await settle(); }
  assert.equal(activeStepIndex(dlg), 3, '前三步应顺利通过（默认勾选了表）');
  const nameInput = dlg.body.querySelector('input[aria-label="文档名称"]');
  const pathInput = dlg.body.querySelector('input[aria-label="输出路径"]');
  assert.ok(nameInput && pathInput, '文件步包含名称与路径输入');
  nameInput.value = 'bad|name';
  await nextBtn(dlg).emit('click');
  await settle();
  assert.equal(activeStepIndex(dlg), 3, '非法文件名应拦截');
  nameInput.value = 'navkit_demo数据库设计文档20261112';
  await nextBtn(dlg).emit('click');
  await settle();
  assert.equal(activeStepIndex(dlg), 3, '缺输出路径应拦截');
  pathInput.value = 'D:\\导出\\数据字典';
  await nextBtn(dlg).emit('click');
  await settle();
  assert.equal(activeStepIndex(dlg), 4, '填写后进入导出步');
  // 后台执行按钮只在导出步出现
  assert.equal(backgroundBtn(dlg).hidden, false);
  assert.equal(backgroundBtn(dlg).textContent, '后台执行');
});

test('后台执行：调用 dictionary_export 并注册 dictionary 任务', async () => {
  const { openDataDictionaryDialog, settle, calls, taskCenter } = harness();
  taskCenter.reset();
  let donePath = '';
  const dlg = openDataDictionaryDialog(connections, connections[0], 'navkit_demo', path => { donePath = path; });
  await settle();
  for (let i = 0; i < 4; i += 1) { await nextBtn(dlg).emit('click'); await settle(); }
  dlg.body.querySelector('input[aria-label="输出路径"]').value = 'D:\\导出';
  await nextBtn(dlg).emit('click');
  await settle();
  await backgroundBtn(dlg).emit('click');
  await settle();
  const exportCall = calls.find(c => c[0] === 'dictionary_export');
  assert.ok(exportCall, '应调用 dictionary_export');
  assert.equal(exportCall[1], 'markdown', '默认格式 markdown');
  assert.match(exportCall[2], /navkit_demo数据库设计文档\d+\.md$/);
  assert.ok(donePath.endsWith('.md'), `onDone 收到产物路径：${donePath}`);
  const tasks = taskCenter.snapshot().tasks;
  const dictTask = tasks.find(t => t.kind === 'dictionary');
  assert.ok(dictTask, '任务中心应有 dictionary 任务');
  assert.equal(dictTask.status, 'success');
  assert.match(dictTask.detail, /2\.0 KB/);
});

test('Excel 单 sheet 模式把 excel_sheet_mode 传给后端', async () => {
  const { openDataDictionaryDialog, settle, calls } = harness();
  const dlg = openDataDictionaryDialog(connections, connections[0], 'navkit_demo', () => {});
  await settle();
  for (let i = 0; i < 3; i += 1) { await nextBtn(dlg).emit('click'); await settle(); }
  const excelCard = dlg.body.querySelectorAll('.dict-fmt-card').find(card => card.dataset.format === 'excel');
  await excelCard.emit('click');
  await settle();
  const radios = dlg.body.querySelectorAll('input[type="radio"]');
  radios[1].checked = true; // 每表一 sheet
  radios[1].emit('change');
  dlg.body.querySelector('input[aria-label="输出路径"]').value = 'D:\\导出';
  await nextBtn(dlg).emit('click');
  await settle();
  await nextBtn(dlg).emit('click'); // 开始导出
  await settle();
  const exportCall = calls.find(c => c[0] === 'dictionary_export');
  assert.equal(exportCall[1], 'excel');
  assert.equal(exportCall[3], 'per_table');
});
