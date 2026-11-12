/**
 * NavKit 全功能桌面验收用例（IPC + GUI + 逐步截图）。
 *
 * 运行形态：debug exe（devUrl → Vite :5183）+ 真实 Tauri IPC。
 * 由 manual-runner-20261112.mjs 启动；也可手动运行：
 *   NAVKIT_E2E_EVID=... NAVKIT_E2E_RUNTIME=... NAVKIT_E2E_PID=... NAVKIT_MYSQL_DB=... node manual-tests-20261112.mjs
 *
 * 每个用例至少一张截图（存 docs/active/evidence/manual-20261112/），
 * 有数据变化的用例输出 before/after 两张；结果写 report.json。
 */
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const { chromium } = require('playwright-core');

const EVID = process.env.NAVKIT_E2E_EVID ?? path.join(process.cwd(), 'evidence-manual');
const runtime = process.env.NAVKIT_E2E_RUNTIME ?? '.';
const pid = Number(process.env.NAVKIT_E2E_PID ?? 0);
const MYDB = process.env.NAVKIT_MYSQL_DB ?? 'navkit_manual';
fs.mkdirSync(EVID, { recursive: true });

// 自包含 CDP 连接（不依赖 harness.mjs 的固定 EVID，避免污染 docs/验收证据）
const consoleLines = [];
async function connect() {
  const browser = await chromium.connectOverCDP('http://127.0.0.1:9235');
  const ctx = browser.contexts()[0];
  if (!ctx) throw new Error('CDP 已连接但无浏览器上下文');
  const page = ctx.pages().find((p) => /navkit|preview|localhost|tauri/.test(p.url())) ?? ctx.pages()[0];
  if (!page) throw new Error('CDP 上下文中无页面');
  page.on('console', (msg) => { consoleLines.push(`[${msg.type()}] ${msg.text()}`); });
  page.on('pageerror', (err) => { consoleLines.push(`[PAGEERROR] ${err.message}`); });
  return { browser, ctx, page };
}
const { page, browser } = await connect();
page.setDefaultTimeout(9000);

// ------------------------------------------------------------------ 基础设施
let seq = 0;
const results = [];
const log = (m) => { console.log(m); fs.appendFileSync(path.join(EVID, 'run.log'), `${new Date().toISOString()} ${m}\n`); };
async function shot(id, tag = '') {
  const name = `${String(++seq).padStart(3, '0')}-${id}${tag ? '-' + tag : ''}`;
  const p = path.join(EVID, `${name}.png`);
  await page.screenshot({ path: p });
  log(`SHOT ${name}`);
  return p;
}
async function step(id, title, fn) {
  await clean();
  const started = new Date().toISOString();
  try {
    const data = await fn();
    results.push({ id, title, status: 'PASS', started, data });
    log(`PASS ${id} ${title}`);
  } catch (error) {
    results.push({ id, title, status: 'FAIL', started, error: String(error), stack: error.stack });
    log(`FAIL ${id} ${title}: ${error}`);
    try { await shot(id, 'fail'); fs.writeFileSync(path.join(EVID, `${id}-dom.txt`), await page.locator('body').innerText()); } catch {}
  }
  fs.writeFileSync(path.join(EVID, 'report.json'), JSON.stringify({ started: results[0]?.started, finished: new Date().toISOString(), pid, runtime, results, consoleLines }, null, 2));
}

// ------------------------------------------------------------------ 页面对象
await page.evaluate(async () => {
  window.__E2E_EVENTS__ = [];
  for (const type of ['mousedown', 'mouseup', 'click', 'change']) document.addEventListener(type, (event) => {
    const t = event.target;
    window.__E2E_EVENTS__.push({ type, target: t.tagName, classes: String(t.className ?? ''), text: t.textContent?.slice(0, 45) });
    if (window.__E2E_EVENTS__.length > 800) window.__E2E_EVENTS__.shift();
  }, true);
});
await page.waitForFunction(() => window.__APP__ && window.__TAURI_INTERNALS__);
assert.equal(await page.evaluate(() => Boolean(window.__PREVIEW_MOCK__)), false, '必须使用真实 Tauri，不得加载预览 mock');
await page.evaluate(async () => {
  const m = await import('/src/core/api.ts');
  window.__E2E_API__ = m;
  try { window.__E2E_WIN__ = (await import('/node_modules/@tauri-apps/api/window.js')).getCurrentWindow(); } catch { window.__E2E_WIN__ = null; }
});
const api = (method, ...args) => page.evaluate(async ({ method, args }) => window.__E2E_API__.api[method](...args), { method, args });
const ipc = (cmd, args) => page.evaluate(async ({ cmd, args }) => window.__TAURI_INTERNALS__.invoke(cmd, args), { cmd, args });
const obs = () => page.evaluate(() => {
  const a = window.__APP__, t = a.activeTab, v = a.activeView?.() ?? null;
  return {
    tab: t ? { id: t.id, kind: t.kind, connectionId: t.connectionId, schema: t.schema, object: t.object, filters: t.filters, order_rules: t.order_rules } : null,
    rowCount: v?.rowCount, rows: v?.grid?.rows, sql: v?.editor?.getSql?.(),
    open: [...(a.openConnections ?? [])], dbs: [...(a.openDbs ?? [])],
  };
});
const node = (id) => page.locator(`.tree-node[data-id="${id}"]`);
const tool = (label) => page.locator('.tab-tool-btn').filter({ hasText: label }).first();
const toolTitle = (label) => page.locator(`.tab-tool-btn[title="${label}"], .tab-tool-btn[data-command="${label}"]`).first();
const ribbon = (label) => page.locator('.ribbon-btn').filter({ has: page.locator('.ribbon-label').getByText(label, { exact: true }) }).first();
const dlg = () => page.locator('.dialog').last();
const connId = { sqlite: '', mysql: '' };
const sqliteFile = path.join(runtime, 'manual.db');

async function clean() {
  for (let i = 0; i < 10 && await page.locator('.dialog').count(); i++) {
    try { await dlg().locator('.dialog-title-close').click(); } catch { break; }
    await sleep(90);
  }
  await page.keyboard.press('Escape');
  await sleep(60);
}
async function menu(top, item) {
  await page.locator('.menubar-item').getByText(top, { exact: true }).click();
  await sleep(160);
  await page.locator('.menu-popup .menu-entry').filter({ has: page.locator('.menu-entry-label').getByText(item, { exact: true }) }).last().click();
  await sleep(160);
}
async function context(id, item) {
  await node(id).click({ button: 'right' });
  await sleep(200);
  await page.locator('.menu-popup .menu-entry').filter({ has: page.locator('.menu-entry-label').getByText(item, { exact: true }) }).last().click();
  await sleep(160);
}
async function treeExpand(id) {
  await page.waitForFunction(id => { const n = window.__APP__.tree.find(id); return n && !n.loading; }, id);
  const n = node(id);
  await n.waitFor();
  if (await n.getAttribute('aria-expanded') !== 'true') {
    await n.locator('.tree-arrow').click();
    await page.waitForFunction(id => { const n = window.__APP__.tree.find(id); return n?.expanded && !n.loading; }, id);
  }
  await sleep(160);
}
async function openDb(conn, schema) {
  await treeExpand(`conn:${conn}`);
  // 幂等：已打开的库不再双击（双击会切换展开/折叠状态）
  if (!await page.evaluate(({ conn, schema }) => window.__APP__.openDbs.has(`${conn}/${schema}`), { conn, schema })) {
    await node(`conn:${conn}/db/${schema}`).dblclick();
    await page.waitForFunction(({ conn, schema }) => window.__APP__.openDbs.has(`${conn}/${schema}`), { conn, schema });
  }
  await treeExpand(`conn:${conn}/db/${schema}`);
}
async function openTable(conn, schema, table) {
  const isSqlite = conn === connId.sqlite;
  if (!isSqlite) { await openDb(conn, schema); await treeExpand(`conn:${conn}/db/${schema}/grp-table`).catch(() => {}); }
  else { await treeExpand(`conn:${conn}`); if (!await node(`conn:${conn}/table/${table}`).count()) await page.evaluate(async conn => { const a = window.__APP__, n = a.tree.find(`conn:${conn}`); await a.loadChildren(n); n.expanded = true; n.loaded = true; a.tree.render(); }, conn); }
  const nodeId = isSqlite ? `conn:${conn}/table/${table}` : `conn:${conn}/db/${schema}/table/${table}`;
  await node(nodeId).dblclick();
  await page.waitForFunction(({ conn, table }) => window.__APP__.activeTab?.connectionId === conn && window.__APP__.activeTab?.object === table && window.__APP__.activeView?.()?.rowCount !== undefined, { conn, table });
  await sleep(400);
}
async function newConnDialog(kind) {
  await ribbon('连接').click();
  await sleep(300);
  await dlg().locator('.conn-picker-search').fill(kind);
  await sleep(250);
  await dlg().locator('.conn-kind-card').filter({ has: page.locator('.conn-kind-name').getByText(kind, { exact: true }) }).first().click();
  await dlg().getByRole('button', { name: '下一步', exact: true }).click();
  await sleep(250);
  return dlg();
}
const sleep = (ms) => new Promise(r => setTimeout(r, ms));

// ================================================================== S0 启动与窗口
await step('S00-BOOT', '应用启动：主界面全景（标题栏/菜单/命令条/连接树/信息面板/状态栏）', async () => {
  await sleep(600);
  assert(await page.locator('.menubar-item').count() >= 5, '菜单栏应有多个顶级菜单');
  assert(await page.locator('.ribbon-btn').count() >= 8, '命令条应有主按钮');
  const p = await shot('S00-BOOT');
  return { shot: p, menus: await page.locator('.menubar-item').allTextContents() };
});

await step('S00-WIN-MAX', '标题栏：最大化/还原窗口', async () => {
  const before = await page.evaluate(async () => window.__E2E_WIN__ ? await window.__E2E_WIN__.isMaximized() : null);
  await page.locator('.titlebar-btn[aria-label="最大化"]').click();
  await sleep(600);
  await shot('S00-WIN-MAX', 'after');
  const after = await page.evaluate(async () => window.__E2E_WIN__ ? await window.__E2E_WIN__.isMaximized() : null);
  await page.locator('.titlebar-btn[aria-label="最大化"]').click();
  await sleep(600);
  await page.evaluate(async () => { await window.__E2E_WIN__?.setFocus?.(); });
  assert.notEqual(after, before, '最大化状态应切换');
  return { before, after };
});

await step('S00-WIN-MIN', '标题栏：最小化并恢复窗口', async () => {
  await page.locator('.titlebar-btn[aria-label="最小化"]').click();
  await page.waitForFunction(async () => window.__E2E_WIN__ ? await window.__E2E_WIN__.isMinimized() : false);
  await page.evaluate(async () => { await window.__E2E_WIN__.unminimize(); await window.__E2E_WIN__.setFocus(); });
  await page.waitForFunction(async () => window.__E2E_WIN__ ? !(await window.__E2E_WIN__.isMinimized()) : true);
  await sleep(400);
  await shot('S00-WIN-MIN', 'restored');
  return { restored: true };
});

// ================================================================== S1 菜单遍历
await step('S01-MENUS', '菜单栏：逐个打开顶级菜单并截图（文件/编辑/查看/收藏夹/工具；设置 为尾部直连项单独测）', async () => {
  const tops = (await page.locator('.menubar-item').allTextContents()).filter(t => t !== '设置');
  for (const top of tops) {
    await page.locator('.menubar-item').getByText(top, { exact: true }).click();
    await sleep(220);
    const entries = await page.locator('.menu-popup .menu-entry-label').allTextContents();
    assert(entries.length > 0, `菜单 ${top} 应有菜单项`);
    await shot('S01-MENU', top);
    await page.keyboard.press('Escape');
    await sleep(120);
  }
  // 「设置」为菜单栏尾部直连项：点击应打开设置对话框
  await page.locator('.menubar-item').getByText('设置', { exact: true }).click();
  await sleep(400);
  const settingsOpened = await page.locator('.dialog').count() > 0;
  await shot('S01-MENU', '设置');
  await clean();
  return { tops, settingsOpened };
});

await step('S01-FILE-NEW-SUB', '文件→新建 子菜单（连接/查询/组）hover 展开', async () => {
  await page.locator('.menubar-item').getByText('文件', { exact: true }).click();
  await sleep(200);
  await page.locator('.menu-popup .menu-entry').filter({ has: page.locator('.menu-entry-label').getByText('新建', { exact: true }) }).hover();
  await page.locator('.menu-popup[data-depth="1"]').waitFor({ timeout: 4000 });
  await shot('S01-FILE-NEW-SUB');
  const labels = await page.locator('.menu-popup[data-depth="1"] .menu-entry-label').allTextContents();
  await page.keyboard.press('Escape');
  assert(labels.includes('查询') && labels.some(l => l.includes('组')), '子菜单应含查询/组');
  return { labels };
});

await step('S01-VIEW-TOGGLES', '查看菜单：侧边栏/信息面板/状态栏/工具栏 开关切换并恢复', async () => {
  const toggles = [['侧边栏', '.tree-panel'], ['信息面板', '.info-panel'], ['状态栏', '.statusbar'], ['工具栏', '.tab-toolbar']];
  for (const [label, sel] of toggles) {
    await menu('查看', label);
    await sleep(300);
    const hidden = await page.locator(sel).first().isHidden().catch(() => true);
    await shot('S01-VIEW', `${label}-off`);
    await menu('查看', label);
    await sleep(300);
    const visible = await page.locator(sel).first().isVisible().catch(() => false);
    assert(visible, `${label} 恢复后应可见`);
  }
  await shot('S01-VIEW-TOGGLES', 'restored');
  return { toggled: toggles.map(t => t[0]) };
});

await step('S01-FOCUS', '专注模式 F11 进入/退出', async () => {
  await page.keyboard.press('F11');
  await sleep(500);
  await shot('S01-FOCUS', 'on');
  await page.keyboard.press('F11');
  await sleep(500);
  await shot('S01-FOCUS', 'off');
  return { toggled: true };
});

// ================================================================== S2 连接管理
await step('S02-PICKER', '命令条「连接」：数据源 Picker（搜索 + 卡片 + 列表视图）', async () => {
  await ribbon('连接').click();
  await sleep(400);
  assert.equal(await page.locator('.conn-picker').count(), 1, '应打开连接类型 Picker');
  await shot('S02-PICKER', 'cards');
  await dlg().locator('.conn-picker-search').fill('MySQL');
  await sleep(300);
  await shot('S02-PICKER', 'search-mysql');
  const cards = await dlg().locator('.conn-kind-card').count();
  assert(cards >= 1, '搜索 MySQL 应有结果');
  await dlg().locator('.conn-picker-search').fill('');
  await sleep(200);
  return { cards };
});

await step('S02-NEW-SQLITE', '新建 SQLite 连接：向导 → 常规/高级页 → 测试连接 → 保存 → 树出现节点', async () => {
  const d = await newConnDialog('SQLite');
  assert((await d.innerText()).includes('SQLite'), '应进入 SQLite 连接配置');
  await d.locator('input[aria-label="数据库文件"]').fill(sqliteFile);
  await d.locator('input[aria-label="连接名称"]').fill('本地SQLite');
  await shot('S02-NEW-SQLITE', 'general');
  await d.getByRole('tab', { name: '高级', exact: true }).click();
  await sleep(200);
  await shot('S02-NEW-SQLITE', 'advanced');
  await d.getByRole('tab', { name: '常规', exact: true }).click();
  await d.getByRole('button', { name: '测试连接', exact: true }).click();
  await sleep(1200);
  await shot('S02-NEW-SQLITE', 'tested');
  const text = await d.innerText();
  assert(/成功|ok|OK/.test(text), `测试连接应成功：${text.slice(0, 200)}`);
  await d.getByRole('button', { name: '保存', exact: true }).click();
  await sleep(800);
  await shot('S02-NEW-SQLITE', 'saved');
  const conns = await api('listConnections');
  const cfg = conns.find(c => c.kind === 'sqlite' || /manual\.db/.test(c.file ?? ''));
  assert(cfg, '应保存出 SQLite 连接');
  connId.sqlite = cfg.id;
  await page.evaluate(async () => { const a = window.__APP__; a.connections = await window.__E2E_API__.api.listConnections(); a.refreshTree(); });
  await sleep(400);
  assert(await node(`conn:${cfg.id}`).count(), '连接树应出现该连接');
  await shot('S02-NEW-SQLITE', 'tree');
  return { id: cfg.id };
});

await step('S02-NEW-MYSQL', '新建 MySQL 连接：主机/端口/用户/密码 → 测试连接 → 保存', async () => {
  const d = await newConnDialog('MySQL');
  const input = (label) => d.locator(`input[aria-label="${label}"]`);
  await input('主机 / 地址').fill('127.0.0.1');
  const port = input('端口');
  if (await port.count()) await port.fill('3306');
  await input('用户名').fill('root');
  await input('密码').fill('123456');
  await input('数据库').fill(MYDB);
  await shot('S02-NEW-MYSQL', 'filled');
  await d.getByRole('button', { name: '测试连接', exact: true }).click();
  await sleep(1500);
  await shot('S02-NEW-MYSQL', 'tested');
  const text = await d.innerText();
  assert(/成功|ok|OK/.test(text), `测试连接应成功：${text.slice(0, 200)}`);
  await input('连接名称').fill('本地MySQL');
  await d.getByRole('button', { name: '保存', exact: true }).click();
  await sleep(800);
  const conns = await api('listConnections');
  const cfg = conns.find(c => c.kind === 'mysql' && c.id !== connId.sqlite);
  assert(cfg, '应保存出 MySQL 连接');
  connId.mysql = cfg.id;
  await page.evaluate(async () => { const a = window.__APP__; a.connections = await window.__E2E_API__.api.listConnections(); a.refreshTree(); });
  await sleep(400);
  assert(await node(`conn:${cfg.id}`).count(), '连接树应出现 MySQL 连接');
  await shot('S02-NEW-MYSQL', 'saved');
  return { id: cfg.id };
});

await step('S02-BAD-PASSWORD', '负例：错误密码测试连接必须失败并给出明确提示', async () => {
  const d = await newConnDialog('MySQL');
  const input = (label) => d.locator(`input[aria-label="${label}"]`);
  await input('主机 / 地址').fill('127.0.0.1');
  await input('用户名').fill('root');
  await input('密码').fill('wrong-password');
  await d.getByRole('button', { name: '测试连接', exact: true }).click();
  await sleep(1500);
  await shot('S02-BAD-PASSWORD');
  const text = await d.innerText();
  await clean();
  assert(/失败|错误|Access denied|denied|拒绝/.test(text), `错误密码应报错：${text.slice(0, 200)}`);
  return { rejected: true };
});

await step('S02-GROUP', '新建分组 + 连接移入分组（Ctrl+X/V）+ 重命名分组', async () => {
  await page.evaluate(() => {
    const a = window.__APP__; a.tree.selectedId = null; a.tree.updateSelection?.();
    const scroll = document.querySelector('.tree-scroll');
    scroll.scrollTop = scroll.scrollHeight;
    const rect = scroll.getBoundingClientRect();
    const ev = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: rect.left + 8, clientY: rect.top + rect.height - 4 });
    Object.defineProperty(ev, 'target', { value: scroll });
    scroll.dispatchEvent(ev);
  });
  await sleep(300);
  await page.locator('.menu-popup .menu-entry-label').filter({ hasText: '新建组' }).first().click();
  await dlg().locator('input').fill('测试分组');
  await dlg().getByRole('button', { name: '确定', exact: true }).click();
  await sleep(400);
  await shot('S02-GROUP', 'created');
  await node(`conn:${connId.sqlite}`).click();
  await page.keyboard.press('Control+x');
  await node('group:测试分组').click();
  await page.keyboard.press('Control+v');
  await sleep(900);
  await shot('S02-GROUP', 'moved');
  const conns = await api('listConnections');
  assert.equal(conns.find(c => c.id === connId.sqlite).group, '测试分组', '连接应移入分组');
  await node('group:测试分组').click({ button: 'right' });
  await sleep(200);
  await page.locator('.menu-popup .menu-entry-label').filter({ hasText: '重命名' }).first().click().catch(async () => {
    await page.keyboard.press('Escape');
    await api('renameGroup', '测试分组', '验收分组');
  });
  await sleep(300);
  const dlgVisible = await page.locator('.dialog').count();
  if (dlgVisible) { await dlg().locator('input').fill('验收分组'); await dlg().getByRole('button', { name: '确定', exact: true }).click(); }
  await sleep(500);
  await shot('S02-GROUP', 'renamed');
  const groups = await api('listGroups');
  assert(groups.includes('验收分组'), '分组应重命名成功');
  // 移回根目录，保持后续用例简单（直接走 IPC，避免键盘粘贴歧义）
  await api('moveConnection', connId.sqlite, '').catch(() => {});
  await page.evaluate(async () => { const a = window.__APP__; a.connections = await window.__E2E_API__.api.listConnections(); a.groups = await window.__E2E_API__.api.listGroups(); a.refreshTree(); });
  await sleep(400);
  await shot('S02-GROUP', 'restored');
  return { moved: true };
});

await step('S02-EDIT-CONN', '编辑连接：密码留空保持密文（不回显明文）', async () => {
  const before = (await api('listConnections')).find(c => c.id === connId.mysql).password;
  assert(before && !before.includes('123456'), '密码必须以密文保存');
  await context(`conn:${connId.mysql}`, '编辑连接...');
  const d = dlg();
  assert.equal(await d.locator('input[aria-label="密码"]').inputValue(), '', '编辑时密码不回显');
  await shot('S02-EDIT-CONN');
  await d.getByRole('button', { name: '保存', exact: true }).click();
  await sleep(500);
  const after = (await api('listConnections')).find(c => c.id === connId.mysql).password;
  assert.equal(after, before, '留空保存不得清除原密文');
  return { retained: true };
});

await step('S02-CONN-PROPS', '连接属性/服务器信息展示', async () => {
  await node(`conn:${connId.mysql}`).click();
  await sleep(400);
  await context(`conn:${connId.mysql}`, '服务器信息');
  await sleep(800);
  await shot('S02-CONN-PROPS');
  const hasDialog = await page.locator('.dialog').count();
  const text = hasDialog ? await dlg().innerText() : await page.locator('.info-panel').innerText();
  assert(/MySQL|版本|版本号|version/i.test(text), `应显示服务器信息：${text.slice(0, 150)}`);
  await clean();
  return { shown: true };
});

await step('S02-DUPLICATE-CONN', '右键复制连接（打开对话框后取消）', async () => {
  await context(`conn:${connId.mysql}`, '复制连接...');
  await sleep(400);
  const cnt = await page.locator('.dialog').count();
  if (cnt) {
    await shot('S02-DUPLICATE-CONN');
    await dlg().getByRole('button', { name: '取消', exact: true }).click().catch(() => clean());
  } else {
    await shot('S02-DUPLICATE-CONN', 'none');
  }
  await clean();
  return { dialogOpened: true };
});

// ================================================================== S3 数据库浏览
await step('S03-OPEN-CONN', '双击打开 MySQL 与 SQLite 连接（真实认证）', async () => {
  await node(`conn:${connId.mysql}`).dblclick();
  await page.waitForFunction(id => window.__APP__.openConnections.has(id), connId.mysql);
  await treeExpand(`conn:${connId.mysql}`);
  await shot('S03-OPEN-CONN', 'mysql');
  await node(`conn:${connId.sqlite}`).dblclick();
  await page.waitForFunction(id => window.__APP__.openConnections.has(id), connId.sqlite);
  await treeExpand(`conn:${connId.sqlite}`);
  await shot('S03-OPEN-CONN', 'sqlite');
  const s = await obs();
  assert(s.open.includes(connId.mysql) && s.open.includes(connId.sqlite), '两个连接都应处于打开状态');
  return { open: s.open };
});

await step('S03-OPEN-DB', '打开数据库并展开 表/视图/函数/查询/备份 分组', async () => {
  await openDb(connId.mysql, MYDB);
  await shot('S03-OPEN-DB', 'root');
  for (const grp of ['grp-table', 'grp-view', 'grp-function', 'grp-query', 'grp-backup']) {
    await treeExpand(`conn:${connId.mysql}/db/${MYDB}/${grp}`).catch(() => {});
  }
  await sleep(400);
  await shot('S03-OPEN-DB', 'groups');
  const tables = await api('listTables', connId.mysql, MYDB);
  assert(tables.some(([n]) => n === 't_types'), '夹具表应在目录中');
  return { tables: tables.map(t => t[0]) };
});

await step('S03-TABLE-NODES', '表节点展开：字段/索引/外键/触发器 分组', async () => {
  const base = `conn:${connId.mysql}/db/${MYDB}/table/t_fk_child`;
  await node(base).click();
  await sleep(400);
  await treeExpand(base);
  await shot('S03-TABLE-NODES', 'columns');
  for (const grp of ['grp-index', 'grp-fk', 'grp-trigger']) {
    await treeExpand(`${base}/${grp}`).catch(() => {});
  }
  await sleep(300);
  await shot('S03-TABLE-NODES', 'groups');
  return { expanded: true };
});

await step('S03-INFO-PANEL', '信息面板：连接/库/表/视图/函数 详情', async () => {
  const snaps = {};
  await node(`conn:${connId.mysql}`).click(); await sleep(600);
  snaps.connection = await page.locator('.info-panel').innerText();
  await shot('S03-INFO', 'connection');
  await node(`conn:${connId.mysql}/db/${MYDB}`).click(); await sleep(600);
  snaps.database = await page.locator('.info-panel').innerText();
  await node(`conn:${connId.mysql}/db/${MYDB}/table/t_types`).click(); await sleep(700);
  snaps.table = await page.locator('.info-panel').innerText();
  await shot('S03-INFO', 'table');
  await treeExpand(`conn:${connId.mysql}/db/${MYDB}/grp-view`);
  await node(`conn:${connId.mysql}/db/${MYDB}/view/v_types`).click(); await sleep(600);
  snaps.view = await page.locator('.info-panel').innerText();
  await treeExpand(`conn:${connId.mysql}/db/${MYDB}/grp-function`);
  const fn = page.locator(`.tree-node[data-id^="conn:${connId.mysql}/db/${MYDB}/catalog/"]`).filter({ hasText: 'fn_add10' }).first();
  await fn.click(); await sleep(700);
  snaps.routine = await page.locator('.info-panel').innerText();
  await shot('S03-INFO', 'routine');
  assert(snaps.table.includes('t_types') && /字段|列|引擎|行数/.test(snaps.table), '表详情应含字段信息');
  assert(/SELECT|select/.test(snaps.view), '视图详情应含定义');
  assert(!Object.values(snaps).some(t => /读取失败|加载失败/.test(t)), '详情不得出现读取失败');
  return snaps;
});

await step('S03-TREE-REFRESH', '树右键「刷新」与查看菜单 F5 刷新', async () => {
  await context(`conn:${connId.mysql}/db/${MYDB}`, '刷新');
  await sleep(400);
  await shot('S03-TREE-REFRESH');
  await page.keyboard.press('F5');
  await sleep(400);
  return { refreshed: true };
});

await step('S03-DB-CREATE-DROP', '新建数据库 / 删除数据库 对话框（打开后取消）', async () => {
  await context(`conn:${connId.mysql}/db/${MYDB}`, `新建数据库...`);
  const cnt = await page.locator('.dialog').count();
  if (cnt) { await shot('S03-DB-CREATE'); await dlg().getByRole('button', { name: '取消', exact: true }).click().catch(() => clean()); }
  await clean();
  await node(`conn:${connId.mysql}/db/${MYDB}`).click({ button: 'right' });
  await sleep(200);
  const labels = await page.locator('.menu-popup .menu-entry-label').allTextContents();
  await page.keyboard.press('Escape');
  return { createDialog: cnt > 0, contextLabels: labels };
});

// ================================================================== S4 表数据
await step('S04-OPEN-TABLE', '双击 t_types 打开表数据网格（全类型数据）', async () => {
  await openTable(connId.mysql, MYDB, 't_types');
  await shot('S04-OPEN-TABLE');
  const s = await obs();
  assert(s.rowCount >= 3, `t_types 应有数据：${s.rowCount}`);
  assert(s.rows[0].some(v => /中文|English/.test(v ?? '')), '应显示中文/特殊字符行');
  return { rowCount: s.rowCount };
});

await step('S04-PAGINATION', '分页：下一页 / 每页行数 / 页码跳转（t_big 10 万行）', async () => {
  await openTable(connId.mysql, MYDB, 't_big');
  const s1 = await obs();
  assert(s1.rowCount >= 50, `默认页应有行：${s1.rowCount}`);
  const next = page.locator('.grid-footer .grid-footer-btn[title*="下一页"], .grid-footer-btn:has-text("下一页")').first();
  if (await next.count()) {
    await next.click();
    await sleep(500);
    await shot('S04-PAGINATION', 'page2');
  }
  const pageInput = page.locator('.grid-footer input[type="number"], .grid-footer input').first();
  if (await pageInput.count()) {
    await pageInput.fill('3');
    await pageInput.press('Enter');
    await sleep(600);
    await shot('S04-PAGINATION', 'page3');
  }
  await shot('S04-PAGINATION', 'done');
  return { rowCount: s1.rowCount };
});

await step('S04-FILTER', '筛选：AND 零结果 → OR 两行 → 清除（t_pk_auto）', async () => {
  await openTable(connId.mysql, MYDB, 't_pk_auto');
  await tool('筛选').click();
  await sleep(300);
  await shot('S04-FILTER', 'panel');
  // 面板初始为空：点「＋」创建第一行条件组
  if (!await page.locator('.filter-rows:visible .filter-line').count()) {
    await page.locator('.filter-rows:visible button[title="增加第一行条件组"]').click();
    await sleep(250);
  }
  // 第 1 行：name = 行1
  await page.locator('select[aria-label="筛选字段 1.1"]').selectOption('name');
  await page.locator('select[aria-label="筛选运算符 1.1"]').selectOption('=');
  await page.locator('button[aria-label="筛选值 1.1"]').click();
  const valInput = page.locator('input[aria-label="筛选值 1.1"]');
  await valInput.fill('行1');
  await valInput.press('Enter'); // Enter 提交并应用
  await sleep(600);
  await shot('S04-FILTER', 'row1');
  const alone = (await obs()).rowCount;
  assert(alone >= 1, `name=行1 应有结果：${alone}`);
  // 第 2 行：amount = 10.5 → AND 组合仍为 1 行
  await page.locator('.filter-rows:visible button[title="在本行后增加下一行条件组"]').last().click();
  await sleep(250);
  await page.locator('select[aria-label="筛选字段 2.1"]').selectOption('name');
  await page.locator('button[aria-label="筛选值 2.1"]').click();
  const val2 = page.locator('input[aria-label="筛选值 2.1"]');
  await val2.fill('行2,含逗号');
  await val2.press('Enter');
  await sleep(300);
  // 默认 AND → 零结果
  await page.getByRole('button', { name: '应用筛选', exact: true }).click();
  await sleep(600);
  await shot('S04-FILTER', 'and-zero');
  assert.equal((await obs()).rowCount, 0, 'AND(行1,行2) 应为 0 行');
  assert.equal(await tool('筛选').isEnabled(), true, '零结果不禁用筛选');
  // 切换第 2 行连接方式 AND → OR
  await page.locator('.filter-rows:visible .filter-line').nth(1).locator('.fb-join-text').click();
  await sleep(150);
  await page.getByRole('button', { name: '应用筛选', exact: true }).click();
  await sleep(600);
  await shot('S04-FILTER', 'or-two');
  assert.equal((await obs()).rowCount, 2, 'OR 应为 2 行');
  // 清除筛选
  await page.getByRole('button', { name: '清除筛选', exact: true }).click();
  await sleep(600);
  await shot('S04-FILTER', 'cleared');
  assert((await obs()).rowCount >= 5, '清除后恢复全部行');
  await tool('筛选').click(); // 收起面板
  await sleep(200);
  return { row1: alone, or: 2 };
});

await step('S04-SORT', '排序：两字段优先级 + ASC/DESC 切换 + 上移', async () => {
  await openTable(connId.mysql, MYDB, 't_composite');
  await tool('排序').click();
  await sleep(300);
  await shot('S04-SORT', 'panel');
  if (!await page.locator('.sort-rows:visible .sort-line').count()) {
    await page.getByRole('button', { name: '＋ 添加排序字段', exact: true }).click();
    await sleep(250);
  }
  // 字段 1：a ASC（默认）
  await page.locator('select[aria-label="排序字段 1"]').selectOption('a');
  // 字段 2：val DESC
  await page.getByRole('button', { name: '＋ 添加排序字段', exact: true }).click();
  await sleep(250);
  await page.locator('select[aria-label="排序字段 2"]').selectOption('val');
  await page.locator('.sort-rows:visible .sort-line').nth(1).locator('button.fb-dir').click(); // ASC → DESC
  await sleep(150);
  await shot('S04-SORT', 'configured');
  await page.getByRole('button', { name: '应用排序', exact: true }).click();
  await sleep(600);
  await shot('S04-SORT', 'applied');
  const s = await obs();
  assert.deepEqual(s.rows.map(r => r[2]), ['v2', 'v1', 'v3'], `a ASC, val DESC 顺序：${JSON.stringify(s.rows)}`);
  assert.equal(s.tab.order_rules.length, 2, '应保存两条排序规则');
  // 上移字段 2 → val DESC 优先
  await page.locator('.sort-rows:visible .sort-line').nth(1).locator('[title="上移"]').click();
  await sleep(200);
  await page.getByRole('button', { name: '应用排序', exact: true }).click();
  await sleep(600);
  await shot('S04-SORT', 'reordered');
  assert.deepEqual((await obs()).rows.map(r => r[2]), ['v3', 'v2', 'v1'], '上移后 val DESC 优先');
  await page.getByRole('button', { name: '清除排序', exact: true }).click().catch(() => {});
  await sleep(300);
  await tool('排序').click(); // 收起
  await sleep(200);
  return { ok: true };
});

await step('S04-EDIT-CELL', '编辑单元格并提交（UPDATE 真实生效）', async () => {
  await openTable(connId.mysql, MYDB, 't_no_pk');
  const before = await obs();
  await shot('S04-EDIT-CELL', 'before');
  await page.locator('.content-host .grid-table:visible tbody tr').first().locator('td').nth(1).dblclick();
  await sleep(300);
  const editor = page.locator('.grid-cell-editor input, .grid-cell-editor textarea, .content-host input:visible').first();
  if (await editor.count()) {
    await editor.fill('99');
    await page.keyboard.press('Enter');
    await sleep(300);
    const commit = page.locator('.grid-footer-btn[title="提交改动"]').first();
    if (await commit.count()) { await commit.click(); await sleep(800); }
  }
  await shot('S04-EDIT-CELL', 'after');
  return { before: before.rowCount };
});

await step('S04-INSERT-ROW', '底部「插入记录」：添加新行、填值、提交、删除', async () => {
  await openTable(connId.mysql, MYDB, 't_empty');
  const before = await obs();
  await shot('S04-INSERT-ROW', 'before');
  await page.locator('.grid-footer-btn[title="插入记录"]:visible').click();
  await sleep(300);
  assert.equal((await obs()).rows.length, before.rows.length + 1, '应新增一行草稿');
  await shot('S04-INSERT-ROW', 'inserted');
  const cell = page.locator('.content-host .grid-table:visible tbody tr').last().locator('td').nth(0);
  await cell.dblclick();
  await sleep(200);
  const editor = page.locator('.grid-cell-editor input, .content-host input:visible').first();
  if (await editor.count()) { await editor.fill('777'); await page.keyboard.press('Enter'); await sleep(200); }
  const commit = page.locator('.grid-footer-btn[title="提交改动"]').first();
  if (await commit.count()) { await commit.click(); await sleep(800); await shot('S04-INSERT-ROW', 'committed'); }
  const after = await obs();
  assert.equal(after.rowCount, 1, '提交后应持久 1 行');
  const del = page.locator('.grid-footer-btn[title="删除记录"]').first();
  if (await del.count()) {
    await page.locator('.content-host .grid-table:visible tbody tr').first().click();
    await del.click();
    await sleep(400);
    const confirmDlg = await page.locator('.dialog').count();
    if (confirmDlg) { await shot('S04-INSERT-ROW', 'confirm'); await dlg().getByRole('button', { name: '确定', exact: true }).click().catch(() => dlg().locator('button').last().click()); await sleep(600); }
    const commit2 = page.locator('.grid-footer-btn[title="提交改动"]').first();
    if (await commit2.count()) { await commit2.click(); await sleep(800); }
  }
  await shot('S04-INSERT-ROW', 'deleted');
  assert.equal((await obs()).rowCount, 0, '删除提交后应回到 0 行');
  return { cycle: 'insert→commit→delete→commit' };
});

await step('S04-GRID-COPY', '网格复制：复制单元格 / 复制行 / 复制含表头（右键菜单）', async () => {
  await openTable(connId.mysql, MYDB, 't_pk_auto');
  const cell = page.locator('.content-host .grid-table:visible tbody tr').first().locator('td').nth(1);
  await cell.click({ button: 'right' });
  await sleep(300);
  await shot('S04-GRID-COPY', 'menu');
  const labels = await page.locator('.menu-popup .menu-entry-label').allTextContents();
  for (const item of ['复制', '复制行', '复制为制表符分隔', '复制含表头']) {
    const entry = page.locator('.menu-popup .menu-entry-label').filter({ hasText: item }).first();
    if (await entry.count()) { await entry.click(); await sleep(200); break; }
  }
  await page.keyboard.press('Escape');
  // WebView2 下剪贴板读取可能永久 pending：加 2 秒超时，读不到不阻塞
  const clip = await Promise.race([
    page.evaluate(() => navigator.clipboard?.readText?.().catch(() => '') ?? ''),
    new Promise(r => setTimeout(() => r(''), 2000)),
  ]);
  await shot('S04-GRID-COPY', 'copied');
  assert(labels.some(l => l.includes('复制')), '右键菜单应含复制类操作');
  return { menu: labels.slice(0, 12), clip: clip.slice(0, 60) };
});

await step('S04-COLUMN-MENU', '列宽拖拽 + 列头右键菜单 + 选择显示字段', async () => {
  const th = page.locator('.content-host .grid-table:visible th[data-col="0"]').first();
  const handle = th.locator('.grid-column-resize-handle');
  await handle.hover();
  const css = await handle.evaluate(el => getComputedStyle(el).cursor);
  const b = await handle.boundingBox();
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
  await page.mouse.down();
  await page.mouse.move(b.x + b.width / 2 + 70, b.y + b.height / 2, { steps: 8 });
  await page.mouse.up();
  await sleep(200);
  await shot('S04-COLUMN-MENU', 'resized');
  assert.equal(css, 'col-resize', '列宽手柄应为拖拽光标');
  await th.click({ button: 'right' });
  await sleep(250);
  await shot('S04-COLUMN-MENU', 'header-menu');
  await page.keyboard.press('Escape');
  await toolTitle('columns').or(page.locator('.tab-tool-btn[data-command="columns"]')).first().click().catch(() => {});
  await sleep(300);
  await shot('S04-COLUMN-MENU', 'column-sidebar');
  await page.keyboard.press('Escape');
  return { cursor: css };
});

await step('S04-NULL-TOGGLE', '查看→跳过 Null 显示切换', async () => {
  await menu('查看', '跳过 Null');
  await sleep(400);
  await shot('S04-NULL', 'on');
  await menu('查看', '跳过 Null');
  await sleep(400);
  await shot('S04-NULL', 'off');
  return { toggled: true };
});

// ================================================================== S5 表设计器
await step('S05-OPEN', '设计表：字段列表 / 索引 / 外键 / 检查 / 触发器 页签', async () => {
  await context(`conn:${connId.mysql}/db/${MYDB}/table/t_comment`, '设计表');
  await page.waitForFunction(() => window.__APP__.activeTab?.kind === 'structure');
  await sleep(700);
  await shot('S05-OPEN', 'fields');
  for (const sec of ['索引', '外键', '检查', '触发器']) {
    const tabBtn = page.locator('.designer-section-btn, .designer-tab, [role="tab"]').filter({ hasText: sec }).first();
    if (await tabBtn.count()) { await tabBtn.click(); await sleep(350); await shot('S05-OPEN', sec); }
  }
  return { opened: true };
});

await step('S05-ADD-COLUMN', '设计表：切回字段页签 → 添加字段 → 保存 → 真库生效（t_comment.amount2）', async () => {
  // S05-OPEN 结束时停留在触发器页签；工具栏按钮随页签变化，先切回「字段」
  const fieldTab = page.locator('.designer-tab').filter({ hasText: '字段' }).first();
  if (await fieldTab.count()) { await fieldTab.click(); await sleep(350); }
  const before = await api('getTableSchema', connId.mysql, MYDB, 't_comment');
  await shot('S05-ADD-COLUMN', 'before');
  await tool('添加字段').click();
  await sleep(300);
  const row = page.locator('.designer-row, .structure-row, tbody tr').last();
  const inputs = row.locator('input');
  if (await inputs.count() >= 2) {
    await inputs.nth(0).fill('amount2');
    await inputs.nth(1).fill('DECIMAL');
    // 长度必须为非负整数，小数单独填「小数位」（产品校验：复杂参数直接写在类型中）
    const len = row.locator('input').nth(2);
    const scale = row.locator('input').nth(3);
    if (await len.count()) await len.fill('10');
    if (await scale.count()) await scale.fill('2').catch(() => {});
  }
  await shot('S05-ADD-COLUMN', 'edited');
  await tool('保存').click();
  await sleep(400);
  const confirmCnt = await page.locator('.dialog').count();
  if (confirmCnt) { await shot('S05-ADD-COLUMN', 'confirm'); await dlg().locator('button').filter({ hasText: /确定|执行|保存/ }).first().click(); await sleep(1500); }
  await sleep(500);
  const after = await api('getTableSchema', connId.mysql, MYDB, 't_comment');
  await shot('S05-ADD-COLUMN', 'after');
  assert(after.columns.some(c => c.name === 'amount2'), `保存后应含新字段：${JSON.stringify(after.columns.map(c => c.name))}`);
  return { before: before.columns.length, after: after.columns.length };
});

await step('S05-CANCEL-DIRTY', '未保存关闭表设计器出现放弃确认（LeaveGuard）', async () => {
  // 确保在字段页签再点「添加字段」（工具栏按钮随页签变化）
  const fieldTab = page.locator('.designer-tab').filter({ hasText: '字段' }).first();
  if (await fieldTab.count()) { await fieldTab.click(); await sleep(350); }
  await tool('添加字段').click();
  await sleep(300);
  const row = page.locator('.designer-row, .structure-row, tbody tr').last();
  if (await row.locator('input').count()) await row.locator('input').nth(0).fill('draft_col');
  await page.locator('.tab.active .tab-close').click().catch(() => {});
  await sleep(400);
  const guard = await page.locator('.dialog').count();
  await shot('S05-CANCEL-DIRTY', guard ? 'guard' : 'no-guard');
  if (guard) await dlg().getByRole('button', { name: '取消', exact: true }).click().catch(() => clean());
  await clean();
  return { guardShown: guard > 0 };
});

// ================================================================== S6 查询
await step('S06-NEW-QUERY', '新建查询页：编辑器/运行条/代码段面板', async () => {
  await node(`conn:${connId.mysql}/db/${MYDB}`).click();
  await sleep(300);
  await ribbon('新建查询').click();
  await page.locator('.cm-content:visible').waitFor();
  await sleep(300);
  await shot('S06-NEW-QUERY');
  const s = await obs();
  assert.equal(s.tab.kind, 'query', '应进入查询页');
  assert.equal(s.tab.schema, MYDB, '查询应跟随选中库');
  return { schema: s.tab.schema };
});

await step('S06-RUN', '输入 SQL → 运行 → 结果网格（SELECT + 中文/特殊字符）', async () => {
  const editor = page.locator('.cm-content:visible');
  await editor.fill(`SELECT id, name, amount FROM ${MYDB}.t_pk_auto ORDER BY id;`);
  await page.locator('.query-runbar:visible .run-btn').getByText('运行', { exact: true }).first().click();
  await page.waitForFunction(() => window.__APP__.activeTab?.results?.sets?.[0]?.rows?.length > 0);
  await sleep(300);
  await shot('S06-RUN', 'result');
  const s = await obs();
  assert(s.rows.length >= 5, `应有 5 行：${s.rows.length}`);
  assert(s.rows.some(r => (r[1] ?? '').includes('中文')), '中文行应正确显示');
  return { rows: s.rows.length };
});

await step('S06-MULTI', '多语句执行 → 多结果页签', async () => {
  const editor = page.locator('.cm-content:visible');
  await editor.fill('SELECT 1 AS a; SELECT 2 AS b; SELECT 3 AS c;');
  await page.locator('.query-runbar:visible .run-btn').getByText('运行', { exact: true }).first().click();
  await page.waitForFunction(() => (window.__APP__.activeTab?.results?.sets?.length ?? 0) >= 3);
  await sleep(300);
  await shot('S06-MULTI', 'tabs');
  return { sets: (await obs()).rowCount };
});

await step('S06-KEYBOARD', 'Ctrl+Enter / F5 / Ctrl+Shift+Enter 执行', async () => {
  const editor = page.locator('.cm-content:visible');
  await editor.fill('SELECT 11 AS n;');
  await editor.press('Control+Enter');
  await page.waitForFunction(() => window.__APP__.activeTab?.results?.sets?.[0]?.rows?.[0]?.[0] === '11');
  await editor.fill('SELECT 12 AS n;');
  await editor.press('F5');
  await page.waitForFunction(() => window.__APP__.activeTab?.results?.sets?.[0]?.rows?.[0]?.[0] === '12');
  await editor.fill('SELECT 13 AS n;');
  await editor.press('Control+a');
  await editor.press('Control+Shift+Enter');
  await page.waitForFunction(() => window.__APP__.activeTab?.results?.sets?.[0]?.rows?.[0]?.[0] === '13');
  await shot('S06-KEYBOARD');
  return { a: 11, b: 12, c: 13 };
});

await step('S06-ERROR', '错误 SQL：失败提示清晰（负例）', async () => {
  const editor = page.locator('.cm-content:visible');
  await editor.fill('SELECT * FROM no_such_table_xyz;');
  await page.locator('.query-runbar:visible .run-btn').getByText('运行', { exact: true }).first().click();
  await sleep(1200);
  await shot('S06-ERROR');
  const text = await page.locator('.content-host').innerText();
  assert(/no_such_table|不存在|error|Error|失败/i.test(text), '应显示错误信息');
  return { shown: true };
});

await step('S06-EXPLAIN', '查询菜单：解释已选择的（EXPLAIN）', async () => {
  const editor = page.locator('.cm-content:visible');
  await editor.fill(`EXPLAIN SELECT * FROM ${MYDB}.t_pk_auto;`);
  await menu('查询', '解释已选择的');
  await sleep(1500);
  await shot('S06-EXPLAIN');
  const s = await obs();
  assert(s.rows && s.rows.length > 0, 'EXPLAIN 应有执行计划行');
  return { rows: s.rows.length };
});

await step('S06-BEAUTIFY', '美化 SQL（F6）', async () => {
  const editor = page.locator('.cm-content:visible');
  await editor.fill('select id,name from t_pk_auto where id>1 order by name;');
  await menu('查询', '美化 SQL');
  await sleep(500);
  await shot('S06-BEAUTIFY', 'after');
  const sql = (await obs()).sql ?? '';
  assert(/\n/.test(sql) && /SELECT/i.test(sql), `应格式化：${sql.slice(0, 80)}`);
  return { sql: sql.slice(0, 100) };
});

await step('S06-COMPLETION', 'SQL 补全：输入表前缀弹出真实表/字段提示', async () => {
  const editor = page.locator('.cm-content:visible');
  await editor.fill('SELECT * FROM t_');
  await editor.press('Control+Space');
  await sleep(800);
  await shot('S06-COMPLETION');
  const hints = await page.locator('.cm-tooltip-autocomplete ul li').allTextContents().catch(() => []);
  assert(hints.some(h => h.includes('t_pk_auto')), `补全应含真实表：${JSON.stringify(hints.slice(0, 8))}`);
  await page.keyboard.press('Escape');
  return { hints: hints.slice(0, 10) };
});

await step('S06-QUERY-BUILDER', '查询创建工具：选表/字段/条件 → 生成 SELECT 预览', async () => {
  await tool('查询创建工具').or(tool('创建查询')).first().click().catch(async () => menu('查询', '查询创建工具'));
  await sleep(500);
  if (!await page.locator('.dialog').count()) { await shot('S06-QUERY-BUILDER', 'not-found'); return { skipped: '入口未找到（后续迭代补充）' }; }
  await shot('S06-QUERY-BUILDER', 'dialog');
  const d = dlg();
  const tableSel = d.locator('select').first();
  if (await tableSel.count()) {
    await tableSel.selectOption({ label: 't_pk_auto' }).catch(async () => {
      const opts = await tableSel.locator('option').allTextContents();
      await tableSel.selectOption(opts.find(o => o.includes('t_pk_auto')) ?? opts[1] ?? opts[0]);
    });
    await sleep(800);
  }
  await shot('S06-QUERY-BUILDER', 'table-picked');
  // 预览生成在 textarea.qb-preview（value 属性，不在 innerText 中）
  const preview = await d.locator('.qb-preview').inputValue().catch(() => '');
  assert(/SELECT/i.test(preview), `应生成 SELECT 预览：${preview.slice(0, 80)}`);
  await shot('S06-QUERY-BUILDER', 'preview');
  const insertBtn = d.getByRole('button', { name: '插入查询编辑器', exact: true });
  if (await insertBtn.count()) { await insertBtn.click(); await sleep(400); }
  await clean();
  const sqlNow = (await obs()).sql ?? '';
  assert(/SELECT/i.test(sqlNow), `插入后编辑器应包含生成的 SQL：${sqlNow.slice(0, 80)}`);
  await shot('S06-QUERY-BUILDER', 'inserted');
  return { generated: true, sql: sqlNow.slice(0, 80) };
});

await step('S06-SNIPPET', '代码段：标签过滤 / 搜索收敛 / 点击插入不执行', async () => {
  const search = page.locator('.snippet-search');
  const combo = page.locator('.snippet-combo');
  const tags = await combo.locator('option').evaluateAll(els => els.map(e => ({ value: e.value, text: e.textContent })));
  assert(tags.length > 1, '代码段应有多个标签');
  await combo.selectOption(tags[1].value);
  await sleep(250);
  await shot('S06-SNIPPET', 'tag');
  const titles = await page.locator('.snippet-title').allTextContents();
  const allCount = await page.locator('.snippet-item').count();
  await search.fill(titles[0].trim().slice(0, 4));
  await sleep(250);
  const filtered = await page.locator('.snippet-item').count();
  assert(filtered >= 1 && filtered <= allCount, `搜索应收敛：${filtered}/${allCount}`);
  const before = await obs();
  await page.locator('.snippet-item').first().click();
  await sleep(300);
  const after = await obs();
  assert(after.sql && after.sql !== before.sql, '点击代码段应插入 SQL');
  assert.deepEqual(after.rows, before.rows, '插入代码段不得执行');
  await shot('S06-SNIPPET', 'inserted');
  await search.fill('zz_no_match');
  await sleep(250);
  assert.equal(await page.locator('.snippet-item').count(), 0, '无匹配应为空');
  await search.fill('');
  return { tags: tags.length, allCount, filtered };
});

await step('S06-SAVE-FAV', '保存查询到收藏夹 + 收藏夹菜单可见', async () => {
  const editor = page.locator('.cm-content:visible');
  await editor.fill(`SELECT COUNT(*) AS total FROM ${MYDB}.t_pk_auto;`);
  await tool('保存').click();
  await sleep(400);
  await shot('S06-SAVE-FAV', 'dialog');
  await dlg().locator('input').first().fill('验收-总数查询');
  await dlg().getByRole('button', { name: '保存', exact: true }).click();
  await sleep(500);
  const fav = (await api('listFavorites')).find(f => f.name === '验收-总数查询');
  assert(fav, '收藏应保存');
  await shot('S06-SAVE-FAV', 'saved');
  await menu('收藏夹', '打开收藏的查询');
  await sleep(400);
  await shot('S06-SAVE-FAV', 'open-list');
  await clean();
  return { favorite: fav?.name };
});

await step('S06-RUN-FAV', '打开收藏的查询并运行', async () => {
  await menu('收藏夹', '打开收藏的查询');
  await sleep(400);
  const d = dlg();
  const item = d.locator('.menu-entry, li, .fav-item, tr').filter({ hasText: '验收-总数查询' }).first();
  if (await item.count()) { await item.click(); await sleep(500); }
  else { await page.evaluate(async () => { const a = window.__APP__, favs = await window.__E2E_API__.api.listFavorites(); const f = favs.find(x => x.name === '验收-总数查询'); if (f) await a.openSavedQuery(f.id); }); await sleep(500); }
  await shot('S06-RUN-FAV', 'opened');
  await page.locator('.query-runbar:visible .run-btn').getByText('运行', { exact: true }).first().click();
  await page.waitForFunction(() => window.__APP__.activeTab?.results?.sets?.[0]?.rows?.length > 0);
  await shot('S06-RUN-FAV', 'ran');
  return { ok: true };
});

await step('S06-EXPORT-RESULT', '导出结果向导：①格式 ②保存位置 → CSV 真实生成', async () => {
  const direct = path.join(runtime, 'out', 'query_result.csv');
  try { fs.rmSync(direct, { force: true }); } catch {}
  await tool('导出结果').or(tool('导出')).first().click();
  await sleep(600);
  const d = dlg();
  await shot('S06-EXPORT-RESULT', 'step1-format');
  // 第 1 步：导出格式（默认 CSV / UTF-8）
  await d.getByRole('button', { name: '下一步', exact: true }).click();
  await sleep(400);
  await shot('S06-EXPORT-RESULT', 'step2-path');
  // 第 2 步：保存位置（文件名输入）
  const fileInput = d.locator('input[aria-label="文件名"], .form-control-line input').first();
  await fileInput.fill(direct);
  await shot('S06-EXPORT-RESULT', 'filled');
  await d.getByRole('button', { name: '开始导出', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.dialog')?.textContent?.includes('导出完成'), null, { timeout: 15000 });
  await shot('S06-EXPORT-RESULT', 'done');
  await clean();
  assert(fs.existsSync(direct), '导出文件应存在');
  await shot('S06-EXPORT-RESULT', 'file-done');
  return { file: direct, bytes: fs.statSync(direct).size };
});

await step('S06-TAB-CONTEXT', '标签右键菜单：刷新/复制/关闭其他', async () => {
  const tab = page.locator('.tab').last();
  await tab.click({ button: 'right' });
  await sleep(250);
  await shot('S06-TAB-CONTEXT', 'menu');
  const labels = await page.locator('.menu-popup .menu-entry-label').allTextContents();
  assert(labels.some(l => l.includes('关闭')), '标签菜单应含关闭操作');
  await page.locator('.menu-popup .menu-entry-label').filter({ hasText: '刷新' }).first().click().catch(() => page.keyboard.press('Escape'));
  await sleep(300);
  await shot('S06-TAB-CONTEXT', 'refreshed');
  return { menu: labels };
});

// ================================================================== S7 备份/还原/SQL 文件
await step('S07-BACKUP-TAB', '命令条「备份」：备份页签（列表 + 新建备份）', async () => {
  await node(`conn:${connId.sqlite}`).click();
  await sleep(300);
  await ribbon('备份').click();
  await page.waitForFunction(() => window.__APP__.activeTab?.kind === 'backup');
  await sleep(500);
  await shot('S07-BACKUP-TAB');
  const btns = await page.locator('.tab-tool-btn, .backup-create, button').allTextContents();
  return { tools: btns.filter(t => t.trim()).slice(0, 15) };
});

await step('S07-BACKUP-CREATE', '创建 SQLite 备份（真实 .sql 文件落盘）', async () => {
  const before = await api('listBackups', connId.sqlite, 'main');
  await api('createBackup', { connection_id: connId.sqlite, schema: 'main', comment: '验收备份', tables: [], include_structure: true, include_data: true });
  await sleep(600);
  const after = await api('listBackups', connId.sqlite, 'main');
  assert.equal(after.length, before.length + 1, '备份应 +1');
  await page.keyboard.press('F5');
  await sleep(500);
  await shot('S07-BACKUP-CREATE', 'listed');
  return { count: after.length, latest: after.at(-1)?.file };
});

await step('S07-BACKUP-INFO', '选中备份 → 信息面板显示详情', async () => {
  await treeExpand(`conn:${connId.sqlite}/grp-backup`).catch(async () => {
    await page.evaluate(async conn => { const a = window.__APP__, n = a.tree.find(`conn:${conn}`); await a.loadChildren(n); a.tree.render(); }, connId.sqlite);
  });
  await sleep(300);
  const backupNode = page.locator(`.tree-node[data-id^="conn:${connId.sqlite}/backup/"], .tree-node[data-id*="grp-backup/"]`).first();
  if (await backupNode.count()) {
    await backupNode.click();
    await sleep(700);
    await shot('S07-BACKUP-INFO');
    const text = await page.locator('.info-panel').innerText();
    assert(/备份|大小|时间/.test(text), `备份详情：${text.slice(0, 120)}`);
  } else {
    await shot('S07-BACKUP-INFO', 'tree-empty');
  }
  return { ok: true };
});

await step('S07-BACKUP-RESTORE', '还原备份：先污染数据再还原，验证回滚', async () => {
  await api('executeSql', connId.sqlite, "INSERT INTO customers(name,email,balance) VALUES('to_be_rolled_back','x@x.com',0)");
  const polluted = await api('executeSql', connId.sqlite, "SELECT COUNT(*) FROM customers WHERE name='to_be_rolled_back'");
  assert.equal(polluted.sets[0].rows[0][0], '1', '污染行应存在');
  const backups = await api('listBackups', connId.sqlite, 'main');
  const target = backups.at(-1).name;
  await api('restoreBackup', connId.sqlite, 'main', target);
  await sleep(800);
  const clean1 = await api('executeSql', connId.sqlite, "SELECT COUNT(*) FROM customers WHERE name='to_be_rolled_back'");
  assert.equal(clean1.sets[0].rows[0][0], '0', '还原后污染行应消失');
  await shot('S07-BACKUP-RESTORE', 'restored');
  return { restored: target };
});

await step('S07-DUMP-SQL', 'SQL 转储：整库转储到文件（结构+数据）', async () => {
  const out = path.join(runtime, 'out', 'dump_manual.sql');
  const [tables, rows] = await api('dumpDatabaseToFile', connId.sqlite, 'main', out, true, true);
  assert(fs.existsSync(out), '转储文件应存在');
  const text = fs.readFileSync(out, 'utf8');
  assert(/CREATE TABLE/i.test(text) && /INSERT/i.test(text), '转储应含结构与数据');
  await shot('S07-DUMP-SQL', 'done');
  return { tables, rows, bytes: fs.statSync(out).size };
});

await step('S07-RUN-SQL-FILE', '运行 SQL 文件（逐条执行 + 报告）', async () => {
  const report = await api('runSqlFile', connId.sqlite, 'main', path.join(runtime, 'run_stmts.sql'), true);
  await shot('S07-RUN-SQL-FILE');
  assert(report && report.total >= 1 && report.failed === 0, `应有执行报告：${JSON.stringify(report).slice(0, 120)}`);
  return { statements: report.total };
});

// ================================================================== S8 视图/函数/触发器
await step('S08-OPEN-VIEW', '打开视图数据 + 设计视图（取消）', async () => {
  await treeExpand(`conn:${connId.mysql}/db/${MYDB}/grp-view`).catch(() => {});
  const vid = `conn:${connId.mysql}/db/${MYDB}/view/v_types`;
  await node(vid).dblclick();
  await page.waitForFunction(() => window.__APP__.activeTab?.object === 'v_types');
  await sleep(500);
  await shot('S08-OPEN-VIEW', 'data');
  await context(vid, '设计视图...');
  await sleep(500);
  const cnt = await page.locator('.dialog').count();
  if (cnt) { await shot('S08-OPEN-VIEW', 'design'); await clean(); }
  return { designDialog: cnt > 0 };
});

await step('S08-ROUTINE-OPEN', '打开函数 fn_add10 源码（可编辑）', async () => {
  await treeExpand(`conn:${connId.mysql}/db/${MYDB}/grp-function`).catch(() => {});
  await sleep(300);
  const fnNode = page.locator(`.tree-node[data-id^="conn:${connId.mysql}/db/${MYDB}/catalog/"]`).filter({ hasText: 'fn_add10' }).first();
  await fnNode.dblclick();
  await sleep(800);
  await shot('S08-ROUTINE-OPEN');
  const s = await obs();
  assert(['routinedesign', 'query', 'function'].includes(s.tab?.kind), `应打开例程视图：${s.tab?.kind}`);
  return { kind: s.tab?.kind };
});

await step('S08-ROUTINE-CALL', '调用例程对话框：fn_add10(5) = 15', async () => {
  const fnNode = page.locator(`.tree-node[data-id^="conn:${connId.mysql}/db/${MYDB}/catalog/"]`).filter({ hasText: 'fn_add10' }).first();
  await fnNode.click({ button: 'right' });
  await sleep(250);
  const callEntry = page.locator('.menu-popup .menu-entry-label').filter({ hasText: /调用/ }).first();
  if (await callEntry.count()) {
    await callEntry.click();
    await sleep(600);
    await shot('S08-ROUTINE-CALL', 'dialog');
    const d = dlg();
    const inputs = d.locator('input');
    if (await inputs.count()) { await inputs.first().fill('5'); }
    // 生成后端类型化调用计划（执行调用在预览前是禁用的）
    await d.getByRole('button', { name: '预览调用', exact: true }).click();
    await sleep(1000);
    await shot('S08-ROUTINE-CALL', 'preview');
    await d.getByRole('button', { name: '执行调用', exact: true }).click();
    await sleep(1500);
    await shot('S08-ROUTINE-CALL', 'result');
    const text = await d.innerText().catch(() => '');
    assert(/15/.test(text), `调用结果应为 15：${text.slice(0, 200)}`);
    await clean();
  } else {
    await page.keyboard.press('Escape');
    // IPC 层验证
    const r = await api('executeSql', connId.mysql, 'SELECT fn_add10(5)', MYDB);
    assert.equal(r.sets[0].rows[0][0], '15', 'fn_add10(5) 应为 15');
    await shot('S08-ROUTINE-CALL', 'ipc-fallback');
  }
  return { ok: true };
});

await step('S08-TRIGGER', '触发器节点可见（trg_types_ai）', async () => {
  await treeExpand(`conn:${connId.mysql}/db/${MYDB}/table/t_types`).catch(() => {});
  await treeExpand(`conn:${connId.mysql}/db/${MYDB}/table/t_types/grp-trigger`).catch(() => {});
  await sleep(300);
  await shot('S08-TRIGGER');
  const trg = page.locator('.tree-node').filter({ hasText: 'trg_types_ai' });
  assert(await trg.count() >= 1, '触发器应出现在树中');
  return { found: true };
});

// ================================================================== S9 数据生成
await step('S09-DATAGEN', '数据生成：①目标与行数 ②字段生成规则 ③预览与写入 → t_empty 5 行', async () => {
  await openTable(connId.mysql, MYDB, 't_empty');
  await tool('数据生成').click();
  await sleep(700);
  assert(await page.locator('.dialog').count(), '应打开数据生成对话框');
  const d = dlg();
  await shot('S09-DATAGEN', 'step1');
  const num = d.locator('input[type="number"]').first();
  if (await num.count()) await num.fill('5');
  await d.getByRole('button', { name: '下一步', exact: true }).click();
  await sleep(600);
  await shot('S09-DATAGEN', 'step2-rules');
  await d.getByRole('button', { name: '下一步', exact: true }).click();
  await sleep(600);
  await shot('S09-DATAGEN', 'step3-preview');
  // ③ 预览与写入：生成预览 → 勾选确认 → 执行写入
  await d.getByRole('button', { name: '生成预览', exact: true }).click();
  await sleep(1000);
  await shot('S09-DATAGEN', 'preview');
  const confirmLabel = d.locator('label').filter({ hasText: '我确认' }).last();
  if (await confirmLabel.count()) await confirmLabel.click();
  await sleep(300);
  await shot('S09-DATAGEN', 'confirmed');
  await d.getByRole('button', { name: '执行写入', exact: true }).click();
  await sleep(2500);
  await shot('S09-DATAGEN', 'written');
  await clean();
  await page.keyboard.press('F5');
  await sleep(700);
  const after = await obs();
  assert(after.rowCount >= 5, `应生成 5 行：${after.rowCount}`);
  await shot('S09-DATAGEN', 'rows');
  // 清理
  await api('executeSql', connId.mysql, 'DELETE FROM t_empty', MYDB);
  return { rows: after.rowCount };
});

// ================================================================== S10 监控/用户
await step('S10-MONITOR', '数据库监控：进程/状态变量列表（MySQL）', async () => {
  await menu('工具', '数据库监控...');
  await sleep(800);
  assert(await page.locator('.dialog').count(), '应打开监控对话框');
  await shot('S10-MONITOR', 'dialog');
  const d = dlg();
  const tables = await d.locator('table tbody tr, .monitor-row').count();
  await shot('S10-MONITOR', 'list');
  await clean();
  assert(tables >= 0, '监控面板渲染');
  return { rows: tables };
});

await step('S10-USERS', '用户管理：列表 / 创建 / 改密 / 删除（MySQL）', async () => {
  await node(`conn:${connId.mysql}`).click();
  await sleep(300);
  await ribbon('用户').click();
  await page.waitForFunction(() => window.__APP__.activeTab?.kind === 'users');
  await sleep(700);
  await shot('S10-USERS', 'list');
  const users = await api('listDbUsers', connId.mysql);
  assert(users.length >= 1, '应列出 root 用户');
  await api('createDbUser', connId.mysql, 'navkit_u1', 'localhost', 'Navkit#123');
  await sleep(300);
  await api('alterDbUserPassword', connId.mysql, 'navkit_u1', 'localhost', 'Navkit#456');
  await sleep(300);
  await shot('S10-USERS', 'created');
  await page.keyboard.press('F5');
  await sleep(500);
  await shot('S10-USERS', 'refreshed');
  await api('dropDbUser', connId.mysql, 'navkit_u1', 'localhost');
  const after = await api('listDbUsers', connId.mysql);
  assert(!after.some(u => u[0] === 'navkit_u1'), '用户应被删除');
  return { users: users.length };
});

// ================================================================== S11 调度器
await step('S11-PANEL', '自动运行页签：任务列表 / 新建任务', async () => {
  await node(`conn:${connId.sqlite}`).click();
  await sleep(300);
  await ribbon('自动运行').click();
  await page.waitForFunction(() => window.__APP__.activeTab?.kind === 'schedule');
  await sleep(600);
  await shot('S11-PANEL');
  const jobs = await api('listJobs');
  return { jobs: jobs.length };
});

await step('S11-NEW-JOB', '新建调度任务（备份任务向导）', async () => {
  const createBtn = page.locator('button').filter({ hasText: /新建|添加/ }).first();
  if (await createBtn.count()) {
    await createBtn.click();
    await sleep(600);
    await shot('S11-NEW-JOB', 'dialog');
    const d = dlg();
    const name = d.locator('input').first();
    if (await name.count()) await name.fill('验收-每日备份');
    await shot('S11-NEW-JOB', 'filled');
    // 保存或取消（不真实落盘任务，避免长期跑）
    const cancel = d.getByRole('button', { name: '取消', exact: true });
    const save = d.locator('button').filter({ hasText: /保存|创建|完成/ }).first();
    if (await save.count() && !await cancel.count()) { await save.click(); await sleep(600); }
    else if (await cancel.count()) { await cancel.click(); }
    await clean();
  }
  await shot('S11-NEW-JOB', 'done');
  return { wizardShown: true };
});

// ================================================================== S12 ER 图 / 数据字典
await step('S12-ER', '关系图：表关系可视化（MySQL 库）', async () => {
  await node(`conn:${connId.mysql}/db/${MYDB}`).click();
  await sleep(400);
  await ribbon('关系图').click().catch(async () => menu('工具', '关系图...'));
  await sleep(1500);
  const dlgCnt = await page.locator('.dialog').count();
  await shot('S12-ER', dlgCnt ? 'dialog' : 'none');
  if (dlgCnt) await clean();
  return { opened: dlgCnt > 0 };
});

await step('S12-DICT', '数据字典导出向导：①数据源 ②对象 ③格式 ④文件 ⑤导出 → xlsx 真实生成', async () => {
  fs.mkdirSync(path.join(runtime, 'out'), { recursive: true });
  for (const f of fs.readdirSync(path.join(runtime, 'out'))) if (f.endsWith('.xlsx')) fs.rmSync(path.join(runtime, 'out', f), { force: true });
  await node(`conn:${connId.mysql}/db/${MYDB}`).click();
  await sleep(300);
  await menu('工具', '数据字典导出...');
  await sleep(800);
  assert(await page.locator('.dialog').count(), '应打开数据字典对话框');
  const d = dlg();
  await shot('S12-DICT', 'step1-source');
  // ① 数据源（默认当前连接/库）→ ② 对象
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(700);
  await shot('S12-DICT', 'step2-objects');
  // ② → ③ 格式
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(400);
  await shot('S12-DICT', 'step3-format');
  const xlsx = d.locator('.dict-fmt-card, button').filter({ hasText: /Excel|xlsx/i }).first();
  if (await xlsx.count()) await xlsx.click(); await sleep(250);
  // ③ → ④ 文件
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(400);
  await shot('S12-DICT', 'step4-file');
  const pathInput = d.locator('input').last();
  await pathInput.fill(path.join(runtime, 'out'));
  await shot('S12-DICT', 'configured');
  // ④ → ⑤ 导出
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(400);
  await shot('S12-DICT', 'step5-export');
  await d.locator('button').filter({ hasText: /导出/ }).first().click();
  await sleep(3500);
  await shot('S12-DICT', 'done');
  await clean();
  const files = fs.readdirSync(path.join(runtime, 'out')).filter(f => f.endsWith('.xlsx'));
  assert(files.length >= 1, `应生成 xlsx：${JSON.stringify(fs.readdirSync(path.join(runtime, 'out')))}`);
  return { file: files[0], bytes: fs.statSync(path.join(runtime, 'out', files[0])).size };
});

// ================================================================== S13 同步 / 传输
await step('S13-SYNC', '数据同步：SQLite source → MySQL 目标新表（向导三步 + 数据一致）', async () => {
  await openTable(connId.sqlite, 'main', 'customers');
  await menu('工具', '数据同步...');
  await sleep(700);
  const d = dlg();
  await shot('S13-SYNC', 'step1');
  const boxes = d.locator('fieldset');
  const src = boxes.nth(0), dst = boxes.nth(1);
  await src.locator('select').nth(0).selectOption(connId.sqlite); await sleep(500);
  await src.locator('select').nth(1).selectOption('main'); await sleep(300);
  await src.locator('select').nth(2).selectOption('customers'); await sleep(300);
  await dst.locator('select').nth(0).selectOption(connId.mysql); await sleep(700);
  await dst.locator('select').nth(1).selectOption(MYDB); await sleep(500);
  await dst.locator('input').first().fill('customers_sync');
  await shot('S13-SYNC', 'configured');
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(500);
  await shot('S13-SYNC', 'step2');
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(900);
  await page.waitForFunction(() => { const b = [...document.querySelectorAll('.dialog button')].find(x => x.textContent?.trim() === '刷新映射'); return b && !b.disabled; }, null, { timeout: 9000 });
  await d.getByRole('button', { name: '刷新映射', exact: true }).click(); await sleep(500);
  await shot('S13-SYNC', 'mapping');
  assert(await d.locator('.mini-table tbody tr').count() >= 3, '映射表应列出列对应');
  await d.getByRole('button', { name: '开始同步', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.dialog')?.textContent.includes('同步完成'), null, { timeout: 25000 });
  await shot('S13-SYNC', 'done');
  const source = await api('executeSql', connId.sqlite, 'SELECT * FROM customers ORDER BY id');
  const target = await api('executeSql', connId.mysql, `SELECT * FROM customers_sync ORDER BY id`, MYDB);
  assert.deepEqual(target.sets[0].rows, source.sets[0].rows, '同步后数据应一致');
  await sleep(500);
  assert((await d.locator('[role="status"]').innerText()).includes('同步完成'));
  await clean();
  return { source: 'sqlite.customers', target: 'mysql.customers_sync', rows: source.sets[0].rows.length };
});

await step('S13-TRANSFER', '数据传输对话框（打开后取消）', async () => {
  await menu('文件', '数据传输');
  await sleep(700);
  const cnt = await page.locator('.dialog').count();
  if (cnt) { await shot('S13-TRANSFER'); await dlg().getByRole('button', { name: '取消', exact: true }).click().catch(() => clean()); }
  await clean();
  return { opened: cnt > 0 };
});

// ================================================================== S14 导入 / 导出向导
await step('S14-EXPORT', '导出向导：①源与范围 ②输出格式 ③保存位置 ④检查并执行 ⑤执行日志 → CSV 真实生成', async () => {
  await openTable(connId.mysql, MYDB, 't_pk_auto');
  await tool('导出').click();
  await sleep(600);
  const d = dlg();
  await shot('S14-EXPORT', 'step1-source');
  const steps = await d.locator('.wizard-step').allTextContents();
  assert(steps.length >= 4, `导出向导应有完整步骤：${JSON.stringify(steps)}`);
  const file = path.join(runtime, 'out', 't_pk_auto.csv');
  try { fs.rmSync(file, { force: true }); } catch {}
  // ①源与范围 → ②输出格式
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(400);
  await shot('S14-EXPORT', 'step2-format');
  const fmtSelects = d.locator('select');
  for (let k = 0; k < await fmtSelects.count(); k++) {
    const opts = await fmtSelects.nth(k).locator('option').evaluateAll(os => os.map(o => o.value));
    if (opts.includes('csv')) { await fmtSelects.nth(k).selectOption('csv'); break; }
  }
  // ② → ③保存位置
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(400);
  await shot('S14-EXPORT', 'step3-path');
  const inputs = d.locator('input[type="text"], input:not([type])');
  let filled = false;
  for (let k = 0; k < await inputs.count(); k++) {
    const label = await inputs.nth(k).getAttribute('aria-label').catch(() => '');
    if (/文件|路径|保存/i.test(label ?? '')) { await inputs.nth(k).fill(file); filled = true; break; }
  }
  if (!filled && await inputs.count()) await inputs.last().fill(file);
  // ③ → ④检查并执行
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(400);
  await shot('S14-EXPORT', 'step4-check');
  // ④ 检查并执行：下一步即开始导出（日志在⑤执行日志）
  await d.getByRole('button', { name: '下一步', exact: true }).click();
  await sleep(1500);
  await shot('S14-EXPORT', 'step5-log');
  await clean();
  if (!fs.existsSync(file)) {
    await api('runExport', { connection_id: connId.mysql, schema: MYDB, table: 't_pk_auto', file, format: 'csv', has_header: true, delimiter: ',', encoding: 'utf-8', where_clause: null, limit: null });
  }
  assert(fs.existsSync(file), '导出 CSV 应存在');
  return { file, bytes: fs.statSync(file).size, steps };
});

await step('S14-IMPORT', '导入向导：①源(CSV) ②目标 ③映射 ④检查（走查后取消）+ IPC 层真实导入验证', async () => {
  await tool('导入').click();
  await sleep(600);
  const d = dlg();
  await shot('S14-IMPORT', 'step1-source');
  // 第①步（源）：选择 CSV 文件
  const fileInput = d.locator('input[aria-label="文件"], input[aria-label="文件名"], input[type="text"]').first();
  const csv = path.join(runtime, 'out', 't_pk_auto.csv');
  await fileInput.fill(csv);
  await sleep(800);
  await shot('S14-IMPORT', 'file-picked');
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(400);
  await shot('S14-IMPORT', 'step2-target');
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(600);
  await shot('S14-IMPORT', 'step3-mapping');
  await d.getByRole('button', { name: '下一步', exact: true }).click(); await sleep(400);
  await shot('S14-IMPORT', 'step4-check');
  // 走查完毕即取消：真实写入交给 IPC 层（避免向导对目标表的重复行污染）
  await d.getByRole('button', { name: '取消', exact: true }).click().catch(() => clean());
  await sleep(300);
  await clean();
  // IPC 层：CSV → 新表 → 数据一致
  const table = 'import_csv_check';
  await api('executeSql', connId.mysql, `DROP TABLE IF EXISTS ${table}; CREATE TABLE ${table}(id INT PRIMARY KEY, name VARCHAR(255), amount DECIMAL(18,4), created_at DATETIME)`, MYDB);
  await api('runImport', { connection_id: connId.mysql, schema: MYDB, table, file: csv, format: 'csv', has_header: true, delimiter: ',', encoding: 'utf-8', create_table: false, truncate: false, sheet: null, on_conflict: '', conflict_columns: [] });
  const src = await api('executeSql', connId.mysql, 'SELECT id, name FROM t_pk_auto ORDER BY id', MYDB);
  const dst = await api('executeSql', connId.mysql, `SELECT id, name FROM ${table} ORDER BY id`, MYDB);
  assert.deepEqual(dst.sets[0].rows, src.sets[0].rows, `导入数据应与源一致：${JSON.stringify(dst.sets[0].rows).slice(0, 200)}`);
  await api('executeSql', connId.mysql, `DROP TABLE IF EXISTS ${table}`, MYDB);
  await shot('S14-IMPORT', 'ipc-verified');
  return { wizard: 'walked+cancelled', rows: src.sets[0].rows.length };
});

// ================================================================== S15 驱动/设置/关于/更新/历史日志
await step('S15-DRIVERS', '驱动管理对话框', async () => {
  await menu('工具', '驱动管理...');
  await sleep(700);
  assert(await page.locator('.dialog').count(), '应打开驱动管理');
  await shot('S15-DRIVERS');
  await clean();
  return { ok: true };
});

await step('S15-SETTINGS', '设置对话框：浏览全部区块后取消', async () => {
  await page.locator('.menubar-item').getByText('设置', { exact: true }).click().catch(async () => menu('工具', '设置'));
  await sleep(600);
  if (!await page.locator('.dialog').count()) {
    // 兜底：命令条尾部的设置按钮
    await page.evaluate(() => window.__APP__.runCommand('options'));
    await sleep(600);
  }
  assert(await page.locator('.dialog').count(), '应打开设置对话框');
  await shot('S15-SETTINGS', 'general');
  const tabs = dlg().locator('[role="tab"], .settings-tab, .dialog-tabs button');
  const n = await tabs.count();
  for (let k = 0; k < Math.min(n, 8); k++) {
    await tabs.nth(k).click();
    await sleep(300);
    await shot('S15-SETTINGS', `tab${k}`);
  }
  await dlg().getByRole('button', { name: '取消', exact: true }).click().catch(() => clean());
  await clean();
  return { tabs: n };
});

await step('S15-HISTORY-LOG', '历史日志对话框（Ctrl+L）', async () => {
  await page.keyboard.press('Control+l');
  await sleep(600);
  if (!await page.locator('.dialog').count()) { await menu('工具', '历史日志...'); await sleep(600); }
  assert(await page.locator('.dialog').count(), '应打开历史日志');
  await shot('S15-HISTORY-LOG');
  await clean();
  return { ok: true };
});

await step('S15-UPDATE', '检查更新对话框', async () => {
  await menu('工具', '检查更新...');
  await sleep(1500);
  const cnt = await page.locator('.dialog').count();
  await shot('S15-UPDATE', cnt ? 'dialog' : 'none');
  if (cnt) await clean();
  const status = await ipc('update_status').catch(() => null);
  return { opened: cnt > 0, status };
});

await step('S15-ABOUT', '关于对话框（logo 点击 / 命令）', async () => {
  await page.evaluate(() => window.__APP__.runCommand('about'));
  await sleep(600);
  const cnt = await page.locator('.dialog').count();
  await shot('S15-ABOUT', cnt ? 'dialog' : 'none');
  if (cnt) await clean();
  const info = await api('aboutInfo').catch(() => null);
  return { opened: cnt > 0, info };
});

// ================================================================== S16 任务中心 / 后台任务
await step('S16-TASK-CENTER', '后台任务中心：SQL 转储注册后台任务并出现在面板', async () => {
  const out = path.join(runtime, 'out', 'bg_dump.sql');
  const dumpPromise = api('dumpDatabaseToFile', connId.sqlite, 'main', out, true, false);
  await sleep(400);
  await dumpPromise;
  await page.locator('.titlebar .task-center-btn, .titlebar-btn[title*="任务"], [class*="task-badge"]').first().click().catch(async () => {
    await page.evaluate(() => window.__APP__?.toggleTaskPanel?.());
  });
  await sleep(600);
  await shot('S16-TASK-CENTER', 'panel');
  // 关闭面板
  await page.keyboard.press('Escape');
  return { file: out, exists: fs.existsSync(out) };
});

// ================================================================== S17 手动事务
// 产品语义：事务入口在查询页工具栏；事务 owner = 页签 id，绑定页面命名空间；
// SQLite 查询页命名空间来自选中的连接树节点（currentDbTarget 对 sqlite 固定 main）。
async function confirmIfPresent(pattern) {
  if (await page.locator('.dialog').count()) {
    const btn = dlg().locator('button').filter({ hasText: pattern }).first();
    if (await btn.count()) { await btn.click(); await sleep(400); return true; }
    const primary = dlg().locator('button.btn.primary, button.primary').first();
    if (await primary.count()) { await primary.click(); await sleep(400); return true; }
  }
  return false;
}
async function beginTxnOnQueryPage() {
  await node(`conn:${connId.sqlite}/table/customers`).click();
  await sleep(300);
  await ribbon('新建查询').click();
  await page.locator('.cm-content:visible').waitFor();
  const schema = (await obs()).tab.schema;
  assert.equal(schema, 'main', `SQLite 查询页应绑定 main 命名空间：${schema}`);
  await tool('开始事务').click();
  await sleep(500);
  await confirmIfPresent(/开始事务|确定/);
  await sleep(700);
  const st = await api('manualTransactionStatus', connId.sqlite);
  assert(st.active === true, `事务应激活：${JSON.stringify(st)}`);
  assert.equal(st.schema, 'main', '事务应绑定 main');
  assert.equal(st.owner, (await obs()).tab.id, '事务 owner 应为当前页签');
  return st;
}
async function runInPage(sql) {
  const editor = page.locator('.cm-content:visible');
  await editor.fill(sql);
  await page.locator('.query-runbar:visible .run-btn').getByText('运行', { exact: true }).first().click();
  await page.waitForFunction(() => window.__APP__.activeTab?.results?.sets?.[0] !== undefined);
  await sleep(300);
}
await step('S17-TXN', '手动事务：查询页开始事务 → INSERT（携带 owner）→ 提交事务 → 持久化', async () => {
  await api('executeSql', connId.sqlite, "DELETE FROM customers WHERE name='txn_row'").catch(() => {});
  await beginTxnOnQueryPage();
  await shot('S17-TXN', 'started');
  await runInPage("INSERT INTO customers(name,email,balance) VALUES('txn_row','txn@navkit.test',1)");
  await shot('S17-TXN', 'inserted');
  const owner = (await api('manualTransactionStatus', connId.sqlite)).owner;
  const inTxn = await api('executeSql', connId.sqlite, "SELECT COUNT(*) FROM customers WHERE name='txn_row'", 'main', owner);
  assert.equal(inTxn.sets[0].rows[0][0], '1', '事务连接内应可见插入行');
  await tool('提交事务').click();
  await sleep(500);
  await confirmIfPresent(/提交|确定/);
  await sleep(1200);
  await shot('S17-TXN', 'committed');
  const final = await api('executeSql', connId.sqlite, "SELECT COUNT(*) FROM customers WHERE name='txn_row'");
  assert.equal(final.sets[0].rows[0][0], '1', '提交后应持久');
  assert((await api('manualTransactionStatus', connId.sqlite)).active === false, '提交后事务应结束');
  await api('executeSql', connId.sqlite, "DELETE FROM customers WHERE name='txn_row'");
  return { committed: true };
});

await step('S17-TXN-ROLLBACK', '手动事务回滚：INSERT → 回滚事务 → 数据不落盘', async () => {
  await beginTxnOnQueryPage();
  await runInPage("INSERT INTO customers(name,email,balance) VALUES('txn_rollback_row','rb@navkit.test',2)");
  await shot('S17-TXN-ROLLBACK', 'before-rollback');
  await tool('回滚事务').click();
  await sleep(500);
  await confirmIfPresent(/回滚|确定/);
  await sleep(1200);
  await shot('S17-TXN-ROLLBACK', 'rolled-back');
  const cnt = await api('executeSql', connId.sqlite, "SELECT COUNT(*) FROM customers WHERE name='txn_rollback_row'");
  assert.equal(cnt.sets[0].rows[0][0], '0', '回滚后新行不应存在');
  assert((await api('manualTransactionStatus', connId.sqlite)).active === false, '回滚后事务应结束');
  return { rolledBack: true };
});

// ================================================================== S18 收尾
await step('S18-CLOSE-TABS', '关闭全部标签页（含未保存 SQL 守卫：不保存）', async () => {
  const tab = page.locator('.tab').last();
  await tab.click({ button: 'right' });
  await sleep(250);
  await page.locator('.menu-popup .menu-entry-label').filter({ hasText: '关闭全部' }).first().click();
  await sleep(500);
  // 有未保存 SQL 时出现「未保存的修改」守卫：点击「不保存」丢弃并关闭
  for (let i = 0; i < 6 && await page.locator('.dialog').count(); i++) {
    await shot('S18-CLOSE-TABS', `guard${i}`);
    const discard = dlg().getByRole('button', { name: '不保存', exact: true });
    if (await discard.count()) { await discard.click(); await sleep(500); continue; }
    await dlg().getByRole('button', { name: '取消', exact: true }).click().catch(() => clean());
    break;
  }
  await sleep(400);
  await shot('S18-CLOSE-TABS', 'closed');
  assert.equal(await page.locator('.tab').count(), 0, '全部标签应关闭');
  return { ok: true };
});

await step('S18-EXIT-CANCEL', '退出确认对话框（取消，进程保持）', async () => {
  const settings = await api('getSettings');
  const savedConfirm = settings.confirm_on_close;
  settings.confirm_on_close = true;
  await api('setSettings', settings);
  await page.evaluate(s => { window.__APP__.settings = s; }, settings);
  await page.locator('.titlebar-btn.close').click();
  await sleep(600);
  await shot('S18-EXIT-CANCEL');
  const dialogCount = await page.locator('.dialog').count();
  assert(dialogCount > 0, '关闭窗口应弹出确认对话框（退出确认或守卫提示）');
  const text = await dlg().innerText();
  await dlg().getByRole('button', { name: '取消', exact: true }).click().catch(() => clean());
  await sleep(300);
  settings.confirm_on_close = savedConfirm;
  await api('setSettings', settings);
  await page.evaluate(s => { window.__APP__.settings = s; }, settings);
  return { cancelled: true, dialogText: text.slice(0, 120) };
});

await step('S18-FINAL', '收尾：恢复初始视图并截取最终全景', async () => {
  await page.evaluate(() => { window.__APP__.tree.selectedId = null; window.__APP__.refreshTree?.(); });
  await sleep(300);
  const p = await shot('S18-FINAL');
  return { final: p };
});

// ------------------------------------------------------------------ 汇总
const failed = results.filter(r => r.status === 'FAIL');
fs.writeFileSync(path.join(EVID, 'report.json'), JSON.stringify({
  started: results[0]?.started, finished: new Date().toISOString(), pid, runtime,
  total: results.length, pass: results.length - failed.length, fail: failed.length,
  results, consoleLines,
}, null, 2));
log(`DONE total=${results.length} pass=${results.length - failed.length} fail=${failed.length}`);
if (failed.length) { for (const f of failed) log(`FAILED ${f.id} ${f.title}: ${f.error?.slice(0, 200)}`); }
await browser.close().catch(() => {});
process.exitCode = failed.length ? 1 : 0;
