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
  assert.equal(height, '30px');
});

test('field grid keeps type select, single-box default with choices, clickable key on every row and extension popover', async t => {
  const page = await designerPage(browser); t.after(() => page.close());
  // 字段属性面板已并入表格列
  assert.equal(await page.locator('.designer-field-props').count(), 0);
  assert.deepEqual(await page.evaluate(() => [...document.querySelectorAll('.designer-grid.designer-fields thead th')].map(th => th.textContent)), ['名称', '类型', '长度', '小数位', '默认值', '主键', '允许 NULL', '扩展', '注释']);
  // 表头、colgroup 与每行单元格数必须一致，否则表格会多出空白列把内容挤到左边
  assert.equal(await page.evaluate(() => {
    const table = document.querySelector('.designer-grid.designer-fields');
    const count = table.querySelectorAll('thead th').length;
    return table.querySelectorAll('colgroup col').length === count && [...table.querySelectorAll('tbody tr')].every(tr => tr.children.length === count);
  }), true);
  // 类型是纯下拉：只能选方言清单里的类型
  await page.getByLabel('字段 computed 类型', { exact: true }).selectOption('bigint');
  assert.equal(await page.evaluate(() => designer.draft.columns[2].data_type), 'bigint');
  // 默认值：一个框承担回显与手输，候选浮层选中后直接回显
  await page.getByRole('button', { name: '字段 computed 默认值候选', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => [...document.querySelectorAll('.popover-option-value')].map(el => el.textContent)), ['0', '1', 'NULL']);
  await page.getByRole('button', { name: '0', exact: true }).click();
  assert.equal(await page.evaluate(() => designer.draft.columns[2].default_value), '0');
  assert.equal(await page.getByRole('textbox', { name: '字段 computed 默认值', exact: true }).inputValue(), '0');
  await page.getByRole('textbox', { name: '字段 computed 默认值', exact: true }).fill('CURRENT_TIMESTAMP');
  assert.equal(await page.evaluate(() => designer.draft.columns[2].default_value), 'CURRENT_TIMESTAMP');
  // 时间类型的候选随类型变化
  await page.getByRole('button', { name: '字段 updated 默认值候选', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => [...document.querySelectorAll('.popover-option-value')].map(el => el.textContent)), ['CURRENT_TIMESTAMP', 'CURRENT_DATE', 'CURRENT_TIME', 'NULL']);
  await page.keyboard.press('Escape');
  // 主键钥匙每行都有：点一下加入，再点一下移出
  assert.equal(await page.locator('.designer-grid.designer-fields .designer-key').count(), 4);
  assert.deepEqual(await page.evaluate(() => [...document.querySelectorAll('.designer-fields .designer-key.active')].map(el => el.getAttribute('aria-label'))), ['字段 id 主键', '字段 title 主键']);
  await page.getByRole('button', { name: '字段 computed 主键', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => designer.draft.primary_key), ['title', 'id', 'computed']);
  await page.getByRole('button', { name: '字段 computed 主键', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => designer.draft.primary_key), ['title', 'id']);
  // 自增 / 无符号 / 填充零收进扩展浮层，按类型决定是否可选
  await page.getByRole('button', { name: '字段 updated 扩展', exact: true }).click();
  assert.equal(await page.getByRole('checkbox', { name: '字段 updated 自增', exact: true }).isDisabled(), true);
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: '字段 computed 扩展', exact: true }).click();
  assert.equal(await page.getByRole('checkbox', { name: '字段 computed 自增', exact: true }).isDisabled(), false);
  assert.equal(await page.getByRole('checkbox', { name: '字段 computed 无符号', exact: true }).isDisabled(), false);
  await page.getByRole('checkbox', { name: '字段 computed 无符号', exact: true }).click();
  assert.equal(await page.evaluate(() => designer.draft.columns[2].unsigned), true);
  await page.keyboard.press('Escape');
  // 索引字段浮层：加字段只滚动内容，浮层尺寸不变
  await tab(page, '索引');
  await page.getByRole('button', { name: '索引 1 字段选择', exact: true }).click();
  const before = await page.locator('.popover.index-cols').boundingBox();
  await page.getByRole('button', { name: '为索引 1添加字段', exact: true }).click();
  await page.getByRole('button', { name: '为索引 1添加字段', exact: true }).click();
  const after = await page.locator('.popover.index-cols').boundingBox();
  assert.equal(after.width, before.width); assert.equal(after.height, before.height);
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
  // 主键行与索引行都用「字段选择」浮层维护有序多字段：点开浮层 → 编辑 → 确定
  assert.deepEqual(await page.evaluate(() => designer.draft.primary_key), ['title', 'id']);
  await page.getByRole('button', { name: '主键 字段选择', exact: true }).click();
  assert.equal(await page.getByRole('combobox', { name: '主键 字段 1', exact: true }).inputValue(), 'title');
  assert.equal(await page.getByRole('combobox', { name: '主键 字段 2', exact: true }).inputValue(), 'id');
  await page.getByRole('button', { name: '取消', exact: true }).click();
  await page.getByRole('button', { name: '索引 1 字段选择', exact: true }).click();
  // 浮层尾部（右边缘）对齐「…」按钮的右边缘
  assert.equal(await page.evaluate(() => document.querySelector('.popover.index-cols')?.dataset.align), 'end');
  const edges = await page.evaluate(() => {
    const trigger = document.querySelector('.designer-picker-more.open');
    const popover = document.querySelector('.popover.index-cols');
    return { trigger: trigger.getBoundingClientRect().right, popover: popover.getBoundingClientRect().right };
  });
  assert.ok(Math.abs(edges.trigger - edges.popover) <= 1, `right edges must align: ${JSON.stringify(edges)}`);
  await page.getByRole('combobox', { name: '索引 1 字段 1', exact: true }).selectOption('id');
  await page.getByRole('button', { name: '为索引 1添加字段', exact: true }).click();
  await page.getByRole('combobox', { name: '索引 1 字段 2', exact: true }).selectOption('title');
  await page.getByRole('combobox', { name: '索引 1 字段 2 排序', exact: true }).selectOption('DESC');
  await page.getByRole('button', { name: '确定', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => designer.draft.indexes[0].columns), ['id', 'title']);
  assert.deepEqual(await page.evaluate(() => designer.draft.indexes[0].descending), ['title']);
  // 索引列表每行不再有删除按钮：点行选中，删除走顶部操作栏（这里直接调用同一条命令）
  assert.equal(await page.locator('.designer-grid.index-grid tbody button.designer-row-remove').count(), 0);
  assert.equal(await page.locator('.designer-grid.index-grid thead th').count(), 4);
  await page.getByRole('textbox', { name: '索引 1 名称', exact: true }).fill('idx_pair');
  await page.evaluate(() => designer.addIndex());
  assert.equal(await page.evaluate(() => designer.draft.indexes.length), 2);
  await page.getByRole('textbox', { name: '索引 2 名称', exact: true }).click();
  await page.evaluate(() => designer.deleteIndex());
  assert.deepEqual(await page.evaluate(() => designer.draft.indexes.map(index => index.name)), ['idx_pair']);
  await tab(page, '外键'); await page.evaluate(() => designer.addForeignKey());
  await page.getByRole('textbox', { name: '外键 1 名称', exact: true }).fill('fk_pair');
  // 外键页直接展示并编辑草稿中的引用配置，不再读取引用元数据
  assert.equal(await page.getByRole('button', { name: '读取引用元数据', exact: true }).count(), 0);
  await page.getByRole('textbox', { name: '外键 1 引用库', exact: true }).fill('other_schema');
  await page.getByRole('textbox', { name: '外键 1 引用表', exact: true }).fill('parents');
  await page.getByRole('combobox', { name: '外键 1 本表字段', exact: true }).fill('title, id');
  await page.getByRole('textbox', { name: '外键 1 引用字段', exact: true }).fill('parent_b, parent_a');
  await page.getByRole('combobox', { name: '外键 1 更新时', exact: true }).selectOption('CASCADE');
  await page.getByRole('combobox', { name: '外键 1 删除时', exact: true }).selectOption('RESTRICT');
  const refsBeforeSave = await page.evaluate(() => JSON.parse(JSON.stringify(fixture.referenceCalls)));
  await saving(page);
  assert.match(await page.getByLabel('后端实际 SQL 计划', { exact: true }).textContent(), /ALTER TABLE backend_actual_plan;/);
  await finishSave(page);
  const state = await page.evaluate(() => ({ sent: fixture.executions, refs: fixture.referenceCalls, dirty: designer.draftDirty(), notices: fixture.notices }));
  assert.equal(state.sent.length, 1); assert.deepEqual(state.sent[0].design.primary_key, ['title', 'id']);
  assert.deepEqual(state.sent[0].design.indexes[0].columns, ['id', 'title']);
  assert.deepEqual(state.sent[0].design.foreign_keys[0], { name: 'fk_pair', columns: ['title', 'id'], ref_schema: 'other_schema', ref_table: 'parents', ref_columns: ['parent_b', 'parent_a'], on_update: 'CASCADE', on_delete: 'RESTRICT' });
  assert.deepEqual(refsBeforeSave, []);
  assert.deepEqual(state.refs, []);
  assert.equal(state.sent[0].design.base_revision, 'r1'); assert.equal(state.sent[0].design.source_fingerprint, 'f1');
  assert.equal(state.sent[0].request.execution_grant, 'fixture-controlled-grant');
  assert.equal(state.sent[0].request.sql, JSON.stringify({ owner: 't', design: state.sent[0].design }));
  assert.equal(state.dirty, false); assert.ok(state.notices.some(n => n[0] === 'success'));
});

test('hidden check, trigger and options tabs stay off while the full comment is captured, saved and discarded as one draft', async t => {
  const page = await designerPage(browser); t.after(() => page.close());
  // 检查 / 触发器 / 选项三个页签暂时隐藏
  assert.deepEqual(await page.evaluate(() => [...document.querySelectorAll('.designer-tab')].map(el => el.textContent)), ['字段', '索引', '外键', '注释', 'SQL 预览']);
  await page.getByRole('tab', { name: '注释', exact: true }).click();
  const comment = '第一行\n完整注释，保留引号 \' 与 Unicode'; await page.getByRole('textbox', { name: '表注释', exact: true }).fill(comment);
  const capture = await page.evaluate(() => designer.captureDraft()); assert.equal(capture.payload.draft.comment, comment);
  await saving(page); await finishSave(page);
  const design = await page.evaluate(() => fixture.executions[0].design); assert.equal(design.comment, comment);
  assert.equal(Object.hasOwn(design, 'checks'), false); assert.equal(Object.hasOwn(design, 'triggers'), false); assert.equal(Object.hasOwn(design, 'options'), false);
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
  await tab(page, '索引'); await page.evaluate(() => designer.addIndex()); await page.evaluate(async () => designer.saveChanges());
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

test('keyboard tabs, modal focus containment and live highlighted SQL are accessible', async t => {
  const page = await designerPage(browser); t.after(() => page.close()); await page.getByRole('tab', { name: '字段', exact: true }).focus(); await page.keyboard.press('ArrowRight'); assert.equal(await page.getByRole('tab', { name: '索引', exact: true }).getAttribute('aria-selected'), 'true');
  // 先制造一处结构改动，SQL 预览才有内容
  await page.getByRole('tab', { name: '字段', exact: true }).click();
  await page.getByRole('textbox', { name: '字段 id 注释', exact: true }).fill('live sql');
  await page.getByRole('tab', { name: 'SQL 预览', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.designer-sql')?.textContent.includes('backend_actual_plan'));
  // 没有「生成」按钮：切到本页即自动生成，并做语法高亮
  assert.equal(await page.getByRole('button', { name: '生成后端实际计划', exact: true }).count(), 0);
  const text = await page.locator('.designer-sql').textContent(); assert.ok(text.includes('ALTER TABLE backend_actual_plan;')); assert.equal(text.includes('\\n'), false);
  assert.ok(await page.locator('.designer-sql .ddl-kw').count() > 0);
  // 改一处结构后自动刷新
  const before = await page.evaluate(() => fixture.previews.length);
  await page.getByRole('tab', { name: '注释', exact: true }).click(); await page.getByRole('textbox', { name: '表注释', exact: true }).fill('keyboard'); await saving(page);
  const confirm = page.getByRole('button', { name: '确认执行', exact: true }); await confirm.focus(); await page.keyboard.press('Tab'); assert.equal(await page.evaluate(() => document.activeElement?.closest('[role="dialog"]') !== null), true); await page.keyboard.press('Escape'); await page.waitForFunction(() => !designer.draftBusy()); assert.equal(await page.evaluate(() => fixture.executions.length), 0);
  assert.ok(await page.evaluate(() => fixture.previews.length) > before);
});

test('SQLite unsupported comment shows its concrete reason and only the intended tabs are exposed', async t => {
  const page = await designerPage(browser, 'sqlite'); t.after(() => page.close());
  assert.deepEqual(await page.evaluate(() => [...document.querySelectorAll('.designer-tab')].map(el => el.textContent)), ['字段', '索引', '外键', '注释', 'SQL 预览']);
  await tab(page, '注释'); assert.equal(await page.getByRole('textbox', { name: '表注释', exact: true }).isDisabled(), true); assert.match(await page.getByRole('tabpanel').textContent(), /SQLite 没有表注释 DDL/);
});

for (const kind of ['clear', 'copy_structure', 'copy_data']) test(`${kind} operation requires backend preview, keeps confirmation and refreshes only the receipt target`, async t => {
  const page = await designerPage(browser); t.after(() => page.close()); await page.evaluate(kind => DesignerModules.openTableOperationDialog({ connectionId: 'c', schema: 'db', table: 'items', kind, ownerId: 'operation-owner', writeReason: () => null, applied: async (receipt, schema) => fixture.refreshed.push({ table: receipt.target_table ?? receipt.source_table, schema }), toast: (kind, message) => fixture.notices.push([kind, message]) }), kind);
  const confirm = page.getByRole('button', { name: kind === 'clear' ? '确认清空全部数据' : '确认创建复制表', exact: true }); assert.equal(await confirm.isDisabled(), true);
  if (kind !== 'clear') { await page.getByRole('textbox', { name: '复制目标表名', exact: true }).fill('items'); await page.getByRole('button', { name: '预览后端计划', exact: true }).click(); assert.equal(await page.evaluate(() => fixture.previews.length), 0); await page.getByRole('textbox', { name: '复制目标表名', exact: true }).fill('new_target'); }
  else assert.match(await page.getByRole('dialog').textContent(), /清空全部数据/);
  await page.getByRole('button', { name: '预览后端计划', exact: true }).click(); await page.waitForFunction(() => fixture.previews.length === 1); assert.match(await page.getByLabel('后端实际 SQL 计划', { exact: true }).textContent(), /BACKEND_OPERATION_PLAN/); assert.equal(await page.evaluate(() => fixture.executions.length), 0);
  await confirm.click(); await page.waitForFunction(() => fixture.refreshed.length === 1); assert.equal(await confirm.isDisabled(), true); assert.deepEqual(await page.evaluate(() => fixture.refreshed), [{ schema: 'db', table: kind === 'clear' ? 'items' : 'new_target' }]); assert.equal(await page.evaluate(() => fixture.executions.length), 1); assert.equal(await page.evaluate(() => fixture.executions[0].request.execution_grant), 'fixture-controlled-grant');
});
