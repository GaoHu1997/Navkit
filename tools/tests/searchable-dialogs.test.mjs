// 真实 Chromium + 三个产品对话框；只构建内存 fixture，不启动应用或访问数据库。
// 全局安装两种搜索增强，IPC 全走显式 __PREVIEW_MOCK__；样式使用产品原文件。
import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';

const root = fileURLToPath(new URL('../../', import.meta.url));
let browser, server, origin;
const source = `
import { installSearchableSelects } from './src/ui/SearchableSelect.ts';
import { installSearchableInputs } from './src/ui/SearchableInput.ts';
import { openDataDictionaryDialog } from './src/dialogs/DataDictionaryDialog.ts';
import { openDataGeneratorDialog } from './src/dialogs/DataGeneratorDialog.ts';
import { openEditUserDialog } from './src/dialogs/UserDialog.ts';
const connections = [
  { id: 'conn-alpha', name: 'Alpha 本地', kind: 'mysql' },
  { id: 'CONN_PRODUCTION', name: '生产数据', kind: 'mysql' },
].map(c => ({ host: '127.0.0.1', port: 3306, user: 'fixture', password: '', database: '',
  file: '', params: '', group: '', read_only: false, keep_alive: false, ...c }));
const databases = [['alpha_dev', '数据库'], ['销售中心', '数据库'], ['ARCHIVE_KEY', '数据库']];
const tables = [['audit_log', 'table'], ['客户明细', 'table'], ['ACTIVE_USERS', 'view'], ['not_a_table', 'procedure']];
const detail = {
  account: { user: 'fixture', host: '%', plugin: 'caching_sha2_password', password_expired: false,
    password_lifetime_days: null, account_locked: false, max_questions: 0, max_updates: 0,
    max_connections: 0, max_user_connections: 0, ssl_type: '', ssl_cipher: '', x509_issuer: '', x509_subject: '' },
  server_privileges: [{ name: 'Select', granted: false, grantable: false }],
  object_privileges: [], roles: [], members: [], supports_roles: false, warnings: [],
};
const fixture = window.fixture = {
  connections, calls: [], events: [], gates: new Map(), dialog: null, saved: [],
  hold(command) {
    let resolve;
    const promise = new Promise(r => { resolve = r; });
    this.gates.set(command, { promise, resolve });
  },
  release(command) {
    const gate = this.gates.get(command);
    if (!gate) throw new Error('No pending gate: ' + command);
    this.gates.delete(command); gate.resolve();
  },
  disposers: [installSearchableSelects(), installSearchableInputs()],
};
window.__PREVIEW_MOCK__ = new Proxy({}, { get(_target, command) {
  return async args => {
    fixture.calls.push({ command, args: structuredClone(args ?? {}) });
    const gate = fixture.gates.get(command);
    if (gate) await gate.promise;
    switch (command) {
      case 'ensure_connection': return { ok: true, message: '', version: 'fixture', charset: 'utf8mb4' };
      case 'list_databases': return structuredClone(databases);
      case 'list_tables': return structuredClone(tables);
      case 'get_table_schema': return { table: args.table, database: args.schemaName,
        columns: [{ name: 'id', type: 'INTEGER', nullable: false, default: null, primary_key: true,
          auto_increment: false, comment: null, length: null, precision: null, scale: null }],
        indexes: [], foreign_keys: [], comment: null, engine: 'InnoDB', collation: null, row_count: 0 };
      case 'dictionary_metadata': return { name: args.schemaName, doc_title: args.schemaName,
        tables: tables.filter(([, type]) => type === 'table').map(([name]) => ({ name, comment: '', db_type: 'mysql', columns: [] })),
        views: [], routines: [] };
      case 'read_db_user': return structuredClone(detail);
      case 'preview_db_user_changes': return { statements: [], warnings: [] };
      case 'apply_db_user_changes': return { statements: [], executed: 0 };
      default: throw new Error('Unexpected fixture IPC: ' + String(command));
    }
  };
} });
for (const type of ['input', 'change']) document.addEventListener(type, event => {
  if (event.target instanceof HTMLSelectElement) fixture.events.push({ type,
    name: event.target.getAttribute('aria-label'), value: event.target.value });
});
document.querySelector('#open-dictionary').addEventListener('click', () => {
  fixture.dialog = openDataDictionaryDialog(connections, connections[0], 'alpha_dev', file => fixture.saved.push(file));
});
document.querySelector('#open-generator').addEventListener('click', () => {
  fixture.dialog = openDataGeneratorDialog(connections[0], 'alpha_dev', 'audit_log',
    (...args) => fixture.saved.push(args), undefined, connections);
});
document.querySelector('#open-user').addEventListener('click', () => {
  fixture.dialog = openEditUserDialog(connections[0].id, 'fixture', '%', identity => fixture.saved.push(identity));
});
`;
const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>产品对话框搜索回归</title>
<link rel="stylesheet" href="/base.css"><link rel="stylesheet" href="/searchable-select.css">
<link rel="stylesheet" href="/data-dictionary.css"><link rel="stylesheet" href="/user-dialog.css">
</head><body><main style="padding:16px">
<button id="open-dictionary" type="button">打开数据字典</button>
<button id="open-generator" type="button">打开数据生成</button>
<button id="open-user" type="button">编辑用户</button>
</main><script type="module" src="/fixture.js"></script></body></html>`;

before(async () => {
  const bundled = await build({
    stdin: { contents: source, resolveDir: root, sourcefile: 'searchable-dialogs-fixture.ts', loader: 'ts' },
    bundle: true, write: false, format: 'esm', platform: 'browser', loader: { '.css': 'empty' }, logLevel: 'silent',
  });
  const files = new Map([
    ['/', ['text/html', html]], ['/fixture.js', ['text/javascript', bundled.outputFiles[0].text]],
    ['/base.css', ['text/css', await readFile(path.join(root, 'src/styles/base.css'), 'utf8')]],
    ['/searchable-select.css', ['text/css', await readFile(path.join(root, 'src/ui/searchable-select.css'), 'utf8')]],
    ['/data-dictionary.css', ['text/css', await readFile(path.join(root, 'src/styles/data-dictionary.css'), 'utf8')]],
    ['/user-dialog.css', ['text/css', await readFile(path.join(root, 'src/styles/user-dialog.css'), 'utf8')]],
  ]);
  server = createServer((req, res) => {
    const file = files.get(new URL(req.url, 'http://127.0.0.1').pathname);
    res.writeHead(file ? 200 : 404, { 'Content-Type': `${file?.[0] ?? 'text/plain'}; charset=utf-8` });
    res.end(file?.[1] ?? '未找到');
  });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  origin = `http://127.0.0.1:${server.address().port}`;
  const executablePath = process.env.BROWSER_EXE || [
    'C:/Program Files/Google/Chrome/Application/chrome.exe',
    'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
    '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome',
  ].find(existsSync);
  browser = await chromium.launch({ executablePath, headless: true });
}, { timeout: 45000 });
after(async () => {
  try { await browser?.close(); } finally { if (server) await new Promise(resolve => server.close(resolve)); }
});

async function withPage(run) {
  const context = await browser.newContext({ viewport: { width: 1120, height: 860 } });
  const page = await context.newPage();
  page.setDefaultTimeout(6000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    await page.goto(origin);
    await page.waitForFunction(() => !!window.fixture);
    assert.equal(await page.evaluate(() => fixture.disposers.length), 2);
    assert.equal(await page.locator('link[href="/base.css"]').evaluate(el => !!el.sheet), true);
    assert.equal(await page.locator('link[href="/searchable-select.css"]').evaluate(el => !!el.sheet), true);
    await run(page);
    assert.deepEqual(errors, [], '真实页面不得出现未捕获异常');
  } catch (error) {
    console.error('fixture diagnostics:', await page.locator('select').evaluateAll(controls => controls.map(el => ({
      name: el.getAttribute('aria-label'), field: el.closest('.field')?.querySelector('.field-label')?.textContent,
      value: el.value, disabled: el.disabled,
    }))));
    throw error;
  } finally { await context.close(); }
}
const select = (page, name) => page.getByRole('combobox', { name, exact: true });
const calls = (page, command) => page.evaluate(cmd => fixture.calls.filter(call => call.command === cmd), command);
const events = (page, name) => page.evaluate(label => fixture.events.filter(event => event.name === label), name);
const rows = page => page.locator('.searchable-select-popover').getByRole('option');
const activeStep = page => page.locator('.wizard-step.active').textContent();
async function waitCall(page, command, count = 1) {
  await page.waitForFunction(({ cmd, count }) => fixture.calls.filter(call => call.command === cmd).length >= count, { cmd: command, count });
}
async function waitReady(page, name) {
  await page.waitForFunction(label => [...document.querySelectorAll('select')]
    .some(el => el.getAttribute('aria-label') === label && !el.disabled && el.options.length > 0), name);
}
async function openSearch(page, name, keyboard = false) {
  const control = select(page, name);
  if (keyboard) { await control.focus(); await page.keyboard.press('ArrowDown'); }
  else await control.click();
  const search = page.getByRole('combobox', { name: `搜索${name}`, exact: true });
  await search.waitFor();
  assert.equal(await search.getAttribute('aria-expanded'), 'true');
  assert.equal(await search.evaluate(el => el === document.activeElement), true);
  const id = await search.getAttribute('aria-controls');
  assert.ok(id, '搜索输入必须与候选列表建立可访问关联');
  assert.equal(await page.locator(`[id="${id}"]`).getAttribute('role'), 'listbox');
  assert.equal(await page.locator(`[id="${id}"]`).getAttribute('aria-label'), name);
  assert.equal(await page.locator('.searchable-select-popover').evaluate(el => el.matches(':popover-open') && !!el.closest('.dialog')), true);
  return search;
}
async function confirmSearch(page, name, query, expected, value, { keyboard = false, mouse = false } = {}) {
  const control = select(page, name);
  const previous = await control.inputValue();
  const beforeEvents = await events(page, name);
  const beforeStep = await page.locator('.wizard-step.active').count() ? await activeStep(page) : null;
  const search = await openSearch(page, name, keyboard);
  await search.fill(query);
  assert.deepEqual(await rows(page).allTextContents(), expected);
  assert.equal(await control.inputValue(), previous, '过滤不应修改原生值');
  assert.deepEqual(await events(page, name), beforeEvents, '过滤不应触发业务 change');
  if (mouse) await rows(page).filter({ hasText: expected[0] }).click();
  else await search.press('Enter');
  await page.waitForFunction(() => !document.querySelector('.searchable-select-popover'));
  assert.equal(await control.inputValue(), value);
  assert.equal(await control.evaluate(el => el === document.activeElement), true, '确认后焦点必须回到原生下拉');
  assert.equal(await control.getAttribute('aria-controls'), null);
  const delta = previous === value ? [] : [{ type: 'input', name, value }, { type: 'change', name, value }];
  assert.deepEqual(await events(page, name), [...beforeEvents, ...delta], '确认仅触发一次 input/change');
  if (beforeStep !== null) assert.equal(await activeStep(page), beforeStep, '搜索回车不能推进产品向导');
}
async function loadingCannotOpen(page, name) {
  const control = select(page, name);
  assert.equal(await control.isDisabled(), true, `${name} 加载中必须使用原生禁用状态`);
  const previous = await control.inputValue();
  const beforeEvents = await events(page, name);
  const beforeCalls = await page.evaluate(() => fixture.calls.length);
  // 除真实 disabled 外，还检查辅助技术/脚本 click 与键盘打开入口不能绕过锁。
  await control.dispatchEvent('click');
  await control.dispatchEvent('keydown', { key: 'ArrowDown', bubbles: true });
  assert.equal(await page.locator('.searchable-select-popover').count(), 0);
  assert.equal(await control.inputValue(), previous);
  assert.deepEqual(await events(page, name), beforeEvents);
  assert.equal(await page.evaluate(() => fixture.calls.length), beforeCalls, '禁用入口不得触发下游业务请求');
}
async function assertNoWrite(page) {
  assert.deepEqual(await page.evaluate(() => fixture.calls.filter(({ command }) =>
    /^(dictionary_export|apply_db_user_changes|start_batch|execute_generated|execute_sql)/.test(command))), []);
  assert.deepEqual(await page.evaluate(() => fixture.saved), []);
}

// 场景 1：从产品入口实际打开字典，连接和数据库搜索确认后使用真实业务值加载对象。
test('DataDictionaryDialog：连接按值/标签过滤、库搜索确认，不误推进或导出', () => withPage(async page => {
  await page.getByRole('button', { name: '打开数据字典', exact: true }).click();
  await page.getByRole('dialog', { name: '数据字典', exact: true }).waitFor();
  await waitReady(page, '数据库');
  await confirmSearch(page, '连接', 'conn_production', ['生产数据 · mysql'], 'CONN_PRODUCTION', { keyboard: true });
  await waitCall(page, 'list_databases', 2);
  await waitReady(page, '数据库');
  assert.equal((await calls(page, 'list_databases')).at(-1).args.id, 'CONN_PRODUCTION');
  // 同值也可按可见中文名称命中，确认不得再派发 change。
  await confirmSearch(page, '连接', '生产', ['生产数据 · mysql'], 'CONN_PRODUCTION', { mouse: true });
  await confirmSearch(page, '数据库', '销售', ['销售中心'], '销售中心', { mouse: true });
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  await waitCall(page, 'dictionary_metadata');
  assert.deepEqual((await calls(page, 'dictionary_metadata')).at(-1).args, { id: 'CONN_PRODUCTION', schemaName: '销售中心' });
  await page.waitForFunction(() => document.querySelectorAll('.backup-tree-leaf').length === 2);
  await assertNoWrite(page);
}));

test('DataDictionaryDialog：读取库列表时搜索增强不能绕过禁用，完成后恢复搜索', () => withPage(async page => {
  await page.evaluate(() => fixture.hold('list_databases'));
  await page.getByRole('button', { name: '打开数据字典', exact: true }).click();
  await waitCall(page, 'list_databases');
  await loadingCannotOpen(page, '数据库');
  await page.evaluate(() => fixture.release('list_databases'));
  await waitReady(page, '数据库');
  await confirmSearch(page, '数据库', 'archive_key', ['ARCHIVE_KEY'], 'ARCHIVE_KEY', { keyboard: true });
  await page.evaluate(() => fixture.hold('list_databases'));
  await confirmSearch(page, '连接', '生产', ['生产数据 · mysql'], 'CONN_PRODUCTION');
  await waitCall(page, 'list_databases', 2);
  await loadingCannotOpen(page, '数据库');
  await page.evaluate(() => fixture.release('list_databases'));
  await waitReady(page, '数据库');
  await confirmSearch(page, '数据库', '销售', ['销售中心'], '销售中心');
  await assertNoWrite(page);
}));

// 场景 2：连接切换→库切换→实际表结构加载，生成预览以前不产生写入。
test('DataGeneratorDialog：连接/库/表搜索确认驱动元数据，视图不能作为可写表候选', () => withPage(async page => {
  await page.getByRole('button', { name: '打开数据生成', exact: true }).click();
  await page.getByRole('dialog', { name: '数据生成', exact: true }).waitFor();
  await waitReady(page, '目标表');
  await confirmSearch(page, '连接', 'conn_production', ['生产数据 (mysql)'], 'CONN_PRODUCTION', { keyboard: true });
  await waitCall(page, 'ensure_connection', 2);
  await waitReady(page, '目标数据库');
  await confirmSearch(page, '目标数据库', '销售', ['销售中心'], '销售中心', { mouse: true });
  await waitReady(page, '目标表');
  assert.deepEqual((await calls(page, 'list_tables')).at(-1).args, { id: 'CONN_PRODUCTION', schemaName: '销售中心' });
  const search = await openSearch(page, '目标表', true);
  assert.deepEqual(await rows(page).allTextContents(), ['audit_log', '客户明细']);
  const previous = await select(page, '目标表').inputValue();
  await search.fill('ACTIVE_USERS');
  assert.deepEqual(await rows(page).allTextContents(), []);
  assert.match(await page.locator('.searchable-select-empty').textContent(), /没有匹配/);
  await search.press('Enter');
  assert.equal(await select(page, '目标表').inputValue(), previous, '零匹配回车不得选择隐藏或非表对象');
  await search.press('Escape');
  assert.equal(await page.getByRole('dialog', { name: '数据生成', exact: true }).count(), 1, 'Escape 只关闭搜索层');
  await confirmSearch(page, '目标表', '客户', ['客户明细'], '客户明细');
  await waitCall(page, 'get_table_schema');
  assert.deepEqual((await calls(page, 'get_table_schema')).at(-1).args, { id: 'CONN_PRODUCTION', schemaName: '销售中心', table: '客户明细' });
  await assertNoWrite(page);
}));

test('DataGeneratorDialog：库/表各自加载期间禁用，不经搜索浮层触发结构读取', () => withPage(async page => {
  await page.evaluate(() => { fixture.hold('list_databases'); fixture.hold('list_tables'); });
  await page.getByRole('button', { name: '打开数据生成', exact: true }).click();
  await waitCall(page, 'list_databases');
  await loadingCannotOpen(page, '目标数据库');
  await loadingCannotOpen(page, '目标表');
  await page.evaluate(() => fixture.release('list_databases'));
  await waitCall(page, 'list_tables');
  await loadingCannotOpen(page, '目标表');
  assert.deepEqual(await calls(page, 'get_table_schema'), []);
  await page.evaluate(() => fixture.release('list_tables'));
  await waitReady(page, '目标表');
  await confirmSearch(page, '目标表', '客户', ['客户明细'], '客户明细', { keyboard: true });
  assert.equal((await calls(page, 'get_table_schema')).at(-1).args.table, '客户明细');
  await assertNoWrite(page);
}));

// 场景 3：用户权限实际 tab 中的库/表下拉；空值（整库）不是占位，不得被增强过滤。
test('UserDialog：权限库/表搜索确认且搜索回车不保存，整库空值保留到权限草稿', () => withPage(async page => {
  await page.getByRole('button', { name: '编辑用户', exact: true }).click();
  await page.getByRole('tab', { name: '权限', exact: true }).click();
  await waitReady(page, '数据库');
  await confirmSearch(page, '数据库', '销售', ['销售中心'], '销售中心', { keyboard: true });
  await waitReady(page, '表（留空=整库）');
  assert.deepEqual((await calls(page, 'list_tables')).at(-1).args, { id: 'conn-alpha', schemaName: '销售中心' });
  const search = await openSearch(page, '表（留空=整库）');
  assert.deepEqual(await rows(page).allTextContents(), ['（整库）', 'audit_log', '客户明细', 'ACTIVE_USERS']);
  await search.press('Escape');
  await confirmSearch(page, '表（留空=整库）', 'active_users', ['ACTIVE_USERS'], 'ACTIVE_USERS', { mouse: true });
  await page.getByRole('button', { name: '添加权限', exact: true }).click();
  await confirmSearch(page, '表（留空=整库）', '整库', ['（整库）'], '', { keyboard: true });
  await page.getByRole('button', { name: '添加权限', exact: true }).click();
  assert.deepEqual(await page.locator('.user-object-table tbody tr').evaluateAll(rows => rows.map(row =>
    [row.cells[0].textContent, row.cells[1].textContent])), [['销售中心', 'ACTIVE_USERS'], ['销售中心', '（整库）']]);
  const selectColumn = await page.getByRole('columnheader', { name: 'Select', exact: true }).evaluate(el => el.cellIndex);
  for (const row of await page.locator('.user-object-table tbody tr').all()) {
    await row.getByRole('cell').nth(selectColumn).getByRole('checkbox').check();
  }
  await page.getByRole('tab', { name: 'SQL 预览', exact: true }).click();
  await waitCall(page, 'preview_db_user_changes');
  const grants = (await calls(page, 'preview_db_user_changes')).at(-1).args.draft.object_grants;
  assert.deepEqual(grants, [
    { database: '销售中心', name: 'ACTIVE_USERS', privileges: ['Select'] },
    { database: '销售中心', name: '*', privileges: ['Select'] },
  ]);
  await assertNoWrite(page);
}));

test('UserDialog：权限元数据加载中不能打开搜索或添加，加载完成后整库可确认', () => withPage(async page => {
  await page.evaluate(() => { fixture.hold('list_databases'); fixture.hold('list_tables'); });
  await page.getByRole('button', { name: '编辑用户', exact: true }).click();
  await page.getByRole('tab', { name: '权限', exact: true }).click();
  await waitCall(page, 'list_databases');
  await loadingCannotOpen(page, '数据库');
  assert.equal(await page.getByRole('button', { name: '添加权限', exact: true }).isDisabled(), true);
  await page.evaluate(() => fixture.release('list_databases'));
  await waitReady(page, '数据库');
  const dbSearch = await openSearch(page, '数据库');
  assert.deepEqual(await rows(page).allTextContents(), ['alpha_dev', '销售中心', 'ARCHIVE_KEY'], '请选择占位不应进入候选');
  await dbSearch.press('Escape');
  await confirmSearch(page, '数据库', 'archive_key', ['ARCHIVE_KEY'], 'ARCHIVE_KEY');
  await waitCall(page, 'list_tables');
  await loadingCannotOpen(page, '表（留空=整库）');
  assert.equal(await page.getByRole('button', { name: '添加权限', exact: true }).isDisabled(), true);
  assert.equal(await page.locator('.user-object-table tbody tr').count(), 0);
  await page.evaluate(() => fixture.release('list_tables'));
  await waitReady(page, '表（留空=整库）');
  await confirmSearch(page, '表（留空=整库）', '客户', ['客户明细'], '客户明细');
  await confirmSearch(page, '表（留空=整库）', '整库', ['（整库）'], '', { keyboard: true });
  await page.getByRole('button', { name: '添加权限', exact: true }).click();
  assert.equal(await page.locator('.user-object-table tbody tr td').nth(1).textContent(), '（整库）');
  await assertNoWrite(page);
}));

// 生成方式选项少且无需过滤：data-no-search 直接列候选，点选即生效。
test('DataGeneratorDialog：字段生成方式直接选择，不渲染搜索框', () => withPage(async page => {
  await page.getByRole('button', { name: '打开数据生成', exact: true }).click();
  await waitReady(page, '目标表');
  await page.getByRole('button', { name: '下一步', exact: true }).click();
  const mode = select(page, 'id 生成方式');
  await mode.waitFor();
  assert.equal(await mode.getAttribute('data-no-search'), '');
  await mode.click();
  assert.equal(await page.locator('.searchable-select-search').count(), 0, '生成方式不提供搜索框');
  assert.deepEqual(await rows(page).allTextContents(),
    ['按类型生成', '固定值', '顺序整数', '省略（数据库默认；必填列由后端拒绝）']);
  assert.equal(await page.locator('.searchable-select-list').evaluate(el => el === document.activeElement), true, '焦点落在候选列表');
  await rows(page).filter({ hasText: '顺序整数' }).click();
  await page.waitForFunction(() => !document.querySelector('.searchable-select-popover'));
  assert.equal(await mode.inputValue(), 'sequence');
  assert.equal(await mode.evaluate(el => el === document.activeElement), true, '选择后焦点回到原生下拉');
  // 需要过滤的目标表仍保留搜索框。
  await page.getByRole('button', { name: '上一步', exact: true }).click();
  const search = await openSearch(page, '目标表');
  await search.press('Escape');
  await assertNoWrite(page);
}));
