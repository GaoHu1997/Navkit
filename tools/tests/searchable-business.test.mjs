// 真实 Chromium + 产品组件/对话框：验证搜索增强在业务场景中的实际落点。
// 覆盖：ER 索引字段 datalist、同步目标列 datalist、备份 / 整库同步对象树过滤。
// 隔离 HTTP 页面与 __PREVIEW_MOCK__，不访问真实数据库或原生 IPC。
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
import { ErTableEditor } from './src/ui/ErTableEditor.ts';
import { openBackupDialog } from './src/dialogs/BackupDialog.ts';
import { openSyncDialog } from './src/dialogs/SyncDialog.ts';
import { installSearchableSelects } from './src/ui/SearchableSelect.ts';
import { installSearchableInputs } from './src/ui/SearchableInput.ts';
installSearchableSelects(); installSearchableInputs();
window.product = { ErTableEditor, openBackupDialog, openSyncDialog };
`;

const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8">
<link rel="stylesheet" href="/base.css"><link rel="stylesheet" href="/select.css"></head><body><main id="root" style="padding:12px"></main>
<script type="module" src="/fixture.js"></script></body></html>`;

before(async () => {
  const bundled = await build({
    stdin: { contents: source, resolveDir: root, sourcefile: 'searchable-business-fixture.ts' },
    bundle: true, write: false, format: 'esm', platform: 'browser', loader: { '.css': 'empty' }, logLevel: 'silent',
  });
  const files = new Map([
    ['/', ['text/html', html]],
    ['/fixture.js', ['text/javascript', bundled.outputFiles[0].text]],
    ['/base.css', ['text/css', await readFile(path.join(root, 'src/styles/base.css'), 'utf8')]],
    ['/select.css', ['text/css', await readFile(path.join(root, 'src/ui/searchable-select.css'), 'utf8')]],
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
  ].find(existsSync);
  browser = await chromium.launch({ executablePath, headless: true });
}, { timeout: 45000 });

after(async () => {
  try { await browser?.close(); } finally { if (server) await new Promise(resolve => server.close(resolve)); }
});

async function withPage(run) {
  const context = await browser.newContext({ viewport: { width: 1100, height: 820 } });
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    await page.goto(origin);
    await page.waitForFunction(() => !!window.product);
    await run(page);
    assert.deepEqual(errors, []);
  } finally { await context.close(); }
}

/** 树中当前可见的叶子名称（ObjectTree.filter 只设置 row.hidden）。 */
const visibleTreeNames = page => page.evaluate(() => [...document.querySelectorAll('.backup-tree-leaf')]
  .filter(row => !row.hidden)
  .map(row => row.querySelector('.backup-tree-label')?.textContent ?? row.textContent.trim()));

/** 树中分组标题及其 hidden 状态。 */
const treeGroups = page => page.evaluate(() => [...document.querySelectorAll('.backup-tree-group')].map(group => ({
  title: group.querySelector('.backup-tree-label')?.textContent ?? '',
  hidden: group.hidden,
})));

/** 复选框勾选状态（按 aria-label 映射）。 */
const checkedState = page => page.evaluate(() => Object.fromEntries([...document.querySelectorAll('.backup-tree input[type="checkbox"]')]
  .map(input => [input.getAttribute('aria-label'), input.checked])));

test('ER 索引字段：当前字段 datalist + 逗号 token，删除字段后候选更新，自由输入保留', () => withPage(async page => {
  await page.evaluate(() => {
    window.__PREVIEW_MOCK__ = {
      read_table_design_snapshot: () => ({
        schema: {},
        objects: { checks: [], triggers: [], completeness: 'complete', limitations: [], table_definition: null },
        options: {},
        design: {
          schema: 'demo', table: 'users',
          columns: [
            { name: 'id', data_type: 'integer', nullable: false, default_value: null, auto_increment: true, comment: '' },
            { name: 'userName', data_type: 'varchar(64)', nullable: true, default_value: null, comment: '' },
            { name: 'legacy', data_type: 'text', nullable: true, default_value: null, comment: '' },
          ],
          primary_key: ['id'],
          indexes: [{ name: 'idx_user', columns: ['userName'], unique: false }],
          foreign_keys: [],
        },
        revision: 'rev-1', source_fingerprint: 'fp-1', owner_id: 'owner-1',
        completeness: 'complete', capabilities: {},
      }),
      list_tables: () => [['orders', 'table']],
    };
    const el = document.createElement('div');
    el.id = 'er-host';
    document.querySelector('#root').append(el);
    window.er = new window.product.ErTableEditor(
      { connectionId: 'c1', schema: 'demo', table: 'users', ownerId: 'owner-1' },
      { connection: () => ({ kind: 'sqlite' }), writeReason: () => null, changed() {}, toast() {} },
      el,
    );
    window.er.activate();
  });
  await page.locator('.er-fedit').first().waitFor();
  assert.equal(await page.locator('.er-fedit').count(), 3);

  // 通过真实 UI 删除第三个字段，候选应随之更新。
  await page.locator('.er-fedit').nth(2).getByTitle('删除字段').click();
  await page.getByRole('tab', { name: '索引' }).click();

  const columnInput = page.locator('[aria-label="索引字段（逗号分隔）"]');
  assert.equal(await columnInput.count(), 1);
  const wired = await page.evaluate(() => {
    const input = document.querySelector('[aria-label="索引字段（逗号分隔）"]');
    return {
      listId: input.getAttribute('list'),
      token: input.getAttribute('data-search-token'),
      disabled: input.disabled,
      options: input.list ? [...input.list.options].map(option => option.value) : null,
      tag: input.tagName,
    };
  });
  assert.equal(wired.tag, 'INPUT');
  assert.equal(wired.disabled, false, '可编辑状态下必须保持键盘可达');
  assert.equal(wired.token, ',', '逗号分隔多值 token');
  assert.ok(wired.listId, '索引字段必须关联 datalist');
  assert.deepEqual(wired.options, ['id', 'userName'], '候选为当前未删除字段');
  await columnInput.fill('id, user');
  await page.locator('.searchable-input-popover').waitFor();
  assert.deepEqual(await page.locator('.searchable-input-popover [role="option"]').allTextContents(), ['userName']);
  await page.locator('.searchable-input-popover [role="option"]').click();
  assert.equal(await columnInput.inputValue(), 'id, userName');

  // 自由输入（含表达式）不被 datalist 限制，并在重新渲染后保留。
  await columnInput.fill('userName, expr(id)');
  await page.getByRole('tab', { name: '外键' }).click();
  await page.getByRole('tab', { name: '索引' }).click();
  const colAfter = page.locator('[aria-label="索引字段（逗号分隔）"]');
  assert.equal(await colAfter.inputValue(), 'userName, expr(id)');
  assert.deepEqual(await page.evaluate(() => {
    const input = document.querySelector('[aria-label="索引字段（逗号分隔）"]');
    return [input.getAttribute('list'), input.getAttribute('data-search-token'), [...input.list.options].map(option => option.value)];
  }), [await colAfter.getAttribute('list'), ',', ['id', 'userName']]);
}));

test('备份对象树：带名称的搜索框只隐藏行，不丢勾选、不改变分组全选语义', () => withPage(async page => {
  await page.evaluate(() => {
    window.__PREVIEW_MOCK__ = {
      list_tables: () => new Promise(resolve => { window.resolveBackupTables = resolve; }),
      list_routines: () => [['function', 'fn_total'], ['procedure', 'sp_refresh']],
      create_backup: () => { throw new Error('过滤测试不应执行备份'); },
    };
    const connection = {
      id: 'c1', name: '本地 SQLite', kind: 'sqlite', host: '', port: 0, user: '', password: '',
      database: '', file: '', params: '', group: '', read_only: false, keep_alive: false,
    };
    window.backupDlg = window.product.openBackupDialog(connection, 'demo', () => {});
  });
  const search = page.getByRole('searchbox', { name: '搜索备份对象', exact: true });
  await search.fill('user');
  await page.waitForFunction(() => !!window.resolveBackupTables);
  await page.evaluate(() => window.resolveBackupTables([['users', 'table'], ['订单', 'table'], ['user_view', 'view']]));
  await page.waitForFunction(() => document.querySelectorAll('.backup-tree-leaf').length === 5);
  assert.deepEqual(await visibleTreeNames(page), ['users', 'user_view'], '加载期间输入的过滤词在列表返回后立即生效');
  await search.fill('');

  assert.equal(await search.count(), 1, '搜索框必须有可访问名称');
  assert.equal(await search.getAttribute('type'), 'search');
  await search.focus();
  assert.equal(await page.evaluate(() => document.activeElement?.getAttribute('aria-label')), '搜索备份对象');

  // 先改变两个勾选状态（一个稍后会被过滤隐藏、一个仍可见），确认过滤/清空后原样保留。
  await page.getByRole('checkbox', { name: '订单', exact: true }).uncheck();
  await page.getByRole('checkbox', { name: 'user_view', exact: true }).uncheck();
  await search.fill('user');
  assert.deepEqual(await visibleTreeNames(page), ['users', 'user_view']);
  assert.deepEqual(await treeGroups(page), [
    { title: '表', hidden: false },
    { title: '视图', hidden: false },
    { title: '函数与存储过程', hidden: true },
  ]);
  assert.deepEqual(await checkedState(page), { users: true, 订单: false, user_view: false, fn_total: true, sp_refresh: true });

  // 过滤中点击「全选」仍会对隐藏行生效（ObjectTree 既有语义不变），且不影响其他分组。
  const tablesGroup = page.locator('.backup-tree-group').nth(0);
  await tablesGroup.getByRole('button', { name: '全选', exact: true }).click();
  assert.deepEqual(await checkedState(page), { users: true, 订单: true, user_view: false, fn_total: true, sp_refresh: true });

  await search.fill('');
  assert.deepEqual(await visibleTreeNames(page), ['users', '订单', 'user_view', 'fn_total', 'sp_refresh']);
  assert.deepEqual(await checkedState(page), { users: true, 订单: true, user_view: false, fn_total: true, sp_refresh: true });

  // 无匹配时整组隐藏，清空后恢复。
  await search.fill('不存在');
  assert.deepEqual(await visibleTreeNames(page), []);
  assert.deepEqual(await treeGroups(page), [
    { title: '表', hidden: true },
    { title: '视图', hidden: true },
    { title: '函数与存储过程', hidden: true },
  ]);
  await search.fill('');
  assert.equal((await visibleTreeNames(page)).length, 5);
}));

test('同步目标列：已有表给出列名 datalist，新建表 / 元数据失败保留自由输入；整库树过滤不丢勾选', () => withPage(async page => {
  await page.evaluate(() => {
    window.__calls = [];
    const record = (command, handler) => args => { window.__calls.push([command, args]); return handler(args); };
    window.__PREVIEW_MOCK__ = {
      ensure_connection: record('ensure_connection', () => ({ ok: true, message: '' })),
      list_databases: record('list_databases', () => [['demo', 'sqlite'], ['other', 'sqlite']]),
      list_tables: record('list_tables', args => args.schemaName === 'demo'
        ? [['users', 'table'], ['orders', 'table'], ['user_view', 'view']]
        : [['other_table', 'table']]),
      sync_type_mapping: record('sync_type_mapping', () => [
        { source: 'id', source_type: 'INTEGER', target_type: 'INTEGER' },
        { source: 'name', source_type: 'VARCHAR(64)', target_type: 'VARCHAR(64)' },
      ]),
      get_table_schema: record('get_table_schema', args => {
        if (args.table === 'users') return { columns: [{ name: 'id' }, { name: 'name' }, { name: 'extra' }] };
        throw new Error('metadata unavailable');
      }),
    };
    const connection = (id, name) => ({
      id, name, kind: 'sqlite', host: '', port: 0, user: '', password: '',
      database: '', file: '', params: '', group: '', read_only: false, keep_alive: false,
    });
    const sourceConnection = connection('c1', '源 SQLite');
    const targetConnection = connection('c2', '目标 SQLite');
    window.syncDlg = window.product.openSyncDialog([sourceConnection, targetConnection], sourceConnection, { schema: 'demo', table: 'users' });
  });

  const dialog = page.locator('[role="dialog"][aria-label="数据库同步"]');
  await dialog.waitFor();
  const targetTable = page.locator('[aria-label="目标表（可输入新表名，不存在时自动创建）"]');
  await targetTable.waitFor();
  assert.equal(await targetTable.inputValue(), 'users');
  const next = dialog.locator('.wizard-actions button').nth(1);
  const back = dialog.locator('.wizard-actions button').nth(0);
  const targetColumn = page.locator('input[aria-label="id 对应目标列（留空跳过）"]');

  // 进入映射页：已有目标表，映射目标列获得真实列名候选。
  await next.click();
  await next.click();
  await targetColumn.waitFor();
  await page.waitForFunction(() => {
    const input = document.querySelector('[aria-label="id 对应目标列（留空跳过）"]');
    return !!input && !!input.list && input.list.options.length === 3;
  });
  assert.deepEqual(await page.evaluate(() => {
    const input = document.querySelector('[aria-label="id 对应目标列（留空跳过）"]');
    return [...input.list.options].map(option => option.value);
  }), ['id', 'name', 'extra']);
  assert.deepEqual(await page.evaluate(() => {
    const call = window.__calls.find(([command, args]) => command === 'get_table_schema' && args.id === 'c2');
    return call?.[1] ?? null;
  }), { id: 'c2', schemaName: 'demo', table: 'users' }, '按真实 API 形态读取目标表列');

  // 增量业务键也使用源字段建议，保留已有逗号前缀。
  const bizKey = page.locator('input[aria-label="增量同步业务键列"]');
  await bizKey.fill('id, na');
  await page.locator('.searchable-input-popover').waitFor();
  assert.deepEqual(await page.locator('.searchable-input-popover [role="option"]').allTextContents(), ['name']);
  await page.locator('.searchable-input-popover [role="option"]').click();
  assert.equal(await bizKey.inputValue(), 'id, name');

  // 使用真实搜索浮层选择一个与源列不同的目标列。
  await targetColumn.fill('ext');
  await page.locator('.searchable-input-popover').waitFor();
  assert.deepEqual(await page.locator('.searchable-input-popover [role="option"]').allTextContents(), ['extra']);
  await page.locator('.searchable-input-popover [role="option"]').click();
  assert.equal(await targetColumn.inputValue(), 'extra');

  // 自由输入仍然可用，且不会触发任何同步写入。
  await targetColumn.fill('custom_col');
  assert.equal(await targetColumn.inputValue(), 'custom_col');
  assert.equal(await page.evaluate(() => window.__calls.filter(([command]) => command === 'run_sync').length), 0);

  // 新建表：元数据读不到时静默回退为自由输入，不阻断、不显示错误。
  await back.click();
  await back.click();
  await targetTable.fill('brand_new');
  await page.waitForFunction(() => window.__calls.some(([command, args]) => command === 'get_table_schema' && args.table === 'brand_new'));
  await next.click();
  await next.click();
  await page.waitForFunction(() => {
    const input = document.querySelector('[aria-label="id 对应目标列（留空跳过）"]');
    return !!input && !input.hasAttribute('list');
  });
  assert.equal(await targetColumn.inputValue(), 'id');
  await targetColumn.fill('custom_new_col');
  assert.equal(await targetColumn.inputValue(), 'custom_new_col');
  assert.equal(await page.evaluate(() => document.querySelector('.wizard-error')?.hidden), true);
  assert.doesNotMatch(await dialog.innerText(), /metadata unavailable/);

  // 恢复已有目标表，然后切到整库模式核对对象树过滤。
  await back.click();
  await back.click();
  await targetTable.fill('users');
  await page.locator('select:has(option[value="database"])').selectOption('database');
  await next.click();
  await next.click();

  const search = page.getByRole('searchbox', { name: '搜索要同步的表与视图', exact: true });
  await search.waitFor();
  await page.waitForFunction(() => document.querySelectorAll('#sync-db-tables .backup-tree-leaf').length === 3);
  assert.equal(await search.getAttribute('type'), 'search');
  await search.focus();
  assert.equal(await page.evaluate(() => document.activeElement?.getAttribute('aria-label')), '搜索要同步的表与视图');

  // 视图默认不勾选；取消一张表的勾选后过滤，勾选状态不变。
  await page.getByRole('checkbox', { name: '同步表 orders', exact: true }).uncheck();
  await search.fill('user');
  assert.deepEqual(await visibleTreeNames(page), ['users', 'user_view']);
  assert.deepEqual(await checkedState(page), { '同步表 users': true, '同步表 orders': false, '同步视图 user_view': false });

  // 过滤状态下分组全选仍覆盖隐藏行。
  const tablesGroup = page.locator('#sync-db-tables .backup-tree-group').nth(0);
  await tablesGroup.getByRole('button', { name: '全选', exact: true }).click();
  assert.deepEqual(await checkedState(page), { '同步表 users': true, '同步表 orders': true, '同步视图 user_view': false });

  await search.fill('');
  assert.deepEqual(await visibleTreeNames(page), ['users', 'orders', 'user_view']);

  // 步骤重进后过滤关键字保留，重新渲染不会回到未过滤状态。
  await search.fill('user');
  await page.getByRole('checkbox', { name: '同步表 users', exact: true }).uncheck();
  await back.click();
  await next.click();
  const searchAgain = page.getByRole('searchbox', { name: '搜索要同步的表与视图', exact: true });
  await searchAgain.waitFor();
  await page.waitForFunction(() => document.querySelectorAll('#sync-db-tables .backup-tree-leaf').length === 3);
  assert.equal(await searchAgain.inputValue(), 'user');
  assert.deepEqual(await visibleTreeNames(page), ['users', 'user_view']);
  assert.deepEqual(await checkedState(page), { '同步表 users': false, '同步表 orders': true, '同步视图 user_view': false });

  assert.equal(await page.evaluate(() => window.__calls.filter(([command]) => command === 'run_sync' || command === 'run_sync_database').length), 0);
}));
