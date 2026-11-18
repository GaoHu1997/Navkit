import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';
import path from 'node:path';
import { mkdir } from 'node:fs/promises';

const bundle = await build({ stdin: { contents: 'export { StructureSyncReview } from "./src/ui/StructureSyncReview.ts";', resolveDir: process.cwd(), loader: 'ts' }, bundle: true, write: false, format: 'iife', globalName: 'ReviewFixture', logLevel: 'silent' });
test('structural changes show per-item consent, block unsupported plans and remain keyboard labelled', async () => {
  const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe', headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 975, height: 794 } });
    await page.route('https://structure-review-fixture.test/**', r => r.fulfill({ contentType: 'text/html', body: '<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"></head><body><main class="dialog form-stacked" style="position:static;width:100%;height:auto;padding:16px"></main></body></html>' }));
    await page.goto('https://structure-review-fixture.test/');
    await page.addStyleTag({ path: path.join(process.cwd(), 'src/styles/base.css') });
    await page.addScriptTag({ content: bundle.outputFiles[0].text });
    await page.evaluate(() => {
      window.configurationChanges = 0; document.querySelector('main').addEventListener('change', () => window.configurationChanges++);
      window.review = new ReviewFixture.StructureSyncReview(() => {}); document.querySelector('main').append(window.review.el);
      window.report = { target_exists: true, alterable: true, unsupported: [], warnings: ['事务式重建保留原表数据；受引用列不会自动 CASCADE'], changes: [
        { kind: 'add_column', object: 'memo', before: null, after: 'TEXT', risk: 'safe' },
        { kind: 'alter_column', object: 'amount', before: 'TEXT', after: 'INTEGER', risk: 'scan_required' },
        { kind: 'drop_index', object: 'extra_idx', before: '["legacy"]', after: null, risk: 'destructive' },
      ], steps: [{ label: '新增备选表', sql: 'CREATE TABLE "target_shadow" ("amount" INTEGER, "memo" TEXT)' }, { label: '复制数据并校验', sql: 'INSERT INTO "target_shadow" SELECT "amount", NULL FROM "target"' }], summary: '3 项结构差异' };
      window.review.render(window.report);
    });
    const group = page.getByRole('group', { name: '结构变更预览' });
    const scan = group.getByRole('checkbox', { name: '确认修改列 · amount的扫描/转换风险' });
    const drop = group.getByRole('checkbox', { name: '确认删除索引 · extra_idx的破坏风险' });
    assert.equal(await group.getByRole('checkbox').count(), 2);
    assert.equal(await page.evaluate(() => review.ready), false);
    await scan.focus(); await page.keyboard.press('Space'); assert.equal(await scan.isChecked(), true);
    assert.equal(await page.evaluate(() => review.ready), false);
    await drop.check(); assert.equal(await page.evaluate(() => review.ready), true);
    assert.equal(await page.evaluate(() => review.approvedIds.length), 2);
    assert.equal(await page.evaluate(() => configurationChanges), 0, 'consent must not invalidate source configuration');
    await group.locator('summary').click(); assert.match(await group.locator('pre').textContent(), /CREATE TABLE/);
    await page.evaluate(() => review.setLocked(true)); assert.equal(await scan.isDisabled(), true); await page.evaluate(() => review.setLocked(false));
    for (const width of [975, 560, 390]) {
      await page.setViewportSize({ width, height: 794 });
      const result = await page.evaluate(() => ({ width: document.documentElement.clientWidth, scroll: document.documentElement.scrollWidth }));
      assert.ok(result.scroll <= result.width + 1, `review overflows ${width}px`);
    }
    await page.setViewportSize({ width: 975, height: 794 });
    await mkdir('artifacts/performance-review', { recursive: true });
    await group.screenshot({ path: 'artifacts/performance-review/structure-change-review.png' });
    await page.evaluate(() => { report.alterable = false; report.unsupported = ['删除列被外键引用：禁止自动 CASCADE']; review.render(report); });
    assert.equal(await page.evaluate(() => review.ready), false); assert.equal(await scan.isDisabled(), true);
    assert.match(await group.textContent(), /删除列被外键引用/);
    await page.evaluate(() => { report.alterable = true; report.unsupported = []; review.render(report); });
    assert.equal(await scan.isChecked(), false, 'new preview clears previous consent');
    assert.equal(await page.evaluate(() => review.approvedIds.length), 0);
    await page.evaluate(() => review.clear()); assert.equal(await group.isVisible(), false);
  } finally { await browser.close(); }
});
