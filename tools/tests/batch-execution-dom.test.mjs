import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { root, startBrowserFixture } from './batch-execution-fixture.mjs';

let fixture;
const sessionRef = `batch-${randomUUID()}`, evidence = [], scenarios = [];
before(async () => { await mkdir(path.join(root, 'artifacts'), { recursive: true }); fixture = await startBrowserFixture(); });
after(async () => {
  try { await writeFile(path.join(root, 'artifacts/batch-session.json'), JSON.stringify({ sessionRef, mode: '真实产品模块 + mock 传输；非 native DB', nativeCommandCount: 0, evidence, scenarios }, null, 2)); }
  finally { await fixture?.close(); }
  console.log(JSON.stringify({ sessionRef, evidence, scenarios }));
});
async function withPage(run) {
  const page = await fixture.browser.newPage({ viewport: { width: 1280, height: 900 } }); page.setDefaultTimeout(7000);
  // 「关闭即转后台」会先弹确认框：E2E 一律接受，表示用户同意转入后台。
  page.on('dialog', dialog => { void dialog.accept(); });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  try {
    await page.goto(fixture.url, { waitUntil: 'domcontentloaded', timeout: 30_000 });
    await page.waitForLoadState('load', { timeout: 30_000 });
    await page.waitForFunction(() => {
      const mounted = window.batchFixture;
      return document.readyState === 'complete'
        && typeof mounted?.openFile === 'function'
        && typeof mounted?.openGenerator === 'function'
        && typeof mounted?.service?.list === 'function'
        && Array.isArray(mounted?.source?.calls);
    });
    await run(page); assert.deepEqual(errors, []);
    scenarios.push(await page.evaluate(() => {
      const { source, service, ipcCalls } = window.batchFixture;
      return { calls: Object.fromEntries(['register', 'run', 'status', 'cancel', 'ensure'].map(name => [name, source.calls.filter(call => call[0] === name).length])),
        ipc: Object.fromEntries([...new Set(ipcCalls.map(call => call[0]))].map(name => [name, ipcCalls.filter(call => call[0] === name).length])),
        tasks: service.list().map(record => ({ kind: record.kind, context: record.context, state: record.state, busy: record.busy, unknown: record.unknown })) };
    }));
  }
  finally { await page.close(); }
}
const fileDialog = page => page.getByRole('dialog', { name: '运行 SQL 文件', exact: true });
const generatorDialog = page => page.getByRole('dialog', { name: '数据生成', exact: true });
const screenshot = async (page, name) => { const file = `artifacts/batch-${name}.png`; await page.screenshot({ path: path.join(root, file), fullPage: true }); evidence.push(file); };
const waitContext = page => page.waitForFunction(() => window.batchFixture.service.list()[0]?.context);
const snapshot = (page, options) => page.evaluate(options => window.batchFixture.snapshot(options), options);
const recordLog = (sequence, extra = {}) => ({ sequence, phase: 'execute', level: 'error', statement: sequence, row: sequence, message: 'RAW_DB_ERROR secret-token',
  sql_template: 'INSERT INTO "<img src=x onerror=window.batchPwned=true>"(id) VALUES (?)', ...extra });
async function previewGenerator(page) {
  await page.evaluate(() => window.batchFixture.openGenerator()); const dialog = generatorDialog(page);
  await page.waitForFunction(() => document.querySelector('[aria-label="目标表"]')?.options.length === 1);
  await dialog.getByRole('button', { name: '下一步', exact: true }).click();
  await dialog.getByRole('button', { name: '下一步', exact: true }).click();
  await dialog.getByRole('button', { name: '生成预览', exact: true }).click(); return dialog;
}

test('真实 SQL 对话框：两 tab、只读固定目标/路径、五种编码、键盘切换与同步 singleflight', async () => withPage(async page => {
  await page.evaluate(() => { window.batchFixture.source.mode = 'delayed-register'; window.batchFixture.openFile(); });
  const dialog = fileDialog(page), general = dialog.getByRole('tab', { name: '常规', exact: true }), records = dialog.getByRole('tab', { name: '执行记录', exact: true });
  assert.equal(await general.getAttribute('aria-selected'), 'true'); assert.equal(await records.getAttribute('aria-selected'), 'false');
  assert.equal(await dialog.getByLabel('固定目标').getAttribute('readonly'), '');
  assert.equal(await dialog.getByLabel('源文件').inputValue(), 'D:/fixtures/SQL 文件.sql');
  assert.equal(await dialog.getByLabel('源文件').getAttribute('title'), 'D:/fixtures/SQL 文件.sql');
  assert.deepEqual(await dialog.getByLabel('文件编码').locator('option').evaluateAll(options => options.map(option => option.value)), ['utf-8', 'gbk', 'gb18030', 'big5', 'latin1']);
  await general.focus(); await general.press('ArrowRight'); assert.equal(await records.getAttribute('aria-selected'), 'true');
  await records.press('Home'); await dialog.getByLabel('文件编码').selectOption('gb18030');
  await dialog.getByRole('checkbox', { name: '遇到错误时停止' }).uncheck();
  await screenshot(page, 'sql-general');
  await dialog.getByRole('button', { name: '开始执行', exact: true }).evaluate(button => { button.click(); button.click(); });
  assert.equal(await records.getAttribute('aria-selected'), 'true'); assert.equal(await dialog.getByRole('button', { name: '开始执行', exact: true }).isDisabled(), true);
  const calls = await page.evaluate(() => window.batchFixture.source.calls);
  assert.equal(calls.filter(call => call[0] === 'register').length, 1); assert.equal(calls.filter(call => call[0] === 'run').length, 0);
  assert.equal(calls[0][1].task.encoding, 'gb18030'); assert.equal(calls[0][1].task.stop_on_error, false);
  const registerIpc = await page.evaluate(() => window.batchFixture.ipcCalls.find(call => call[0] === 'register_batch_execution'));
  assert.deepEqual(Object.keys(registerIpc[1]), ['request']); assert.equal(registerIpc[1].request.task.kind, 'sql_file');
  await dialog.getByRole('button', { name: '停止', exact: true }).click();
  await page.evaluate(() => window.batchFixture.source.releaseRegistration()); await waitContext(page);
  await page.waitForFunction(() => window.batchFixture.source.calls.some(call => call[0] === 'cancel'));
  assert.equal(await page.evaluate(() => window.batchFixture.source.calls.filter(call => call[0] === 'run').length), 0);
}));

test('SQL 文件总超时默认不限时，明确发送 0，执行与排空期间锁定', async () => withPage(async page => {
  await page.evaluate(() => { window.batchFixture.source.mode = 'delayed-register'; window.batchFixture.openFile(); });
  const dialog = fileDialog(page), timeout = dialog.getByLabel('文件总超时（秒）', { exact: true });
  assert.equal(await timeout.inputValue(), '0');
  assert.match(await dialog.innerText(), /整个 SQL 文件.*0 表示不限时/);
  await dialog.getByRole('button', { name: '开始执行', exact: true }).click();
  assert.equal(await timeout.isDisabled(), true);
  assert.equal(await page.evaluate(() => window.batchFixture.source.calls.find(call => call[0] === 'register')[1].timeout_ms), 0);
  assert.equal(await page.evaluate(() => window.batchFixture.ipcCalls.find(call => call[0] === 'register_batch_execution')[1].request.timeout_ms), 0);
  await page.evaluate(() => window.batchFixture.source.releaseRegistration()); await waitContext(page);
  await snapshot(page, { state: 'succeeded', pending: true });
  await page.waitForFunction(() => window.batchFixture.service.list()[0].state === 'succeeded');
  assert.equal(await timeout.isDisabled(), true);
  await snapshot(page, { state: 'succeeded', pending: false, elapsed: 4000 });
  await page.waitForFunction(() => !window.batchFixture.service.list()[0].busy);
  assert.equal(await timeout.isEnabled(), true);
}));

test('SQL 文件总超时拦截无效整数，秒转换为毫秒后提交并保留配置', async () => withPage(async page => {
  await page.evaluate(() => window.batchFixture.openFile());
  const dialog = fileDialog(page), timeout = dialog.getByLabel('文件总超时（秒）', { exact: true });
  for (const value of ['', '-1', '1.5', '9007199254741']) {
    await timeout.fill(value);
    await dialog.getByRole('button', { name: '开始执行', exact: true }).click();
    assert.equal(await timeout.getAttribute('aria-invalid'), 'true');
    assert.equal(await timeout.evaluate(input => input === document.activeElement), true);
    assert.match(await dialog.innerText(), /必须是非负整数秒/);
    assert.equal(await page.evaluate(() => window.batchFixture.source.calls.filter(call => call[0] === 'register').length), 0);
  }
  await timeout.fill('75');
  assert.equal(await timeout.getAttribute('aria-invalid'), null);
  await dialog.getByRole('button', { name: '开始执行', exact: true }).click(); await waitContext(page);
  assert.equal(await page.evaluate(() => window.batchFixture.source.calls.find(call => call[0] === 'register')[1].timeout_ms), 75000);
  assert.equal(await page.evaluate(() => window.batchFixture.ipcCalls.find(call => call[0] === 'register_batch_execution')[1].request.timeout_ms), 75000);
  assert.equal(await timeout.inputValue(), '75');
  assert.equal(await timeout.isDisabled(), true);
}));

test('只读 SQL 入口门禁不登记任务', async () => withPage(async page => {
  await page.evaluate(() => window.batchFixture.openFile(true)); const dialog = fileDialog(page);
  assert.equal(await dialog.getByRole('button', { name: '开始执行', exact: true }).isDisabled(), true);
  assert.match(await dialog.innerText(), /只读/); assert.deepEqual(await page.evaluate(() => window.batchFixture.source.calls), []);
}));

test('SQL 指标与错误原文可见，模板作为文本渲染', async () => withPage(async page => {
  await page.evaluate(() => window.batchFixture.openFile()); const dialog = fileDialog(page);
  await dialog.getByRole('button', { name: '开始执行', exact: true }).click(); await waitContext(page);
  await snapshot(page, { logs: [recordLog(8)], dropped: 7, elapsed: 2345, parseError: 'RAW_DB_ERROR secret-token' });
  await page.waitForFunction(() => document.querySelector('.batch-execution-logs')?.textContent.includes('语句 #8'));
  assert.deepEqual(await dialog.locator('.batch-execution-metrics dt').allTextContents(), ['扫描', '执行', '错误', '耗时']);
  assert.deepEqual(await dialog.locator('.batch-execution-metrics dd').allTextContents(), ['7', '4', '1', '2.3 s']);
  const logs = dialog.locator('.batch-execution-logs'); assert.match(await logs.innerText(), /INSERT INTO/); assert.equal(await logs.locator('img').count(), 0);
  assert.equal(await page.evaluate(() => window.batchPwned), undefined); assert.match(await dialog.innerText(), /RAW_DB_ERROR secret-token/);
  assert.match(await dialog.innerText(), /8192 字节/); assert.match(await dialog.innerText(), /序号 7 及之前/);
  assert.match(await dialog.innerText(), /扫描进行中/); assert.ok(await logs.evaluate(el => el.getBoundingClientRect().height) >= 200, '记录区必须占满剩余高度');
  await screenshot(page, 'sql-records');
}));

test('关闭转后台后工具入口回看同一任务（没有浮动按钮），真实停止，ack 与 pending 均保持忙碌', async () => withPage(async page => {
  await page.evaluate(() => window.batchFixture.openFile()); let dialog = fileDialog(page);
  await dialog.getByRole('button', { name: '开始执行', exact: true }).click(); await waitContext(page);
  const original = await page.evaluate(() => window.batchFixture.service.list()[0].context);
  await dialog.getByRole('button', { name: '转入后台', exact: true }).click(); assert.equal(await fileDialog(page).count(), 0);
  assert.equal(await page.locator('.batch-task-entry').count(), 0);
  await page.evaluate(() => window.batchFixture.openRecords()); dialog = page.getByRole('dialog', { name: '批执行记录', exact: true });
  assert.match(await dialog.getByLabel('回看批执行任务').innerText(), new RegExp(original.execution_id.slice(0, 8)));
  await dialog.getByRole('button', { name: '停止', exact: true }).click();
  await page.waitForFunction(() => window.batchFixture.source.calls.some(call => call[0] === 'cancel'));
  let state = await page.evaluate(() => ({ record: window.batchFixture.service.list()[0], calls: window.batchFixture.source.calls }));
  assert.equal(state.record.busy, true); assert.deepEqual(state.calls.find(call => call[0] === 'cancel')[1], original);
  const cancelIpc = await page.evaluate(() => window.batchFixture.ipcCalls.find(call => call[0] === 'cancel_execution'));
  assert.deepEqual(cancelIpc[1], { executionId: original.execution_id, connectionId: original.connection_id, ownerId: original.owner_id, namespace: original.namespace });
  await snapshot(page, { state: 'cancelled', pending: true, transaction: 'rolled_back' });
  await page.waitForFunction(() => window.batchFixture.service.list()[0].state === 'cancelled');
  assert.equal(await page.evaluate(() => window.batchFixture.service.list()[0].busy), true);
  await screenshot(page, 'background-stop-pending');
  await snapshot(page, { state: 'cancelled', pending: false, transaction: 'rolled_back', elapsed: 4000 });
  await page.waitForFunction(() => !window.batchFixture.service.list()[0].busy);
  state = await page.evaluate(() => ({ record: window.batchFixture.service.list()[0], calls: window.batchFixture.source.calls }));
  assert.equal(state.calls.filter(call => call[0] === 'register').length, 1); assert.equal(state.calls.filter(call => call[0] === 'run').length, 1);
  assert.deepEqual(state.record.context, original);
}));

test('unknown 在资源排空后仍禁止重放，回看只查询原 context', async () => withPage(async page => {
  await page.evaluate(() => { window.batchFixture.source.mode = 'run-reject'; window.batchFixture.openFile(); }); const dialog = fileDialog(page);
  await dialog.getByRole('button', { name: '开始执行', exact: true }).click(); await waitContext(page);
  await snapshot(page, { state: 'unknown', pending: true, transaction: 'unknown' });
  await page.waitForFunction(() => window.batchFixture.service.list()[0].snapshot?.status.state === 'unknown');
  await snapshot(page, { state: 'unknown', pending: false, transaction: 'unknown', elapsed: 4000 });
  await page.waitForFunction(() => !window.batchFixture.service.list()[0].busy);
  assert.equal(await dialog.getByRole('button', { name: '开始执行', exact: true }).isDisabled(), true);
  assert.equal(await dialog.getByRole('button', { name: '回到配置', exact: true }).isDisabled(), true);
  await dialog.getByRole('button', { name: '开始执行', exact: true }).evaluate(button => { button.click(); button.dispatchEvent(new Event('click')); });
  assert.equal(await page.evaluate(() => window.batchFixture.source.calls.filter(call => call[0] === 'run').length), 1);
  assert.equal(await page.evaluate(() => window.batchFixture.source.calls.filter(call => call[0] === 'register').length), 1);
  await screenshot(page, 'unknown-no-replay');
}));

test('生成器唯一 footer 写入：未确认禁用；更改配置清确认且要求新预览', async () => withPage(async page => {
  const dialog = await previewGenerator(page), finish = dialog.locator('.dialog-footer').getByRole('button', { name: '执行写入', exact: true, includeHidden: true });
  assert.equal(await dialog.getByRole('button', { name: '执行写入', exact: true, includeHidden: true }).count(), 1); assert.equal(await finish.isDisabled(), true);
  const confirm = dialog.getByRole('checkbox', { name: /我确认将预览/ }); await confirm.check(); assert.equal(await finish.isEnabled(), true);
  await dialog.getByRole('button', { name: '上一步', exact: true }).click();
  await dialog.getByLabel('id 生成方式').selectOption('sequence');
  await dialog.getByRole('button', { name: '下一步', exact: true }).click(); assert.equal(await confirm.isChecked(), false); assert.equal(await finish.isDisabled(), true);
  await dialog.getByRole('button', { name: '生成预览', exact: true }).click(); await confirm.check();
  await screenshot(page, 'generator-confirmation');
  await dialog.getByRole('button', { name: '上一步', exact: true }).click(); await dialog.getByRole('button', { name: '上一步', exact: true }).click();
  await dialog.getByLabel('生成条数').fill('500'); await dialog.getByRole('button', { name: '下一步', exact: true }).click(); await dialog.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await confirm.isChecked(), false); assert.equal(await finish.isDisabled(), true);
  await dialog.getByRole('button', { name: '生成预览', exact: true }).click(); await confirm.check();
  await finish.evaluate(button => { button.click(); button.click(); }); await waitContext(page);
  assert.equal(await dialog.locator('.batch-generator-record').isVisible(), true);
  const calls = await page.evaluate(() => window.batchFixture.source.calls), registrations = calls.filter(call => call[0] === 'register');
  assert.equal(registrations.length, 1); assert.equal(calls.filter(call => call[0] === 'run').length, 1);
  assert.equal(registrations[0][1].task.request.rows.length, 500); assert.equal(registrations[0][1].task.kind, 'generated_rows');
  assert.equal(await finish.isDisabled(), true); assert.deepEqual(await page.evaluate(() => window.batchFixture.done), []);
  await snapshot(page, { state: 'succeeded', pending: true, transaction: 'committed' });
  await page.waitForFunction(() => window.batchFixture.service.list()[0].state === 'succeeded');
  assert.deepEqual(await page.evaluate(() => window.batchFixture.done), []); assert.equal(await finish.isDisabled(), true);
  await snapshot(page, { state: 'succeeded', pending: false, transaction: 'committed', affected: 500, elapsed: 4000 });
  await page.waitForFunction(() => window.batchFixture.done.length === 1);
  assert.deepEqual(await page.evaluate(() => window.batchFixture.done), [[500, 'items']]);
  await dialog.getByRole('button', { name: '回到配置', exact: true }).click(); assert.equal(await finish.isDisabled(), true); assert.equal(await confirm.isChecked(), false);
}));

test('生成器日志复用模板记录，关闭不 dispose，后台停止回滚不触发 onDone', async () => withPage(async page => {
  const dialog = await previewGenerator(page);
  await dialog.getByRole('checkbox', { name: /我确认将预览/ }).check();
  await dialog.locator('.dialog-footer').getByRole('button', { name: '执行写入', exact: true, includeHidden: true }).click(); await waitContext(page);
  await snapshot(page, { logs: [recordLog(1), recordLog(2, { phase: 'rollback', sql_template: null })] });
  await page.waitForFunction(() => document.querySelector('.batch-generator-record .batch-execution-logs')?.textContent.includes('INSERT INTO'));
  assert.match(await dialog.innerText(), /RAW_DB_ERROR secret-token/); assert.equal(await dialog.locator('.batch-execution-logs img').count(), 0);
  await screenshot(page, 'generator-records'); await dialog.getByRole('button', { name: '转入后台', exact: true }).click();
  assert.equal(await page.locator('.batch-task-entry').count(), 0);
  await page.evaluate(() => window.batchFixture.openRecords()); const records = page.getByRole('dialog', { name: '批执行记录', exact: true });
  await records.getByRole('button', { name: '停止', exact: true }).click();
  await snapshot(page, { state: 'cancelled', pending: false, transaction: 'rolled_back', elapsed: 4000 });
  await page.waitForFunction(() => !window.batchFixture.service.list()[0].busy);
  assert.deepEqual(await page.evaluate(() => window.batchFixture.done), []);
  assert.equal(await page.evaluate(() => window.batchFixture.source.calls.filter(call => call[0] === 'run').length), 1);
}));

test('生成器 unknown 排空仍锁定配置，finally 不重启任务或触发 onDone', async () => withPage(async page => {
  const dialog = await previewGenerator(page);
  await dialog.getByRole('checkbox', { name: /我确认将预览/ }).check();
  await dialog.locator('.dialog-footer').getByRole('button', { name: '执行写入', exact: true, includeHidden: true }).click(); await waitContext(page);
  await snapshot(page, { state: 'unknown', pending: false, transaction: 'unknown' });
  await page.waitForFunction(() => window.batchFixture.service.list()[0].unknown && !window.batchFixture.service.list()[0].busy);
  assert.equal(await dialog.getByRole('button', { name: '回到配置', exact: true }).isDisabled(), true);
  assert.equal(await dialog.getByLabel('生成条数').isDisabled(), true);
  assert.equal(await dialog.locator('.dialog-footer').getByRole('button', { name: '执行写入', exact: true, includeHidden: true }).isDisabled(), true);
  assert.deepEqual(await page.evaluate(() => window.batchFixture.done), []);
  assert.equal(await page.evaluate(() => window.batchFixture.source.calls.filter(call => call[0] === 'run').length), 1);
}));

test('生成器关闭后仍接收真实 committed 回执，且回调只触发一次', async () => withPage(async page => {
  const dialog = await previewGenerator(page);
  await dialog.getByRole('checkbox', { name: /我确认将预览/ }).check();
  await dialog.locator('.dialog-footer').getByRole('button', { name: '执行写入', exact: true, includeHidden: true }).click(); await waitContext(page);
  await dialog.getByRole('button', { name: '转入后台', exact: true }).click();
  await snapshot(page, { state: 'succeeded', pending: false, transaction: 'committed', affected: 10 });
  await page.waitForFunction(() => window.batchFixture.done.length === 1);
  assert.equal(await page.locator('.batch-task-entry').count(), 0);
  await page.evaluate(() => window.batchFixture.openRecords());
  assert.deepEqual(await page.evaluate(() => window.batchFixture.done), [[10, 'items']]);
  assert.equal(await page.evaluate(() => window.batchFixture.source.calls.filter(call => call[0] === 'run').length), 1);
}));

test('只读生成器在真实 Wizard 内阻止预览和登记', async () => withPage(async page => {
  await page.evaluate(() => window.batchFixture.openGenerator(true)); const dialog = generatorDialog(page);
  assert.match(await dialog.innerText(), /只读/); await dialog.getByRole('button', { name: '下一步', exact: true }).click();
  assert.equal(await dialog.getByRole('button', { name: '下一步', exact: true }).count(), 1);
  assert.deepEqual(await page.evaluate(() => window.batchFixture.source.calls), []);
}));

test('真实 Wizard 的 finally 重新求业务门禁，未确认/外部 busy/重复调用均不放行', async () => withPage(async page => {
  const result = await page.evaluate(async () => {
    const { Wizard } = window.batchFixture; let eligible = false, calls = 0, finish;
    const wizard = new Wizard({ steps: [{ label: '确认', render() {} }], finishEligible: () => eligible,
      onFinish: () => { calls++; return new Promise(resolve => { finish = resolve; }); } });
    document.body.append(wizard.stepsEl, wizard.bodyEl, ...wizard.buttons());
    const initial = wizard.nextBtn.disabled; await wizard.next(); eligible = true; wizard.refreshControls();
    const pending = wizard.next(); await wizard.next(); eligible = false; finish(); await pending;
    const afterFinally = wizard.nextBtn.disabled; eligible = true;
    const second = wizard.next(); wizard.setBusy(true); finish(); await second; await wizard.next();
    const busy = wizard.nextBtn.disabled, ariaBusy = wizard.bodyEl.getAttribute('aria-busy'); wizard.setBusy(false); eligible = false; wizard.refreshControls();
    return { initial, calls, afterFinally, busy, ariaBusy, afterUnlock: wizard.nextBtn.disabled };
  });
  assert.deepEqual(result, { initial: true, calls: 2, afterFinally: true, busy: true, ariaBusy: 'true', afterUnlock: true });
}));
