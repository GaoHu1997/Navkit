// Explicit browser evidence: real production UI/controller + isolated mock IPC.
// Not desktop/native/database acceptance. No skip, force-exit, or business fallback.
import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { root, startObjectMutationBrowserFixture, objectMutationPage } from './object-mutation-browser-fixture.mjs';

let fixture, nextCase = 0;
const directory = path.join(root, 'artifacts', `object-mutation-browser-${randomUUID()}`);
const scenarios = [];
before(async () => { await mkdir(directory, { recursive: true }); fixture = await startObjectMutationBrowserFixture(); }, { timeout: 45000 });
after(async () => {
  try {
    assert.deepEqual(fixture?.browser.contexts().map(context => context.pages().length) ?? [], [], 'Every owned page/context must be closed');
  } finally {
    try { await fixture?.close(); }
    finally {
      const report = { mode: 'real Chromium + actual product dialog/session/controller + mock IPC only; NOT desktop/native/database evidence', nativeCommandCount: 0, bundledInputs: fixture?.inputs ?? [], scenarios };
      await writeFile(path.join(directory, 'report.json'), JSON.stringify(report, null, 2));
      console.log(JSON.stringify({ directory, ...report }));
    }
  }
}, { timeout: 45000 });

const dialog = page => page.getByRole('dialog', { name: '受控删除触发器', exact: true });
const previewButton = page => dialog(page).getByRole('button', { name: '读取删除预览', exact: true });
const submitButton = page => dialog(page).getByRole('button', { name: '确认删除', exact: true });
const stopButton = page => dialog(page).getByRole('button', { name: '停止', exact: true });
const closeButton = page => dialog(page).getByRole('button', { name: '关闭', exact: true });
const confirmation = page => dialog(page).getByRole('checkbox', { name: /我已核对完整对象与父表/ });
const status = page => dialog(page).getByRole('status');
const calls = (page, name) => page.evaluate(name => window.objectMutationFixture.calls.filter(call => call.name === name), name);
const waitStatus = (page, text) => page.waitForFunction(text => [...document.querySelectorAll('[role="status"]')].some(element => element.textContent.includes(text)), text);
const waitCall = (page, name, count = 1) => page.waitForFunction(({ name, count }) => window.objectMutationFixture.calls.filter(call => call.name === name).length >= count, { name, count });
const waitClosable = page => page.waitForFunction(() => [...document.querySelectorAll('[role="dialog"] button')].some(button => button.textContent === '关闭' && !button.disabled));
const release = (page, name) => page.evaluate(name => {
  const f = window.objectMutationFixture, callback = f[`release${name}`];
  if (typeof callback !== 'function') throw new Error(`Fixture ${name} has not reached its held IPC boundary`);
  callback();
}, name);
async function loaded(page) {
  await page.getByRole('button', { name: '打开受控对象删除', exact: true }).click();
  await waitStatus(page, '基线已读取');
}
async function prepared(page) {
  await loaded(page); await previewButton(page).click(); await waitStatus(page, '预览已固定');
}
async function closeAndRestore(page) {
  await closeButton(page).click();
  assert.equal(await dialog(page).count(), 0);
  assert.equal(await page.getByRole('button', { name: '打开受控对象删除', exact: true }).evaluate(button => button === document.activeElement), true);
}
async function guardedSubmit(page) {
  await submitButton(page).evaluate(button => { button.click(); button.dispatchEvent(new MouseEvent('click', { bubbles: true })); });
}
async function withPage(t, options, run) {
  const caseDirectory = path.join(directory, String(++nextCase).padStart(2, '0'));
  await mkdir(caseDirectory);
  let page;
  try {
    const mounted = await objectMutationPage(fixture, options); page = mounted.page;
    await run(page, caseDirectory);
    const observed = await page.evaluate(() => {
      const f = window.objectMutationFixture;
      return { calls: f.calls, forbidden: f.forbidden, nativeCalls: f.nativeCalls, committed: f.committed, visibleDialogs: document.querySelectorAll('[role="dialog"]').length };
    });
    assert.deepEqual(observed.forbidden, [], 'No execute_sql, execute_sql_v2 or legacy SQL adapter may be used');
    assert.deepEqual(observed.nativeCalls, [], 'This test must not invoke native IPC');
    assert.deepEqual(mounted.errors, [], 'Real browser execution must have no uncaught page errors');
    scenarios.push({ test: t.name, passed: true, nativeCommandCount: 0, ...observed });
    await writeFile(path.join(caseDirectory, 'scenario.json'), JSON.stringify(scenarios.at(-1), null, 2));
  } catch (error) {
    scenarios.push({ test: t.name, passed: false, error: { message: error.message, stack: error.stack } });
    if (page && !page.isClosed()) {
      await Promise.allSettled([
        page.screenshot({ path: path.join(caseDirectory, 'failure.png'), fullPage: true, timeout: 2000 }),
        page.content().then(html => writeFile(path.join(caseDirectory, 'failure.html'), html)),
      ]);
    }
    throw error;
  } finally { await page?.close(); }
}
const options = { timeout: 30000 };

test('打开真实对话框只加载精确父表基线，加载期间不能预览或删除', options, async t => withPage(t, { holdBaseline: true }, async page => {
  await page.getByRole('button', { name: '打开受控对象删除', exact: true }).click(); await waitCall(page, 'baseline');
  assert.equal(await dialog(page).getAttribute('aria-modal'), 'true');
  assert.equal(await dialog(page).getAttribute('aria-busy'), 'true');
  assert.equal(await previewButton(page).isDisabled(), true); assert.equal(await submitButton(page).isDisabled(), true);
  assert.equal(await confirmation(page).isDisabled(), true); assert.match(await status(page).innerText(), /正在读取.*尚未发送删除 SQL/);
  const [read] = await calls(page, 'baseline');
  assert.deepEqual(read.args, await page.evaluate(() => [window.objectMutationFixture.target, window.objectMutationFixture.owner]));
  assert.equal((await calls(page, 'preview')).length, 0); assert.equal((await calls(page, 'grant')).length, 0); assert.equal((await calls(page, 'typed_execute')).length, 0);
  await release(page, 'Baseline'); await waitStatus(page, '基线已读取');
  assert.equal(await dialog(page).getAttribute('aria-busy'), 'false'); assert.equal(await previewButton(page).isEnabled(), true);
  assert.equal(await submitButton(page).isDisabled(), true); assert.equal((await calls(page, 'baseline')).length, 1);
  assert.match(await dialog(page).innerText(), /main\.audit"中文<img> — main\.items"中文/); assert.equal(await dialog(page).locator('img').count(), 0);
  await closeAndRestore(page);
}));

test('基线读取失败显示真实错误且不能伪造预览、授权或删除', options, async t => withPage(t, { baselineError: true }, async page => {
  await page.getByRole('button', { name: '打开受控对象删除', exact: true }).click(); await waitStatus(page, '基线读取失败');
  assert.match(await status(page).innerText(), /fixture baseline unavailable/);
  assert.equal(await previewButton(page).isDisabled(), true); assert.equal(await submitButton(page).isDisabled(), true);
  assert.deepEqual((await page.evaluate(() => window.objectMutationFixture.calls)).map(call => call.name), ['baseline']);
  await closeAndRestore(page);
}));

test('固定预览与打开时证据绑定，checkbox确认前零提交，确认后仅typed对象IPC一次', options, async t => withPage(t, {}, async (page, caseDirectory) => {
  await prepared(page);
  const expected = await page.evaluate(() => { const f = window.objectMutationFixture; return { baseline: f.openingBaseline, plan: f.plans[0], owner: f.owner }; });
  const details = await dialog(page).locator('pre').textContent();
  for (const text of [expected.baseline.base_revision, expected.plan.plan_id, expected.plan.review_digest, 'main.audit"中文<img>', 'main.items"中文']) assert.ok(details.includes(text), text);
  assert.equal(await confirmation(page).isChecked(), false); assert.equal(await submitButton(page).isDisabled(), true);
  await guardedSubmit(page); await waitStatus(page, '必须确认固定预览');
  assert.equal((await calls(page, 'grant')).length, 0); assert.equal((await calls(page, 'typed_execute')).length, 0);
  await confirmation(page).check(); assert.equal(await submitButton(page).isEnabled(), true);
  await page.screenshot({ path: path.join(caseDirectory, 'fixed-preview-confirmed.png'), fullPage: true });
  await submitButton(page).click(); await page.waitForFunction(() => document.querySelectorAll('[role="dialog"]').length === 0);
  const [typed] = await calls(page, 'typed_execute'), [grant] = await calls(page, 'grant');
  assert.equal((await calls(page, 'typed_execute')).length, 1); assert.equal((await calls(page, 'baseline')).length, 1);
  const [request, work] = typed.args;
  assert.equal(request.sql, expected.plan.request_sql); assert.match(request.sql, /^OBJECT_MUTATION_DROP /); assert.doesNotMatch(request.sql, /DROP\s+(?:TRIGGER|TABLE)/);
  assert.equal(request.connection_id, expected.baseline.object.connection_id); assert.equal(request.namespace, 'main'); assert.equal(request.owner_id, expected.owner); assert.equal(request.origin, 'object_drop'); assert.equal(request.transaction_owner, null);
  assert.equal(request.execution_grant, await page.evaluate(() => window.objectMutationFixture.signed.execution_grant));
  assert.equal(grant.args[0].sql, expected.plan.request_sql);
  assert.deepEqual(work, { plan_id: expected.plan.plan_id, review_digest: expected.plan.review_digest, confirm_drop: true, draft: { baseline_id: expected.baseline.baseline_id, object: expected.baseline.object, source_fingerprint: expected.baseline.source_fingerprint, base_revision: expected.baseline.base_revision, operation: 'drop' } });
  assert.equal(await page.evaluate(() => window.objectMutationFixture.committed), 1);
  assert.equal(await page.locator('#open-object-mutation').evaluate(button => button === document.activeElement), true);
}));

test('伪造原生DROP文本的预览不能替代后端固定计划，错误后零授权/删除', options, async t => withPage(t, { previewMismatch: true }, async page => {
  await loaded(page); await previewButton(page).click(); await waitStatus(page, '预览失败');
  assert.match(await status(page).innerText(), /预览与打开时的基线不匹配/);
  assert.equal(await submitButton(page).isDisabled(), true); assert.equal(await confirmation(page).isDisabled(), true);
  assert.equal((await calls(page, 'grant')).length, 0); assert.equal((await calls(page, 'typed_execute')).length, 0); await closeAndRestore(page);
}));

test('延迟预览立即发布真实busy并冻结旧确认/预览/关闭，完成后必须新确认', options, async t => withPage(t, {}, async page => {
  await prepared(page); await confirmation(page).check();
  await page.evaluate(() => { window.objectMutationFixture.holdPreview = true; });
  await previewButton(page).click(); await waitCall(page, 'preview', 2);
  assert.equal(await dialog(page).getAttribute('aria-busy'), 'true'); assert.equal(await confirmation(page).isChecked(), false);
  assert.equal(await confirmation(page).isDisabled(), true); assert.equal(await previewButton(page).isDisabled(), true);
  assert.equal(await submitButton(page).isDisabled(), true); assert.equal(await closeButton(page).isDisabled(), true);
  await page.keyboard.press('Escape'); assert.equal(await dialog(page).count(), 1);
  await release(page, 'Preview'); await waitClosable(page);
  assert.equal(await dialog(page).getAttribute('aria-busy'), 'false'); assert.equal(await confirmation(page).isChecked(), false);
  assert.equal(await submitButton(page).isDisabled(), true); assert.equal((await calls(page, 'typed_execute')).length, 0); await closeAndRestore(page);
}));

test('重新读取预览失败必须失效旧checkbox与旧计划，不能误提交先前授权内容', options, async t => withPage(t, {}, async page => {
  await prepared(page); await confirmation(page).check();
  await page.evaluate(() => { window.objectMutationFixture.previewError = true; });
  await previewButton(page).click(); await waitStatus(page, '预览失败');
  assert.equal(await confirmation(page).isChecked(), false); assert.equal(await confirmation(page).isDisabled(), true); assert.equal(await submitButton(page).isDisabled(), true);
  await guardedSubmit(page); await waitStatus(page, '必须确认固定预览');
  assert.equal((await calls(page, 'grant')).length, 0); assert.equal((await calls(page, 'typed_execute')).length, 0); assert.equal((await calls(page, 'baseline')).length, 1);
  await page.evaluate(() => { window.objectMutationFixture.previewError = false; });
  await previewButton(page).click(); await waitStatus(page, '预览已固定');
  assert.equal((await calls(page, 'preview')).length, 3); assert.equal(await confirmation(page).isChecked(), false); assert.equal(await submitButton(page).isDisabled(), true);
  await closeAndRestore(page);
}));

test('停止during grant只附加cancel_before_send给同一签发typed工作，不发伪造cancel或SQL', options, async t => withPage(t, { holdGrant: true }, async page => {
  await prepared(page); const evidence = await dialog(page).locator('pre').textContent();
  await confirmation(page).check(); await submitButton(page).click(); await waitCall(page, 'grant');
  assert.equal(await dialog(page).getAttribute('aria-busy'), 'true'); assert.equal(await closeButton(page).isDisabled(), true); assert.equal(await stopButton(page).isEnabled(), true);
  await stopButton(page).click(); await waitStatus(page, '停止请求已发送');
  assert.equal((await calls(page, 'cancel')).length, 0); assert.equal((await calls(page, 'typed_execute')).length, 0);
  await release(page, 'Grant'); await waitStatus(page, 'fixture stopped before SQL'); await waitClosable(page);
  const [typed] = await calls(page, 'typed_execute');
  assert.equal(typed.args[0].cancel_before_send, true); assert.equal(typed.args[0].execution_grant, await page.evaluate(() => window.objectMutationFixture.signed.execution_grant));
  assert.equal(typed.args[1].plan_id, await page.evaluate(() => window.objectMutationFixture.plans[0].plan_id));
  assert.equal((await calls(page, 'typed_execute')).length, 1); assert.equal((await calls(page, 'cancel')).length, 0); assert.equal(await page.evaluate(() => window.objectMutationFixture.committed), 0);
  assert.equal(await dialog(page).locator('pre').textContent(), evidence); assert.equal(await submitButton(page).isDisabled(), true); await closeAndRestore(page);
}));

for (const executeMode of ['unknown', 'lost']) {
  test(`${executeMode}: Unknown保持原对象证据，资源排空前不能关闭，排空后仍禁止重放`, options, async t => withPage(t, { executeMode, resourcesPending: true, statusPending: true }, async page => {
    await prepared(page); const evidence = await dialog(page).locator('pre').textContent();
    await confirmation(page).check(); await submitButton(page).click(); await waitStatus(page, '禁止重放'); await waitCall(page, 'status');
    assert.equal(await previewButton(page).isDisabled(), true); assert.equal(await submitButton(page).isDisabled(), true); assert.equal(await confirmation(page).isDisabled(), true);
    assert.equal(await closeButton(page).isDisabled(), true); assert.equal(await dialog(page).getAttribute('aria-busy'), 'true');
    await page.keyboard.press('Escape'); await page.evaluate(() => window.objectMutationFixture.dialog.close()); assert.equal(await dialog(page).count(), 1);
    await page.evaluate(() => { window.objectMutationFixture.statusPending = false; }); await waitClosable(page);
    assert.equal(await dialog(page).getAttribute('aria-busy'), 'false'); assert.equal(await dialog(page).locator('pre').textContent(), evidence);
    assert.equal(await previewButton(page).isDisabled(), true); assert.equal(await submitButton(page).isDisabled(), true); assert.match(await status(page).innerText(), /禁止重放/);
    await guardedSubmit(page); await waitStatus(page, '已执行或未知计划禁止重放');
    assert.equal((await calls(page, 'grant')).length, 1); assert.equal((await calls(page, 'typed_execute')).length, 1); assert.equal(await page.evaluate(() => window.objectMutationFixture.committed), 0);
    const [typed] = await calls(page, 'typed_execute'), statuses = await calls(page, 'status');
    for (const read of statuses) assert.deepEqual(read.args[0], typed.args[0], 'Status observes only the original signed execution context');
    await closeAndRestore(page);
  }));
}

test('页面归属改变后晚到grant不发送typed IPC，不变成永久Unknown，保留证据且可关闭', options, async t => withPage(t, { holdGrant: true }, async page => {
  await prepared(page); const evidence = await dialog(page).locator('pre').textContent();
  await confirmation(page).check(); await submitButton(page).click(); await waitCall(page, 'grant');
  await page.evaluate(() => { window.objectMutationFixture.contextAlive = false; }); await release(page, 'Grant');
  await waitStatus(page, '页面归属已变化'); await waitClosable(page);
  assert.equal(await dialog(page).getAttribute('aria-busy'), 'false'); assert.equal(await dialog(page).locator('pre').textContent(), evidence);
  assert.doesNotMatch(await status(page).innerText(), /原对象证据已保留，禁止重放/);
  assert.equal((await calls(page, 'grant')).length, 1); assert.equal((await calls(page, 'typed_execute')).length, 0); assert.equal((await calls(page, 'cancel')).length, 0); assert.equal((await calls(page, 'status')).length, 0);
  assert.equal(await page.evaluate(() => window.objectMutationFixture.committed), 0); await closeAndRestore(page);
}));

test('真实键盘Tab/Shift-Tab被限制在modal，Escape关闭并恢复触发按钮焦点', options, async t => withPage(t, {}, async page => {
  await loaded(page);
  assert.equal(await previewButton(page).evaluate(button => button === document.activeElement), true);
  await previewButton(page).press('Shift+Tab'); assert.equal(await closeButton(page).evaluate(button => button === document.activeElement), true);
  await closeButton(page).press('Tab'); assert.equal(await previewButton(page).evaluate(button => button === document.activeElement), true);
  await previewButton(page).click(); await waitStatus(page, '预览已固定'); await confirmation(page).check();
  await closeButton(page).focus(); await closeButton(page).press('Tab'); assert.equal(await confirmation(page).evaluate(input => input === document.activeElement), true);
  await confirmation(page).press('Shift+Tab'); assert.equal(await closeButton(page).evaluate(button => button === document.activeElement), true);
  for (let index = 0; index < 8; index++) { await page.keyboard.press('Tab'); assert.equal(await dialog(page).evaluate(element => element.contains(document.activeElement)), true); }
  await page.keyboard.press('Escape'); assert.equal(await dialog(page).count(), 0);
  assert.equal(await page.locator('#open-object-mutation').evaluate(button => button === document.activeElement), true); assert.equal((await calls(page, 'typed_execute')).length, 0);
}));

test('成功outcome仍有resources_pending时不关闭/不伪造committed，直到status=false才允许关闭', options, async t => withPage(t, { resourcesPending: true, statusPending: true }, async (page, caseDirectory) => {
  await prepared(page); const evidence = await dialog(page).locator('pre').textContent();
  await confirmation(page).check(); await submitButton(page).click(); await waitStatus(page, '资源排空结果未知'); await waitCall(page, 'status');
  assert.equal(await closeButton(page).isDisabled(), true); assert.equal(await dialog(page).getAttribute('aria-busy'), 'true');
  assert.equal(await submitButton(page).isDisabled(), true); assert.equal(await page.evaluate(() => window.objectMutationFixture.committed), 0);
  await closeButton(page).evaluate(button => button.click()); await page.keyboard.press('Escape'); await page.evaluate(() => window.objectMutationFixture.dialog.close());
  assert.equal(await dialog(page).count(), 1); assert.equal(await dialog(page).locator('pre').textContent(), evidence);
  await page.screenshot({ path: path.join(caseDirectory, 'pending-resource-close-blocked.png'), fullPage: true });
  await page.evaluate(() => { window.objectMutationFixture.statusPending = false; }); await waitClosable(page);
  assert.equal(await dialog(page).getAttribute('aria-busy'), 'false'); assert.equal(await previewButton(page).isDisabled(), true); assert.equal(await submitButton(page).isDisabled(), true);
  assert.equal(await page.evaluate(() => window.objectMutationFixture.committed), 0); assert.equal((await calls(page, 'typed_execute')).length, 1);
  const [typed] = await calls(page, 'typed_execute'); for (const read of await calls(page, 'status')) assert.deepEqual(read.args[0], typed.args[0]);
  await closeAndRestore(page);
}));

test('快速双击和程序click在grant期间仍singleflight，最终只消费一份签发typed计划', options, async t => withPage(t, { holdGrant: true }, async page => {
  await prepared(page); await confirmation(page).check();
  await guardedSubmit(page); await waitCall(page, 'grant');
  assert.equal((await calls(page, 'grant')).length, 1); assert.equal((await calls(page, 'typed_execute')).length, 0); assert.equal(await closeButton(page).isDisabled(), true);
  await release(page, 'Grant'); await page.waitForFunction(() => document.querySelectorAll('[role="dialog"]').length === 0);
  assert.equal((await calls(page, 'grant')).length, 1); assert.equal((await calls(page, 'typed_execute')).length, 1); assert.equal(await page.evaluate(() => window.objectMutationFixture.committed), 1);
}));
