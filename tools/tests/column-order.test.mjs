import test, { after } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const read = file => fs.readFileSync(path.join(root, file), 'utf8');
const definitions = [
  '`id` bigint unsigned NOT NULL AUTO_INCREMENT',
  "`title` enum('a,b','c)') CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL DEFAULT 'a,b' COMMENT '引号,括号)'",
  '`computed` int GENERATED ALWAYS AS ((`id` + length(\'x,y\'))) STORED',
  '`updated` timestamp(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3)',
];
const ddl = (order = ['id', 'title', 'computed', 'updated']) => 'CREATE TABLE `items` (\n' + order.map(name => definitions.find(d => d.startsWith('`' + name + '`'))).join(',\n') + ',\nPRIMARY KEY (`title`,`id`),\nKEY `title_idx` (`title`),\nCONSTRAINT `items_check` CHECK ((`id` > 0))\n) ENGINE=InnoDB';
const json = value => JSON.parse(JSON.stringify(value));

// 纯 helper 测试单独加载无依赖模块，绝不构造 App 或初始化 UI。
function columnHelpers() {
  const context = vm.createContext({ exports: {} });
  vm.runInContext(ts.transpile(read('src/core/columnOrder.ts'), { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS }), context);
  return context.exports;
}
let browser, browserFixture;
after(async () => { await browser?.close(); });
async function loaded(t, kind) {
  browserFixture ??= await import('./column-order-browser-fixture.mjs');
  browser ??= await browserFixture.launchDesignerBrowser();
  const page = await browserFixture.columnOrderPage(browser, kind); t.after(() => page.close()); return page;
}
const selectField = (...args) => browserFixture.selectField(...args);
const reorderColumns = (...args) => browserFixture.reorderColumns(...args);
const startSave = (...args) => browserFixture.startSave(...args);
const finishSave = (...args) => browserFixture.finishSave(...args);
const desiredOrder = ['computed', 'id', 'title', 'updated'];
const state = page => page.evaluate(() => ({ columns: designer.draft.columns, dirty: view.draftDirty(), tabDirty: tab.dirty, busy: view.isSavingStructure(), previews: fixture.previews, executions: fixture.executions, grants: fixture.grants, notices: fixture.notices, snapshot: designer.snapshot, baseline: designer.baselineDraft, schema: tab.schemaInfo }));

test('physical column order capability fails closed for other dialects, ODBC and read-only', () => {
  const c = columnHelpers();
  assert.equal(c.columnOrderCapability('mysql'), null); assert.equal(c.columnOrderCapability('mariadb'), null);
  for (const kind of ['sqlite', 'postgresql', 'sqlserver', 'tidb']) assert.match(c.columnOrderCapability(kind), /不支持安全原地调整/);
  assert.match(c.columnOrderCapability('mysql', '@navkit-driver:{}'), /自定义驱动/);
  assert.match(c.columnOrderCapability('mysql', '', true), /只读/);
});

test('SHOW CREATE lexer preserves enum commas, generated expressions, collation and ON UPDATE', () => {
  const c = columnHelpers();
  assert.deepEqual(json([...c.mysqlColumnDefinitions(ddl()).values()]), definitions);
  const tricky = "CREATE TABLE `a(b` (`a` varchar(20) DEFAULT 'can\\'t,(split)', `b` int /* , ) */ DEFAULT (length('a,b')), -- comma, parenthesis )\n `c``x` varchar(40) DEFAULT 'it''s,one', CHECK (`b` > 0))";
  const parts = c.mysqlColumnDefinitions(tricky);
  assert.equal(parts.size, 2); // 非标准 SHOW CREATE 的前置行注释不能冒充可复用的字段。
  assert.match(parts.get('a'), /can\\'t,\(split\)/);
  assert.match(parts.get('b'), /\/\* , \) \*\//);
});

test('one ALTER reuses full definitions and never drops or rebuilds compound PK/index/constraint', () => {
  const c = columnHelpers();
  const sql = c.mysqlColumnOrderSql(ddl(), 'db', 'items', ['id', 'title', 'computed', 'updated'], ['computed', 'updated', 'id', 'title']);
  assert.match(sql, /^ALTER TABLE `db`\.`items`/);
  assert.equal((sql.match(/MODIFY COLUMN/g) ?? []).length, 2);
  assert.ok(sql.includes(definitions[2] + ' FIRST'));
  assert.ok(sql.includes(definitions[3] + ' AFTER `computed`'));
  assert.doesNotMatch(sql, /DROP|CREATE|PRIMARY KEY|CONSTRAINT|KEY `title_idx`/);
  assert.equal(c.mysqlColumnOrderSql(ddl(), '', 'items', ['id', 'title', 'computed', 'updated'], ['id', 'title', 'computed', 'updated']), null);
});

test('identifier quoting is safe for database, table and malicious column names', () => {
  const c = columnHelpers();
  const name = 'x`; DROP TABLE y; --';
  const escaped = name.replace(/`/g, '``');
  const sql = c.mysqlColumnOrderSql('CREATE TABLE `t` (`id` int, `' + escaped + '` int)', 'a`b', 't`z', ['id', name], [name, 'id']);
  assert.ok(sql.startsWith('ALTER TABLE `a``b`.`t``z`'));
  assert.ok(sql.includes('MODIFY COLUMN `' + escaped + '` int FIRST'));
});

test('malformed, duplicate, renamed and concurrent order definitions are rejected', () => {
  const c = columnHelpers();
  for (const input of ['DROP TABLE x', 'CREATE TABLE x (`a` int', 'CREATE TABLE x (`a` int, `a` int)', 'CREATE TABLE x (a int)']) assert.throws(() => c.mysqlColumnDefinitions(input));
  assert.throws(() => c.mysqlColumnOrderSql(ddl(), 'db', 't', ['wrong'], ['wrong']), /已发生变化/);
  assert.throws(() => c.mysqlColumnOrderSql(ddl(), 'db', 't', ['id', 'title', 'computed', 'updated'], ['id', 'title', 'computed', 'renamed']), /完整保留/);
});

test('real App toolbar enables interior moves and exposes first/last/protocol boundary reasons', async t => {
  const page = await loaded(t); await selectField(page, 'title');
  const up = page.locator('.tab-toolbar [data-command="move-up"]'), down = page.locator('.tab-toolbar [data-command="move-down"]');
  assert.equal(await up.isEnabled(), true); assert.equal(await down.isEnabled(), true);
  await selectField(page, 'id'); assert.equal(await up.isDisabled(), true); assert.match(await up.getAttribute('aria-description'), /第一个/);
  await selectField(page, 'updated'); assert.equal(await down.isDisabled(), true); assert.match(await down.getAttribute('title'), /最后一个/);
  await page.evaluate(() => { cfg.kind = 'sqlite'; app.notifyToolbar(); });
  assert.equal(await up.isDisabled(), true); assert.match(await up.getAttribute('aria-description'), /不支持安全原地调整/);
});

test('actual runCommand reorders the selected stable row, preserves focus and restores clean at baseline', async t => {
  const page = await loaded(t); await selectField(page, 'title'); await page.evaluate(() => app.runCommand('move-up'));
  assert.deepEqual((await state(page)).columns.map(c => c.name), ['title', 'id', 'computed', 'updated']);
  assert.equal((await state(page)).tabDirty, true);
  assert.deepEqual(await page.evaluate(() => ({ name: designer.draft.columns.find(c => c.key === designer.selected).name, focused: document.activeElement.value })), { name: 'title', focused: 'title' });
  await page.evaluate(() => app.runCommand('move-down'));
  const clean = await state(page); assert.deepEqual(clean.columns.map(c => c.name), ['id', 'title', 'computed', 'updated']);
  assert.equal(clean.dirty, false); assert.equal(clean.tabDirty, false); assert.equal(await page.evaluate(() => document.activeElement.value), 'title');
});

test('keyboard focus selects a field without replacing its live input, and rename blocks mixed moves', async t => {
  const page = await loaded(t);
  await selectField(page, 'title');
  // 由上一行最后一个控件按 Tab 进入下一行，验证真实 focusin 生命周期。
  await page.evaluate(() => { globalThis.liveInput = document.querySelectorAll('.designer-grid tbody tr')[2].querySelector('input'); });
  await page.getByRole('textbox', { name: '字段 title 注释', exact: true }).focus(); await page.keyboard.press('Tab');
  assert.deepEqual(await page.evaluate(() => ({ connected: liveInput.isConnected, same: document.activeElement === liveInput, selected: designer.selected === designer.draft.columns[2].key })), { connected: true, same: true, selected: true });
  await page.keyboard.type('renamed_');
  const typed = await page.evaluate(() => liveInput.value); await page.keyboard.press('Tab');
  assert.equal((await state(page)).columns[2].name, typed); assert.match(await page.evaluate(() => view.fieldMoveReason(-1)), /不能与其它结构修改混合/);
  assert.equal(await page.evaluate(() => liveInput.isConnected), true);
});

test('reorder previews then uses C1 authorization with exact immutable baseline and normalized receipt', async t => {
  const page = await loaded(t); const before = await state(page); await reorderColumns(page);
  await page.evaluate(() => { const execute = __api.executeTableDesign; __api.executeTableDesign = async (...args) => { const response = await execute(...args); fixture.returnedReceipt = structuredClone(response.receipt); return response; }; });
  await startSave(page, true);
  const pending = await state(page); assert.equal(pending.previews.length, 1); assert.equal(pending.grants.length, 0); assert.equal(pending.executions.length, 0);
  assert.match(await page.getByLabel('后端实际 SQL 计划', { exact: true }).textContent(), /ALTER TABLE backend_actual_plan;/);
  const design = pending.previews[0];
  assert.deepEqual(design.columns, desiredOrder.map(name => before.snapshot.design.columns.find(c => c.name === name)));
  assert.equal(design.schema, 'db'); assert.equal(design.table, 'items'); assert.equal(design.base_revision, before.snapshot.revision); assert.equal(design.source_fingerprint, before.snapshot.source_fingerprint);
  assert.deepEqual(design.primary_key, ['title', 'id']);
  for (const preserved of ['indexes', 'foreign_keys', 'checks', 'triggers', 'options', 'comment']) assert.equal(Object.hasOwn(design, preserved), false, `${preserved} 未改定义不能隐式重建`);
  await finishSave(page);
  const saved = await state(page); assert.equal(saved.executions.length, 1); assert.equal(saved.grants.length, 1); assert.deepEqual(saved.executions[0].design, design);
  const request = saved.executions[0].request;
  assert.deepEqual(saved.grants[0], { execution_id: request.execution_id, connection_id: 'c', namespace: 'db', owner_id: 't', sql: JSON.stringify({ owner: 't', design }), origin: 'table-design' });
  assert.equal(request.execution_grant, 'fixture-controlled-grant'); assert.equal(request.cancel_before_send, false);
  const receipt = await page.evaluate(() => fixture.returnedReceipt); assert.equal(receipt.verified, true); assert.equal(receipt.unverified, false); assert.deepEqual(receipt.mismatches, []);
  assert.deepEqual(saved.snapshot, receipt.snapshot); assert.equal(saved.snapshot.revision, 'r2'); assert.equal(saved.snapshot.source_fingerprint, 'f2');
  assert.deepEqual(saved.columns.map(c => c.name), desiredOrder); assert.deepEqual(saved.columns.map(c => c.orig_name), desiredOrder);
  assert.deepEqual(saved.schema.indexes[0], before.schema.indexes[0]); assert.deepEqual(saved.snapshot.design.indexes, before.snapshot.design.indexes);
  assert.equal(saved.dirty, false); assert.equal(saved.tabDirty, false); assert.equal(saved.busy, false); assert.ok(saved.notices.some(n => n[0] === 'success' && /完整读回验证通过/.test(n[1])));
});

test('simulated backend CAS rejects metadata and hidden source_definition changes without rebasing the frontend draft', async t => {
  for (const mode of ['metadata', 'source_definition']) {
    const page = await loaded(t); await reorderColumns(page); const original = await state(page);
    await page.evaluate(mode => {
      fixture.backendBefore = structuredClone(fixture.snapshot);
      if (mode === 'metadata') fixture.snapshot.schema.columns[0].type = 'varchar(20)';
      else fixture.snapshot.objects.table_definition = "CREATE TABLE items (id bigint, computed int GENERATED ALWAYS AS (length('changed')))";
      fixture.snapshot.revision = 'backend-new-revision'; fixture.snapshot.source_fingerprint = 'backend-new-fingerprint';
      // 后端拒绝的模拟只证明前端请求/保稿行为，不是原生驱动 CAS 验收。
      __api.previewTableDesign = async (connection, design, owner) => {
        fixture.previews.push(structuredClone(design)); fixture.cas = { connection, owner, changed: JSON.stringify(fixture.backendBefore) !== JSON.stringify(fixture.snapshot), mode };
        if (design.base_revision !== fixture.snapshot.revision || design.source_fingerprint !== fixture.snapshot.source_fingerprint) throw new Error('BASE_REVISION_CONFLICT: metadata/source_definition 已发生变化');
        throw new Error('fixture 必须拒绝旧基线');
      };
    }, mode);
    await page.evaluate(() => view.saveChanges()); const rejected = await state(page);
    assert.equal(rejected.previews.length, 1); assert.equal(rejected.previews[0].base_revision, original.snapshot.revision); assert.equal(rejected.previews[0].source_fingerprint, original.snapshot.source_fingerprint);
    assert.deepEqual(await page.evaluate(() => fixture.cas), { connection: 'c', owner: 't', changed: true, mode });
    assert.deepEqual(rejected.snapshot, original.snapshot); assert.deepEqual(rejected.columns, original.columns); assert.equal(rejected.dirty, true); assert.equal(rejected.grants.length, 0); assert.equal(rejected.executions.length, 0);
    assert.match(rejected.notices.at(-1)[1], /BASE_REVISION_CONFLICT.*草稿已保留/); assert.ok(rejected.notices.every(n => n[0] !== 'success'));
  }
});

test('mixed edits, readonly, unsupported dialect and readonly during preview all refuse execution', async t => {
  for (const mode of ['mixed', 'readonly', 'sqlite', 'changed']) {
    const page = await loaded(t); await reorderColumns(page);
    if (mode === 'mixed') await page.getByRole('textbox', { name: '字段 computed 注释', exact: true }).fill('changed');
    await page.evaluate(mode => { if (mode === 'readonly') cfg.read_only = true; if (mode === 'sqlite') cfg.kind = 'sqlite'; if (mode === 'changed') fixture.holdPreview = true; }, mode);
    if (mode === 'changed') {
      await page.evaluate(() => { globalThis.savePromise = view.saveChanges(); }); await page.waitForFunction(() => !!fixture.releasePreview);
      await page.evaluate(() => { cfg.read_only = true; fixture.releasePreview(); }); await page.getByRole('dialog', { name: '保存表结构', exact: true }).waitFor(); await finishSave(page);
    } else await page.evaluate(() => view.saveChanges());
    const rejected = await state(page); assert.equal(rejected.executions.length, 0, mode); assert.equal(rejected.grants.length, 0, mode); assert.equal(rejected.dirty, true, mode); assert.deepEqual(rejected.columns.map(c => c.name), desiredOrder);
    assert.match(rejected.notices.at(-1)[1], mode === 'mixed' ? /不能与其它结构修改混合/ : mode === 'sqlite' ? /不支持安全原地调整/ : /只读/);
  }
});

test('single-flight preview blocks every direct field mutation and duplicate save', async t => {
  const page = await loaded(t); await reorderColumns(page);
  await page.evaluate(() => { fixture.holdPreview = true; globalThis.savePromise = view.saveChanges(); }); await page.waitForFunction(() => !!fixture.releasePreview);
  const frozen = await state(page); assert.equal(frozen.busy, true);
  const unchanged = await page.evaluate(async () => {
    const before = JSON.stringify(designer.draft); const results = [];
    for (const mutate of [() => view.addField(), () => view.insertField(), () => view.deleteField(), () => view.togglePrimaryKey(), () => view.moveField(1), () => view.moveField(-1)]) { mutate(); results.push(JSON.stringify(designer.draft) === before); }
    await view.saveChanges(); return results;
  }); assert.deepEqual(unchanged, [true, true, true, true, true, true]); assert.deepEqual((await state(page)).columns, frozen.columns); assert.equal((await state(page)).previews.length, 1);
  await page.evaluate(() => fixture.releasePreview()); await page.getByRole('dialog', { name: '保存表结构', exact: true }).waitFor(); await finishSave(page);
  const saved = await state(page); assert.equal(saved.executions.length, 1); assert.equal(saved.grants.length, 1); assert.equal(saved.busy, false);
});

test('failed, partial, unknown and readback errors preserve draft/evidence without success or replay', async t => {
  for (const mode of ['execute', 'failed', 'partial', 'unknown', 'order', 'readback']) {
    const page = await loaded(t); await reorderColumns(page); const before = await state(page);
    await page.evaluate(mode => {
      const execute = __api.executeTableDesign;
      __api.executeTableDesign = async (...args) => {
        fixture.mode = mode === 'partial' || mode === 'unknown' ? mode : 'success';
        const response = await execute(...args);
        if (mode === 'execute') throw new Error('database denied ALTER');
        if (mode === 'failed') { response.outcome.state = 'failed'; response.outcome.error = { message: 'DDL failed', code: 'DDL_FAILED', retryable: false }; response.receipt.verified = false; }
        if (mode === 'order') { response.receipt.verified = false; response.receipt.unverified = true; response.receipt.mismatches = ['实际字段顺序不同']; response.receipt.snapshot = structuredClone(designer.snapshot); }
        if (mode === 'readback') { response.receipt.verified = false; response.receipt.unverified = true; response.receipt.snapshot = null; response.receipt.mismatches = ['readback offline']; }
        return response;
      };
    }, mode);
    await startSave(page); await page.getByRole('button', { name: '确认执行', exact: true }).click();
    await page.waitForFunction(() => designer.saving === false);
    if (mode === 'execute') await page.waitForFunction(() => !view.draftBusy());
    const rejected = await state(page); assert.equal(rejected.executions.length, 1); assert.deepEqual(rejected.columns, before.columns); assert.deepEqual(rejected.snapshot, before.snapshot); assert.equal(rejected.dirty, true); assert.ok(rejected.notices.every(n => n[0] !== 'success'));
    assert.match(rejected.notices.at(-1)[1], mode === 'execute' ? /database denied ALTER.*原草稿保留/ : /回执未证实完整成功.*原草稿保留/);
    assert.equal(await page.evaluate(() => view.captureDraft().payload.outcome_requires_reconciliation), true);
    assert.equal(await page.evaluate(() => { try { view.discardDraft(); return false; } catch { return true; } }), true);
    await page.evaluate(() => view.saveChanges()); assert.equal((await state(page)).executions.length, 1); assert.equal((await state(page)).grants.length, 1);
  }
});

test('real DOM exposes accessible names/status and busy tabs cannot recreate enabled editors', async t => {
  const page = await loaded(t); assert.equal(await page.getByRole('textbox', { name: '字段 id 名称', exact: true }).count(), 1);
  await reorderColumns(page); assert.match(await page.locator('.designer-status').textContent(), /未保存.*草稿/); assert.equal(await page.locator('.designer-status').getAttribute('aria-live'), 'polite');
  await page.evaluate(() => { fixture.holdPreview = true; globalThis.savePromise = view.saveChanges(); }); await page.waitForFunction(() => !!fixture.releasePreview);
  assert.equal(await page.locator('#designer').getAttribute('aria-busy'), 'true');
  const selected = await page.evaluate(() => designer.selected);
  await page.locator('.designer-grid tbody tr').last().dispatchEvent('mousedown');
  assert.equal(await page.evaluate(() => designer.selected), selected);
  await page.getByRole('tab', { name: '选项', exact: true }).click();
  assert.equal(await page.getByRole('textbox', { name: '排序规则', exact: true }).isDisabled(), true);
  await page.getByRole('tab', { name: '字段', exact: true }).click();
  assert.equal(await page.locator('.designer-grid input:not(:disabled), .designer-grid select:not(:disabled)').count(), 0);
  assert.equal(await page.locator('.tab-toolbar button:not(:disabled)').count(), 0);
  await page.evaluate(() => fixture.releasePreview()); await page.getByRole('dialog', { name: '保存表结构', exact: true }).waitFor(); await finishSave(page);
  assert.equal(await page.locator('#designer').getAttribute('aria-busy'), 'false'); assert.match(await page.locator('.designer-notice[role="status"]').textContent(), /已保存.*完整读回/); assert.match(await page.locator('.designer-status').textContent(), /读取基线一致/);
});

test('immutable snapshot namespace fully qualifies empty tab schema while an unknown namespace blocks moves', async t => {
  const page = await loaded(t); await page.evaluate(async () => { tab.schema = ''; await view.reloadAfterLeave(); }); await reorderColumns(page);
  await startSave(page); assert.match(await page.getByRole('dialog', { name: '保存表结构', exact: true }).textContent(), /目标：db\.items/); await finishSave(page);
  const saved = await state(page); assert.equal(saved.executions[0].design.schema, 'db'); assert.equal(saved.executions[0].request.namespace, 'db'); assert.equal(saved.executions[0].design.table, 'items');
  const unknown = await loaded(t); await unknown.evaluate(async () => { tab.schema = ''; fixture.snapshot.design.schema = ''; fixture.snapshot.schema.database = ''; await view.reloadAfterLeave(); }); await selectField(unknown, 'title');
  assert.match(await unknown.evaluate(() => view.fieldMoveReason(-1)), /未知命名空间/); const before = await state(unknown); await unknown.evaluate(() => app.runCommand('move-up'));
  const rejected = await state(unknown); assert.deepEqual(rejected.columns, before.columns); assert.equal(rejected.dirty, false); assert.equal(rejected.previews.length, 0); assert.equal(rejected.executions.length, 0);
});

test('ordinary appended field uses structured C1 payload and an unverified receipt keeps the whole draft', async t => {
  const page = await loaded(t); await page.evaluate(() => view.addField());
  await page.getByRole('textbox', { name: '字段 未命名 名称', exact: true }).fill('new_value');
  await page.getByLabel('字段 未命名 类型', { exact: true }).selectOption('int');
  await page.evaluate(() => { fixture.mode = 'partial'; }); await startSave(page);
  const preview = (await state(page)).previews[0]; assert.equal(preview.schema, 'db'); assert.equal(preview.table, 'items'); assert.equal(preview.base_revision, 'r1'); assert.equal(preview.source_fingerprint, 'f1');
  assert.deepEqual(preview.columns.filter(c => c.orig_name === null).map(c => [c.name, c.data_type]), [['new_value', 'int']]); assert.deepEqual(preview.columns.filter(c => c.orig_name !== null).map(c => c.orig_name), ['id', 'title', 'computed', 'updated']);
  await finishSave(page); const rejected = await state(page); assert.equal(rejected.executions.length, 1); assert.equal(rejected.executions[0].request.connection_id, 'c'); assert.deepEqual(rejected.executions[0].design, preview); assert.equal(rejected.dirty, true); assert.equal(rejected.columns.at(-1).name, 'new_value'); assert.ok(rejected.notices.every(n => n[0] !== 'success'));
});

test('actual App manual transaction gate disables toolbar and runCommand without mutating the draft', async t => {
  const page = await loaded(t); await selectField(page, 'title'); const before = await state(page);
  await page.evaluate(() => { fixture.transactionStatus = { active: true, schema: 'db', owner: 'query-owner', failed: false }; app.notifyToolbar(); });
  for (const id of ['move-up', 'move-down', 'save', 'add-field', 'insert-field', 'delete-field', 'primary-key']) {
    const button = page.locator(`.tab-toolbar [data-command="${id}"]`); assert.equal(await button.isDisabled(), true); assert.match(await button.getAttribute('aria-description'), /手动事务/);
    await page.evaluate(id => app.runCommand(id), id);
  }
  let rejected = await state(page); assert.deepEqual(rejected.columns, before.columns); assert.deepEqual(rejected.snapshot, before.snapshot); assert.equal(rejected.dirty, false); assert.equal(rejected.previews.length, 0); assert.equal(rejected.grants.length, 0); assert.equal(rejected.executions.length, 0);
  await page.evaluate(() => { fixture.transactionStatus = undefined; fixture.transactionBusy = true; app.notifyToolbar(); });
  assert.match(await page.evaluate(() => app.commandUnavailableReason('move-down')), /事务操作正在进行/); await page.evaluate(() => app.runCommand('move-down'));
  rejected = await state(page); assert.deepEqual(rejected.columns, before.columns);
  await page.evaluate(() => { fixture.transactionBusy = false; app.notifyToolbar(); }); assert.equal(await page.evaluate(() => app.commandUnavailableReason('move-up')), null); assert.equal(await page.locator('.tab-toolbar [data-command="move-up"]').isEnabled(), true);
});
