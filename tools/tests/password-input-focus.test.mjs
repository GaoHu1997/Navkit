// 密码输入框的焦点行为回归（对应历史缺陷 P8-R：非清空输入会与旧值拼接）。
//
// 这里不启动浏览器：用最小 DOM shim 复现 passwordInput 的构造与 focus 语义，
// 断言"首次 focus 会全选已有内容"，从而让用户直接键入即可替换旧密码。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import ts from 'typescript';
import vm from 'node:vm';

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname.replace(/^\/(\w):/, '$1:')), '../..');
const dialogText = fs.readFileSync(path.join(root, 'src/ui/Dialog.ts'), 'utf8');

/** 提取 passwordInput 函数源码（不引入整个 Dialog.ts 的依赖）。 */
function passwordInputSource() {
  const source = ts.createSourceFile('Dialog.ts', dialogText, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const fn = source.statements.find(
    (node) => ts.isFunctionDeclaration(node) && node.name?.text === 'passwordInput',
  );
  assert(fn, 'Dialog.ts 中应有 passwordInput');
  return fn.getText(source);
}

/** 最小 input 元素：记录 listeners，select() 标记选中状态。 */
class FakeInput {
  constructor() {
    this.type = 'text';
    this.value = '';
    this.style = {};
    this.dataset = {};
    this.listeners = new Map();
    this.selected = false;
  }
  setAttribute() {}
  getAttribute() { return null; }
  addEventListener(type, handler) {
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    this.listeners.get(type).push(handler);
  }
  select() { this.selected = true; }
  /** 模拟浏览器：focus() 触发 focus 监听器。 */
  focus() {
    for (const handler of this.listeners.get('focus') ?? []) handler();
  }
}

function build() {
  const context = vm.createContext({ document: { createElement: () => new FakeInput() }, console });
  const source = `globalThis.passwordInput = ${passwordInputSource().replace(/^export\s+/, '')};`;
  const js = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None },
  }).outputText;
  vm.runInContext(js, context);
  return context;
}

test('P8-R: first focus selects the existing value so typing replaces it', () => {
  const ctx = build();
  const el = ctx.passwordInput('old-secret', 280);
  assert.equal(el.value, 'old-secret');
  assert.equal(el.selected, false, '未聚焦时不应选中');

  el.focus();
  assert.equal(el.selected, true, '首次聚焦应全选，用户键入即替换而非追加');
});

test('P8-R: re-focusing does not re-select while the user is editing', () => {
  const ctx = build();
  const el = ctx.passwordInput('old-secret');
  el.focus();
  assert.equal(el.selected, true);
  // 用户开始编辑（浏览器会自动取消选中），随后失焦再回来。
  el.selected = false;
  el.focus();
  assert.equal(el.selected, false, '二次聚焦不应再次全选，避免打断继续编辑');
});

test('P8-R: empty password field stays untouched on focus', () => {
  const ctx = build();
  const el = ctx.passwordInput();
  assert.equal(el.value, '');
  el.focus();
  assert.equal(el.selected, true, '空值全选是无操作但语义一致');
  assert.equal(el.value, '', '不得把任何值写入空密码框');
});

test('P8-R: password fields are still type=password and keep the given width', () => {
  const ctx = build();
  const el = ctx.passwordInput('', 280);
  assert.equal(el.type, 'password', '不得退化为明文输入');
  assert.equal(el.style.width, '280px');
});
