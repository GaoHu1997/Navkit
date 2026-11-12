import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import { launchDesignerBrowser, designerPage, finishSave } from './table-designer-fixture.mjs';
let browser;
before(async () => { browser = await launchDesignerBrowser(); });
after(async () => { await browser?.close(); });
const tab = (page, name) => page.getByRole('tab', { name, exact: true }).click();
async function saving(page) { await page.evaluate(() => { globalThis.saving = designer.saveChanges(); }); await page.getByRole('dialog', { name: '保存表结构', exact: true }).waitFor(); }

test('real DOM flushes focused edits before add/insert/delete and late blur stays on its stable field', async t => {
  const page = await designerPage(browser); t.after(() => page.close());
  const name = page.getByRole('textbox', { name: '字段 title 名称', exact: true });
  await name.fill('edited_title');
  assert.equal(await name.evaluate(el => document.activeElement === el && el.isConnected), true);
  await page.evaluate(() => { globalThis.oldName = document.activeElement; oldName.addEventListener('blur', () => { fixture.blurred = true; }); designer.insertField(); });
  assert.equal(await page.evaluate(() => fixture.blurred), true);
  assert.deepEqual(await page.evaluate(() => designer.draft.columns.map(c => c.name)), ['id', '', 'edited_title', 'computed', 'updated']);
  await page.getByRole('textbox', { name: '字段 未命名 名称', exact: true }).fill('new_field');
  await page.evaluate(() => { designer.addField(); oldName.value = 'late_wrong'; oldName.dispatchEvent(new FocusEvent('blur')); });
  assert.deepEqual(await page.evaluate(() => designer.draft.columns.map(c => c.name)), ['id', 'new_field', 'edited_title', 'computed', 'updated', '']);
  await page.getByRole('textbox', { name: '字段 未命名 名称', exact: true }).fill('to_remove');
  await page.evaluate(() => designer.deleteField());
  assert.deepEqual(await page.evaluate(() => designer.draft.columns.map(c => c.name)), ['id', 'new_field', 'edited_title', 'computed', 'updated']);
  assert.equal(await page.evaluate(() => designer.draftDirty()), true);
  // 类型只出方言清单：选中值直接显示在选择框；本表既有但不在清单里的类型（MySQL enum）原样保留为选项
  assert.equal(await page.getByLabel('字段 edited_title 类型', { exact: true }).inputValue(), "enum('a,b','c)')");
  const newType = page.getByLabel('字段 new_field 类型', { exact: true });
  await newType.selectOption('bigint');
  assert.equal(await newType.inputValue(), 'bigint');
  assert.equal(await page.evaluate(() => designer.draft.columns[1].data_type), 'bigint');
  const height = await page.getByRole('textbox', { name: '字段 new_field 名称', exact: true }).evaluate(el => getComputedStyle(el).height);
  assert.equal(height, '26px');
});

test('real numeric validation accepts digits and rejects malformed length, scale, blank expressions and fillfactor', async t => {
  const page = await designerPage(browser); t.after(() => page.close());
  const results = await page.evaluate(() => {
    const cases = [], baseline = fixture.snapshot;
    const attempt = change => { const draft = DesignerModules.draftFromSnapshot(baseline); change(draft); try { DesignerModules.payloadFromDraft(baseline, draft); return 'ok'; } catch (error) { return error.message; } };
    cases.push(attempt(d => { d.columns[0].length = '10'; d.columns[0].scale = '2'; }));
    for (const value of ['abc', '1.2', '-1', '12x', '\\d']) cases.push(attempt(d => { d.columns[0].length = value; }));
    cases.push(attempt(d => { d.columns[0].scale = '1x'; }));
    cases.push(attempt(d => { d.columns[0].default_value = '  '; }));
    for (const value of [9, 101, 12.2, NaN, Infinity]) cases.push(attempt(d => { d.options.fillfactor = value; }));
    cases.push(attempt(d => { d.options.fillfactor = 80; })); return cases;
  });
  assert.equal(results[0], 'ok'); assert.equal(results.at(-1), 'ok');
  assert.ok(results.slice(1, -1).every(value => value !== 'ok'));
});

test('column type params are split into length and scale for display', async t => {
  const page = await designerPage(browser); t.after(() => page.close());
  const out = await page.evaluate(() => {
    const s = structuredClone(fixture.snapshot);
    s.design.columns[0].data_type = 'varchar(64)'; s.design.columns[0].length = null; s.design.columns[0].scale = null;
    s.design.columns[1].data_type = 'decimal(18,4)'; s.design.columns[1].length = null; s.design.columns[1].scale = null;
    const d = DesignerModules.draftFromSnapshot(s);
    return { t0: d.columns[0].data_type, l0: d.columns[0].length, t1: d.columns[1].data_type, l1: d.columns[1].length, s1: d.columns[1].scale };
  });
  assert.deepEqual(out, { t0: 'varchar', l0: '64', t1: 'decimal', l1: '18', s1: '4' });
});

test('index and foreign-key controls preserve compound order, real reference columns and ref_schema in one C1 save', async t => {
  const page = await designerPage(browser); t.after(() => page.close());
  await tab(page, '索引');
  // 主键行与索引行都用「字段 + 排序」编辑器维护有序多字段
  assert.deepEqual(await page.evaluate(() => designer.draft.primary_key), ['title', 'id']);
  assert.equal(await page.getByRole('combobox', { name: '主键 字段 1', exact: true }).inputValue(), 'title');
  assert.equal(await page.getByRole('combobox', { name: '主键 字段 2', exact: true }).inputValue(), 'id');
  await page.getByRole('combobox', { name: '索引 1 字段 1', exact: true }).selectOption('id');
  await page.getByRole('button', { name: '为索引 1添加字段', exact: true }).click();
  await page.getByRole('combobox', { name: '索引 1 字段 2', exact: true }).selectOption('title');
  await page.getByRole('combobox', { name: '索引 1 字段 2 排序', exact: true }).selectOption('DESC');
  assert.deepEqual(await page.evaluate(() => designer.draft.indexes[0].columns), ['id', 'title']);
  assert.deepEqual(await page.evaluate(() => designer.draft.indexes[0].descending), ['title']);
  await tab(page, '外键'); await page.getByRole('button', { name: '新增外键', exact: true }).click();
  await page.getByRole('textbox', { name: '外键 1 名称', exact: true }).fill('fk_pair');
  await page.getByRole('combobox', { name: '外键 1 引用库', exact: true }).selectOption('other_schema');
  await page.getByRole('combobox', { name: '外键 1 引用表', exact: true }).selectOption('parents');
  await page.waitForFunction(() => fixture.referenceCalls.length === 1);
  await page.getByRole('combobox', { name: '外键 1 本表字段', exact: true }).fill('title, id');
  await page.getByRole('combobox', { name: '外键 1 引用字段', exact: true }).fill('parent_b, parent_a');
  await page.getByRole('combobox', { name: '外键 1 更新时', exact: true }).selectOption('CASCADE');
  await page.getByRole('combobox', { name: '外键 1 删除时', exact: true }).selectOption('RESTRICT');
  // 编辑流程恰好一次引用字段读取（保存成功后设计器会重建元数据缓存并自动重新证实，不计入本断言）
  const refsBeforeSave = await page.evaluate(() => JSON.parse(JSON.stringify(fixture.referenceCalls)));
  await saving(page);
  assert.match(await page.getByLabel('后端实际 SQL 计划', { exact: true }).textContent(), /ALTER TABLE backend_actual_plan;/);
  await finishSave(page);
  const state = await page.evaluate(() => ({ sent: fixture.executions, refs: fixture.referenceCalls, dirty: designer.draftDirty(), notices: fixture.notices }));
  assert.equal(state.sent.length, 1); assert.deepEqual(state.sent[0].design.primary_key, ['title', 'id']);
  assert.deepEqual(state.sent[0].design.indexes[0].columns, ['id', 'title']);
  assert.deepEqual(state.sent[0].design.foreign_keys[0], { name: 'fk_pair', columns: ['title', 'id'], ref_schema: 'other_schema', ref_table: 'parents', ref_columns: ['parent_b', 'parent_a'], on_update: 'CASCADE', on_delete: 'RESTRICT' });
  assert.deepEqual(refsBeforeSave, [{ connection: 'c', schema: 'other_schema', table: 'parents' }]);
  assert.equal(state.sent[0].design.base_revision, 'r1'); assert.equal(state.sent[0].design.source_fingerprint, 'f1');
  assert.equal(state.sent[0].request.execution_grant, 'fixture-controlled-grant');
  assert.equal(state.sent[0].request.sql, JSON.stringify({ owner: 't', design: state.sent[0].design }));
  assert.equal(state.dirty, false); assert.ok(state.notices.some(n => n[0] === 'success'));
});

test('checks, triggers, table options and full comment are captured, saved and discarded as one draft', async t => {
  const page = await designerPage(browser); t.after(() => page.close());
  await tab(page, '检查'); await page.getByRole('button', { name: '新增检查约束', exact: true }).click();
  await page.getByRole('textbox', { name: '检查约束 1 名称', exact: true }).fill('ck_positive'); await page.getByRole('textbox', { name: '检查约束 1 CHECK 表达式', exact: true }).fill('id > 0');
  await tab(page, '触发器'); await page.getByRole('button', { name: '新增触发器', exact: true }).click();
  await page.getByRole('textbox', { name: '触发器 1 名称', exact: true }).fill('tr_update');
  await page.getByRole('combobox', { name: '触发器 1 时机', exact: true }).selectOption('AFTER'); await page.getByRole('combobox', { name: '触发器 1 事件', exact: true }).selectOption('UPDATE');
  await page.getByRole('textbox', { name: '触发器 1 触发语句', exact: true }).fill('BEGIN SET @seen = NEW.id; END');
  await tab(page, '选项'); await page.getByRole('textbox', { name: '存储引擎', exact: true }).fill('InnoDB'); await page.getByRole('textbox', { name: '行格式', exact: true }).fill('DYNAMIC');
  await tab(page, '注释'); const comment = '第一行\n完整注释，保留引号 \' 与 Unicode'; await page.getByRole('textbox', { name: '表注释', exact: true }).fill(comment);
  const capture = await page.evaluate(() => designer.captureDraft()); assert.equal(capture.payload.draft.checks[0].expression, 'id > 0'); assert.equal(capture.payload.draft.triggers[0].event, 'UPDATE'); assert.equal(capture.payload.draft.options.row_format, 'DYNAMIC'); assert.equal(capture.payload.draft.comment, comment);
  await saving(page); await finishSave(page);
  const design = await page.evaluate(() => fixture.executions[0].design); assert.equal(design.checks[0].name, 'ck_positive'); assert.equal(design.triggers[0].statement, 'BEGIN SET @seen = NEW.id; END'); assert.deepEqual(design.options, { row_format: 'DYNAMIC' }); assert.equal(design.comment, comment);
  await page.getByRole('textbox', { name: '表注释', exact: true }).fill('discard me'); await page.evaluate(() => designer.discardDraft()); assert.equal(await page.getByRole('textbox', { name: '表注释', exact: true }).inputValue(), comment); assert.equal(await page.evaluate(() => designer.draftDirty()), false);
});

test('partial metadata, undefined collections and incomplete rows cannot become implicit drops', async t => {
  const page = await designerPage(browser); t.after(() => page.close());
  const results = await page.evaluate(() => {
    const s = structuredClone(fixture.snapshot); const out = {};
    for (const key of ['indexes', 'foreign_keys', 'checks', 'triggers', 'options', 'comment']) delete s.design[key];
    const draft = DesignerModules.draftFromSnapshot(s); const payload = DesignerModules.payloadFromDraft(s, draft); out.undefinedPreserved = ['indexes', 'foreign_keys', 'checks', 'triggers', 'options', 'comment'].every(key => !Object.hasOwn(payload, key));
    const original = structuredClone(fixture.snapshot); original.design.indexes = [{ name: 'existing', columns: ['id'], unique: false }]; const clear = DesignerModules.draftFromSnapshot(original); clear.indexes = []; out.explicitClear = DesignerModules.payloadFromDraft(original, clear).indexes;
    out.invalid = [];
    const fail = mutate => { const d = DesignerModules.draftFromSnapshot(fixture.snapshot); mutate(d); try { DesignerModules.payloadFromDraft(fixture.snapshot, d); return false; } catch { return true; } };
    out.invalid.push(fail(d => d.indexes.push({ name: '', columns: [], unique: false })), fail(d => d.foreign_keys.push({ name: 'half', columns: ['id'], ref_table: '', ref_columns: [] })), fail(d => d.checks.push({ name: 'half', expression: '' })), fail(d => d.triggers.push({ name: 'half', timing: 'BEFORE', event: 'INSERT', statement: '' })));
    fixture.snapshot.completeness = 'unknown'; try { DesignerModules.draftFromSnapshot(fixture.snapshot); out.unknown = false; } catch { out.unknown = true; } return out;
  });
  assert.equal(results.undefinedPreserved, true); assert.deepEqual(results.explicitClear, []); assert.ok(results.invalid.every(Boolean)); assert.equal(results.unknown, true);
  await tab(page, '索引'); await page.getByRole('button', { name: '新增索引', exact: true }).click(); await page.evaluate(async () => designer.saveChanges());
  assert.equal(await page.evaluate(() => fixture.previews.length), 0); assert.equal(await page.evaluate(() => fixture.executions.length), 0); assert.equal(await page.evaluate(() => designer.draftDirty()), true);
});

test('single-flight save freezes preview, disallows edits and only verified receipt replaces baseline', async t => {
  const page = await designerPage(browser); t.after(() => page.close());
  await tab(page, '注释'); await page.getByRole('textbox', { name: '表注释', exact: true }).fill('new comment');
  await page.evaluate(() => { fixture.holdPreview = true; globalThis.firstSave = designer.saveChanges(); globalThis.secondSave = designer.saveChanges(); });
  assert.equal(await page.evaluate(() => fixture.previews.length), 1); assert.equal(await page.locator('#designer').getAttribute('aria-busy'), 'true'); assert.equal(await page.getByRole('textbox', { name: '表注释', exact: true }).isDisabled(), true);
  await page.evaluate(() => fixture.releasePreview()); await page.getByRole('dialog', { name: '保存表结构', exact: true }).waitFor(); await finishSave(page);
  assert.equal(await page.evaluate(() => fixture.executions.length), 1); assert.equal(await page.evaluate(() => fixture.grants.length), 1); assert.equal(await page.evaluate(() => designer.snapshot.revision), 'r2'); assert.equal(await page.evaluate(() => designer.draftDirty()), false);
});

for (const mode of ['partial', 'unknown']) test(`${mode} receipt preserves the whole draft and forbids replay or discard`, async t => {
  const page = await designerPage(browser); t.after(() => page.close()); await tab(page, '注释'); await page.getByRole('textbox', { name: '表注释', exact: true }).fill(`keep ${mode}`); await page.evaluate(mode => { fixture.mode = mode; }, mode);
  await saving(page); await finishSave(page); await page.evaluate(async () => designer.saveChanges());
  assert.equal(await page.evaluate(() => fixture.executions.length), 1); assert.equal(await page.evaluate(() => designer.draftDirty()), true); assert.equal(await page.evaluate(() => designer.captureDraft().payload.outcome_requires_reconciliation), true);
  assert.equal(await page.evaluate(() => { try { designer.discardDraft(); return false; } catch { return true; } }), true);
  assert.ok((await page.evaluate(() => fixture.notices)).every(n => n[0] !== 'success'));
});

test('CAS preview rejection, readonly and manual transaction guards never send an execution', async t => {
  for (const mode of ['cas', 'readonly', 'transaction']) {
    const page = await designerPage(browser); t.after(() => page.close()); await tab(page, '注释'); await page.getByRole('textbox', { name: '表注释', exact: true }).fill(mode);
    await page.evaluate(async mode => { if (mode === 'cas') fixture.mode = mode; else fixture[mode === 'readonly' ? 'readOnly' : mode] = true; await designer.saveChanges(); }, mode);
    assert.equal(await page.evaluate(() => fixture.executions.length), 0); assert.equal(await page.evaluate(() => designer.draftDirty()), true);
  }
});

test('failed reload preserves complete baseline and edits; successful reloadAfterLeave accepts a real new snapshot', async t => {
  const page = await designerPage(browser); t.after(() => page.close()); await tab(page, '注释'); await page.getByRole('textbox', { name: '表注释', exact: true }).fill('unsaved');
  const preserved = await page.evaluate(async () => { fixture.snapshotError = true; const before = JSON.stringify(designer.captureDraft()); try { await designer.reloadAfterLeave(); } catch {} return before === JSON.stringify(designer.captureDraft()); }); assert.equal(preserved, true);
  await page.evaluate(async () => { fixture.snapshotError = false; fixture.snapshot.design.comment = 'new server baseline'; await designer.reloadAfterLeave(); }); assert.equal(await page.getByRole('textbox', { name: '表注释', exact: true }).inputValue(), 'new server baseline'); assert.equal(await page.evaluate(() => designer.draftDirty()), false);
});

test('column order uses a full baseline-bound C1 design, preserves compound PK and refuses mixed/protocol drafts', async t => {
  const page = await designerPage(browser); t.after(() => page.close()); await page.getByRole('textbox', { name: '字段 computed 名称', exact: true }).focus(); await page.evaluate(() => { designer.moveField(-1); designer.moveField(-1); });
  assert.equal(await page.evaluate(() => document.activeElement?.value), 'computed'); assert.deepEqual(await page.evaluate(() => designer.draft.columns.map(c => c.name)), ['computed', 'id', 'title', 'updated']);
  await saving(page); await finishSave(page); const design = await page.evaluate(() => fixture.executions[0].design); assert.deepEqual(design.columns.map(c => c.orig_name), ['computed', 'id', 'title', 'updated']); assert.deepEqual(design.primary_key, ['title', 'id']); assert.equal(Object.hasOwn(design, 'indexes'), false);
  await page.getByLabel('字段 id 类型', { exact: true }).selectOption('int'); assert.match(await page.evaluate(() => designer.fieldMoveReason(1)), /不能与其它结构修改混合/);
  for (const kind of ['sqlite', 'postgresql']) { const unsupported = await designerPage(browser, kind); t.after(() => unsupported.close()); assert.match(await unsupported.evaluate(() => designer.fieldMoveReason(1)), /不支持安全原地调整/); }
});

test('keyboard tabs, modal focus containment and actual SQL text are accessible', async t => {
  const page = await designerPage(browser); t.after(() => page.close()); await page.getByRole('tab', { name: '字段', exact: true }).focus(); await page.keyboard.press('ArrowRight'); assert.equal(await page.getByRole('tab', { name: '索引', exact: true }).getAttribute('aria-selected'), 'true'); await page.keyboard.press('End'); assert.equal(await page.getByRole('tab', { name: 'SQL 预览', exact: true }).getAttribute('aria-selected'), 'true');
  await page.getByRole('button', { name: '生成后端实际计划', exact: true }).click(); await page.waitForFunction(() => document.querySelector('.designer-sql')?.textContent.includes('backend_actual_plan'));
  const text = await page.locator('.designer-sql').textContent(); assert.ok(text.includes('\nALTER TABLE backend_actual_plan;')); assert.equal(text.includes('\\n'), false);
  await tab(page, '注释'); await page.getByRole('textbox', { name: '表注释', exact: true }).fill('keyboard'); await saving(page);
  const confirm = page.getByRole('button', { name: '确认执行', exact: true }); await confirm.focus(); await page.keyboard.press('Tab'); assert.equal(await page.evaluate(() => document.activeElement?.closest('[role="dialog"]') !== null), true); await page.keyboard.press('Escape'); await page.waitForFunction(() => !designer.draftBusy()); assert.equal(await page.evaluate(() => fixture.executions.length), 0);
});

test('SQLite unsupported options/comments and capability-disabled checks/triggers show concrete reasons', async t => {
  const page = await designerPage(browser, 'sqlite'); t.after(() => page.close()); await tab(page, '选项'); assert.equal(await page.getByRole('textbox', { name: '存储引擎', exact: true }).isDisabled(), true); assert.equal(await page.getByRole('textbox', { name: '排序规则', exact: true }).isDisabled(), true); assert.match(await page.getByRole('tabpanel').textContent(), /SQLite 没有此表级选项/);
  await tab(page, '注释'); assert.equal(await page.getByRole('textbox', { name: '表注释', exact: true }).isDisabled(), true); assert.match(await page.getByRole('tabpanel').textContent(), /SQLite 没有表注释 DDL/);
  await page.evaluate(() => { designer.snapshot.capabilities.checks = { supported: false, reason: 'DDL checks cannot be safely rebuilt' }; designer.snapshot.capabilities.triggers = { supported: false, reason: 'trigger protocol unavailable' }; });
  await tab(page, '检查'); assert.equal(await page.getByRole('button', { name: '新增检查约束', exact: true }).isDisabled(), true); assert.match(await page.getByRole('tabpanel').textContent(), /DDL checks cannot/); await tab(page, '触发器'); assert.equal(await page.getByRole('button', { name: '新增触发器', exact: true }).isDisabled(), true);
});

for (const kind of ['clear', 'copy_structure', 'copy_data']) test(`${kind} operation requires backend preview, keeps confirmation and refreshes only the receipt target`, async t => {
  const page = await designerPage(browser); t.after(() => page.close()); await page.evaluate(kind => DesignerModules.openTableOperationDialog({ connectionId: 'c', schema: 'db', table: 'items', kind, ownerId: 'operation-owner', writeReason: () => null, applied: async (receipt, schema) => fixture.refreshed.push({ table: receipt.target_table ?? receipt.source_table, schema }), toast: (kind, message) => fixture.notices.push([kind, message]) }), kind);
  const confirm = page.getByRole('button', { name: kind === 'clear' ? '确认清空全部数据' : '确认创建复制表', exact: true }); assert.equal(await confirm.isDisabled(), true);
  if (kind !== 'clear') { await page.getByRole('textbox', { name: '复制目标表名', exact: true }).fill('items'); await page.getByRole('button', { name: '预览后端计划', exact: true }).click(); assert.equal(await page.evaluate(() => fixture.previews.length), 0); await page.getByRole('textbox', { name: '复制目标表名', exact: true }).fill('new_target'); }
  else assert.match(await page.getByRole('dialog').textContent(), /清空全部数据/);
  await page.getByRole('button', { name: '预览后端计划', exact: true }).click(); await page.waitForFunction(() => fixture.previews.length === 1); assert.match(await page.getByLabel('后端实际 SQL 计划', { exact: true }).textContent(), /BACKEND_OPERATION_PLAN/); assert.equal(await page.evaluate(() => fixture.executions.length), 0);
  await confirm.click(); await page.waitForFunction(() => fixture.refreshed.length === 1); assert.equal(await confirm.isDisabled(), true); assert.deepEqual(await page.evaluate(() => fixture.refreshed), [{ schema: 'db', table: kind === 'clear' ? 'items' : 'new_target' }]); assert.equal(await page.evaluate(() => fixture.executions.length), 1); assert.equal(await page.evaluate(() => fixture.executions[0].request.execution_grant), 'fixture-controlled-grant');
});
