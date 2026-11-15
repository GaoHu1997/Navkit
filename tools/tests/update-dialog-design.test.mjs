// 软件更新对话框的局部样式约束：只用 base.css 令牌、选择器不外溢、不写死颜色。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { transform } from 'esbuild';

const css = await readFile(new URL('../../src/styles/update-dialog.css', import.meta.url), 'utf8');
const base = await readFile(new URL('../../src/styles/base.css', import.meta.url), 'utf8');
const source = await readFile(new URL('../../src/dialogs/UpdateDialog.ts', import.meta.url), 'utf8');

test('样式可解析，全部令牌在 base.css 中存在', async () => {
  assert.deepEqual((await transform(css, { loader: 'css', logLevel: 'silent' })).warnings, []);
  const tokens = new Set([...base.matchAll(/(--[\w-]+)\s*:/g)].map(match => match[1]));
  for (const match of css.matchAll(/var\((--[\w-]+)/g)) assert.ok(tokens.has(match[1]), '不存在的基础令牌：' + match[1]);
});

test('选择器限定在 .update-dialog 内，不写死颜色', () => {
  const selectors = [...css.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{/g)].map(match => match[1].trim());
  for (const selector of selectors) {
    assert.ok(
      selector.startsWith('.update-dialog') || selector.startsWith('@keyframes') || selector === 'to',
      '越界选择器：' + selector,
    );
  }
  assert.doesNotMatch(css, /#[0-9a-f]{3,8}\b/i);
});

test('打开即自动检查：源码不再渲染手动「检查更新」按钮', () => {
  assert.doesNotMatch(source, /textContent\s*=\s*"检查更新"/);
  assert.match(source, /updateCheck\(\)/);
  // 打开时先看 update_status，再自动进入 check()
  assert.match(source, /updateStatus\(\)/);
  assert.match(source, /await check\(\)/);
});

test('状态机覆盖六个状态且单一入口推导界面', () => {
  assert.match(source, /type UpdateState = "checking" \| "latest" \| "available" \| "downloading" \| "ready" \| "error"/);
  assert.match(source, /function setState\(next: UpdateState/);
  for (const label of ['正在检查更新', '已是最新版本', '发现新版本', '正在下载', '已就绪', '检查更新失败']) {
    assert.ok(source.includes(label), '缺少状态文案：' + label);
  }
});
