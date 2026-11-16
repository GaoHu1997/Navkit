// 数据字典向导布局回归：对象步是「对象树 + 260px 已选清单」双栏。
// 历史上 1fr 列的最小宽度是 auto，被长表名 / 长批注的固有宽度撑开（实测 820px 对话框里内容宽 960），
// 于是向导内容区出现横向滚动条。这里用真实布局引擎，在常见与窄窗口下逐步断言没有横向溢出。
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { chromium } from 'playwright-core';

const root = fileURLToPath(new URL('../../', import.meta.url));
const bundle = await build({
  stdin: {
    contents: 'export { openDataDictionaryDialog } from "./src/dialogs/DataDictionaryDialog.ts";',
    resolveDir: root,
    loader: 'ts',
  },
  bundle: true, write: false, platform: 'browser', format: 'iife', globalName: 'dictFixture',
  loader: { '.css': 'empty' }, logLevel: 'silent',
});

// Quartz 表结构：长表名 + 长批注（截图里触发横向滚动条的真实数据形态）。
const tables = [
  ['dbswitch_assignment_config', '任务配置表'],
  ['dbswitch_assignment_job', 'JOB日志表'],
  ['dbswitch_assignment_task', '任务信息表'],
  ['dbswitch_blob_triggers', 'TRIGGER作为BLOB类型存储(用于QUARTZ用户用JDBC创建他们自己定制的Trigger)'],
  ['dbswitch_calendars', '以BLOB类型存储QUARTZ的CALENDAR日历信息,QUARTZ可配置一个日历,提供排除时间段功能'],
  ['dbswitch_cron_triggers', '存储CRON类型的触发器(包括Cron表达式和时区信息)'],
  ['dbswitch_database_connection', '数据库连接'],
  ['dbswitch_fired_triggers', '存储与已触发的TRIGGER相关的状态信息，以及相关联JOB的执行信息'],
  ['dbswitch_job_details', '存储每一个已配置的JOB的详细信息'],
  ['dbswitch_job_logback', 'JOB执行日志'],
  ['dbswitch_locks', ''],
];

const browser = await chromium.launch({
  executablePath: process.env.BROWSER_EXE || 'C:/Program Files/Google/Chrome/Application/chrome.exe',
  headless: true,
});
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.setContent('<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"></head><body></body></html>');
  await page.addStyleTag({ path: path.join(root, 'src/styles/base.css') });
  await page.addStyleTag({ path: path.join(root, 'src/styles/data-dictionary.css') });
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  await page.evaluate(payload => {
    window.__PREVIEW_MOCK__ = {
      list_databases: () => [['navkit_demo', 'MySQL']],
      dictionary_metadata: () => ({
        name: 'navkit_demo', doc_title: 'navkit_demo数据库设计文档',
        tables: payload.tables.map(([name, comment]) => ({
          name, comment, db_type: 'MySQL',
          columns: [{ order: 1, name: 'id', type_name: 'bigint', length: '', primary_key: true, auto_increment: true, nullable: false, default_value: '', comment: '主键' }],
        })),
        views: [{ name: 'v_quartz_overview', comment: 'Quartz 调度总览视图（长批注用于验证行内省略）', script: 'select 1', db_type: 'MySQL' }],
        routines: [],
      }),
    };
    window.conns = [{ id: 'c1', name: '本地 MySQL', kind: 'mysql' }];
    window.openDict = () => dictFixture.openDataDictionaryDialog(window.conns, window.conns[0], 'navkit_demo', () => {});
  }, { tables });

  const measure = () => page.evaluate(() => {
    const dialog = document.querySelector('.dialog');
    const panel = document.querySelector('.wizard-panel:not([hidden])');
    const pick = selector => panel?.querySelector(selector);
    const box = el => el && {
      sw: el.scrollWidth, cw: el.clientWidth,
      w: Math.round(el.getBoundingClientRect().width),
      right: Math.round(el.getBoundingClientRect().right),
    };
    return {
      dialog: box(dialog),
      inner: box(pick('.wizard-panel-inner')),
      card: box(pick('.wizard-panel-card')),
      tree: box(pick('.backup-tree')),
      list: box(pick('[data-role="dictionary-selected-list"]')),
      title: panel.querySelector('.wizard-panel-title')?.textContent ?? '',
    };
  });

  const noOverflow = (box, layer, label) => {
    if (!box) return;
    assert.ok(box.sw <= box.cw + 1, `${label}：${layer} 出现横向溢出（scrollWidth ${box.sw} > clientWidth ${box.cw}）`);
  };

  const checkStep = async (label, { expectTree = false } = {}) => {
    const m = await measure();
    noOverflow(m.dialog, '对话框', label);
    noOverflow(m.inner, '向导内容区', label);
    noOverflow(m.card, '内容卡片', label);
    if (expectTree) {
      noOverflow(m.tree, '对象树', label);
      noOverflow(m.list, '已选清单', label);
      assert.ok(m.tree.w > 0 && m.list.w > 0, `${label}：对象树与已选清单都应可见`);
      assert.ok(m.list.right <= m.card.right + 1, `${label}：已选清单越出内容卡片右边界（${m.list.right} > ${m.card.right}）`);
    }
    return m;
  };

  const next = () => page.locator('.wizard-actions button').nth(1);
  const close = () => page.locator('.dialog-footer > button').click();

  for (const viewport of [
    { width: 1280, height: 800 },
    { width: 1024, height: 768 },
    { width: 900, height: 700 },
    { width: 800, height: 640 },
  ]) {
    const label = `窗口 ${viewport.width}×${viewport.height}`;
    await page.setViewportSize(viewport);
    await page.evaluate(() => window.openDict());
    await page.waitForSelector('.dialog');
    await checkStep(`${label} · 步骤 1 数据源`);
    assert.ok(await page.evaluate(() => Boolean(document.querySelector('.dialog'))), `${label}：数据字典对话框应已打开`);

    await next().click();
    await page.waitForFunction(() => document.querySelectorAll('.backup-tree-leaf').length > 0);
    const step2 = await checkStep(`${label} · 步骤 2 对象`, { expectTree: true });
    assert.match(step2.title, /对象/);
    if (viewport.width >= 1024) {
      assert.ok(step2.tree.w >= 600, `${label}：加宽后的对象树应留出 ≥600px（实际 ${step2.tree.w}px），否则长表名/批注仍会挤压双栏`);
    }

    await next().click();
    await checkStep(`${label} · 步骤 3 格式`);
    await next().click();
    await checkStep(`${label} · 步骤 4 文件`);
    await close();
    await page.waitForFunction(() => document.querySelectorAll('.dialog').length === 0);
  }

  assert.deepEqual(errors, []);
  console.log(JSON.stringify({
    status: 'passed',
    scenarios: ['1280 / 1024 / 900 / 800 窗口下四步均无横向滚动条', '对象步双栏在卡片内且树宽 ≥600px（1280 / 1024）', '长表名与长批注在树内省略而不撑宽'],
  }, null, 2));
} finally {
  await browser.close();
}
