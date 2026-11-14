import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { build, transform } from 'esbuild';
import vm from 'node:vm';
import ts from 'typescript';

const root = process.cwd();
const paths = {
  app: 'src/core/App.ts',
  css: 'src/styles/base.css',
  toolbar: 'src/ui/TabToolbar.ts',
  picker: 'src/dialogs/ConnectionPickerDialog.ts',
  connection: 'src/dialogs/ConnectionDialog.ts',
  user: 'src/dialogs/UserDialog.ts',
  builder: 'src/dialogs/QueryBuilderDialog.ts',
  monitor: 'src/dialogs/ServerMonitorDialog.ts',
  schedule: 'src/dialogs/ScheduleDialog.ts',
  transfer: 'src/dialogs/ImportExportDialog.ts',
};
const source = Object.fromEntries(await Promise.all(Object.entries(paths).map(async ([key, path]) => [key, await readFile(path, 'utf8')])));
const app = ts.createSourceFile(paths.app, source.app, ts.ScriptTarget.Latest, true);
const actions = [], methods = new Map();
function visit(node) {
  if (ts.isObjectLiteralExpression(node)) {
    const values = Object.fromEntries(node.properties.filter(ts.isPropertyAssignment).flatMap(property => {
      const value = property.initializer;
      if (ts.isStringLiteral(value)) return [[property.name.getText(app).replaceAll('"', ''), value.text]];
      if (value.kind === ts.SyntaxKind.TrueKeyword || value.kind === ts.SyntaxKind.FalseKeyword) return [[property.name.getText(app), value.kind === ts.SyntaxKind.TrueKeyword]];
      return [];
    }));
    if (values.id) actions.push(values);
  }
  if (ts.isMethodDeclaration(node)) methods.set(node.name.getText(app), node.getText(app));
  ts.forEachChild(node, visit);
}
visit(app);
const cssRule = selector => {
  const at = source.css.indexOf(selector + ' {');
  assert.notEqual(at, -1, 'missing CSS selector: ' + selector);
  return source.css.slice(at, source.css.indexOf('}', at) + 1);
};

// Minimal DOM exercises the real toolbar renderer and its callback guards; no App/IPC is started.
class Element {
  constructor(tag) {
    this.tagName = tag.toUpperCase(); this.children = []; this.style = {}; this.attributes = new Map();
    this.listeners = new Map(); this.dataset = {}; this.className = ''; this.disabled = false;
    this.classList = {
      contains: value => this.className.split(/\s+/).includes(value),
      add: value => { if (!this.classList.contains(value)) this.className = (this.className + ' ' + value).trim(); },
    };
  }
  set innerHTML(value) { this.html = value; this.children = []; }
  get innerHTML() { return this.html ?? ''; }
  appendChild(child) { this.children.push(child); return child; }
  append(...children) { for (const child of children) this.appendChild(child); }
  replaceChildren(...children) { this.children = []; this.append(...children); }
  setAttribute(name, value) { this.attributes.set(name, value); }
  addEventListener(name, handler) { this.listeners.set(name, handler); }
  emit(name) { this.listeners.get(name)?.({}); }
}
const built = await build({ stdin: { contents: 'export { TabToolbar } from "./src/ui/TabToolbar.ts";', resolveDir: root, loader: 'ts' }, bundle: true, write: false, platform: 'node', format: 'esm', logLevel: 'silent' });
const { TabToolbar } = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].text).toString('base64'));

test('F01: snippet search uses full control styling without losing width or focus tokens', () => {
  const rule = cssRule('.snippet-search');
  for (const expected of ['width: calc(100% - 2 * var(--sp-3))', 'border: 1px solid var(--border-strong)', 'border-radius: var(--r-2)', 'font-family: var(--font-ui)', 'font-size: var(--fs-12)', 'height: var(--h-control)']) assert.ok(rule.includes(expected), expected);
  assert.ok(source.css.includes('input:focus,') && source.css.includes('box-shadow: 0 0 0 3px var(--ring)'));
});
test('F02: category active appearance is not overridden by inline border or gray background', () => {
  assert.doesNotMatch(source.picker, /el\.style\.cssText = "display:block;width:100%;text-align:left;border:0;background:transparent"/);
  assert.doesNotMatch(source.picker, /el\.style\.background = !query/);
  assert.match(source.picker, /classList\.toggle\("active", !query && cid === category\)/);
  const rule = cssRule('.conn-picker-cat.active');
  assert.ok(rule.includes('background: var(--accent-50)') && rule.includes('border-left-color: var(--accent-500)'));
});
test('F03: save-password checkbox shares the field column and retains password policy', () => {
  assert.match(source.connection, /general\.append\(field\("", savePassword\)\)/);
  assert.doesNotMatch(source.connection, /margin:0 0 14px 140px/);
  assert.ok(source.connection.includes('!saveCheckbox.checked ? ""'));
  assert.ok(source.connection.includes('existing?.password && !passwordChanged ? undefined : password.value'));
});
test('F04: all three save configurations use the real primary renderer while disabled clicks stay blocked', () => {
  const saves = actions.filter(action => action.id === 'save');
  assert.equal(saves.length, 3, 'query, structure, and shared view/routine each supply a save configuration（模型入口已移除）');
  assert.ok(saves.every(action => action.primary === true));
  const original = globalThis.document;
  globalThis.document = { createElement: tag => new Element(tag) };
  try {
    const calls = [], toolbar = new TabToolbar({ onCommand: id => calls.push(id) });
    const buttons = () => toolbar.el.children.flatMap(host => host.children).filter(child => child.tagName === 'BUTTON');
    for (const save of saves) {
      toolbar.setItems([save, { id: 'refresh', label: '刷新' }]);
      assert.ok(buttons()[0].classList.contains('primary'), save.label);
      assert.equal(buttons()[1].classList.contains('primary'), false);
      buttons()[0].emit('click');
      const beforeDisabled = calls.length;
      toolbar.setItems([{ ...save, disabled: true, disabledReason: '只读' }]);
      const disabled = buttons()[0];
      assert.equal(disabled.disabled, true); assert.ok(disabled.classList.contains('primary') && disabled.classList.contains('disabled'));
      assert.equal(disabled.title, '只读'); disabled.emit('click'); assert.equal(calls.length, beforeDisabled);
    }
    assert.deepEqual(calls, ['save', 'save', 'save']);
  } finally { if (original === undefined) delete globalThis.document; else globalThis.document = original; }
  const rule = cssRule('.tab-tool-btn.primary:disabled');
  assert.ok(rule.includes('background: var(--bg-subtle)') && rule.includes('color: var(--text-disabled)') && rule.includes('pointer-events: none'));
});
// F05 已随「模型」功能移除而删除：原测试引用不存在的 ModelDocumentView.ts 与 model-zoom-out 入口。

test('F06: SQL menu disabled color survives the later inherit rule without enabling callbacks', async () => {
  const rule = cssRule('.sql-selection-menu .menu-entry:disabled');
  assert.ok(rule.includes('color: var(--text-disabled)'));
  assert.ok(source.css.indexOf('.sql-selection-menu .menu-entry:disabled') > source.css.indexOf('.sql-selection-menu .menu-entry {'));
  const editor = await readFile('src/ui/SqlEditor.ts', 'utf8');
  assert.ok(editor.includes('disabled: !this.opts.onExecute') && editor.includes('disabled: !this.opts.onExplain'));
});
test('F07: CodeMirror search/replace/goto controls are locally scoped and use existing UI tokens', () => {
  const selector = '.sql-editor :is(.cm-search, .cm-goto-line) :is(.cm-textfield, .cm-button)';
  const rule = cssRule(selector);
  for (const expected of ['height: var(--h-control)', 'border: 1px solid var(--border-strong)', 'border-radius: var(--r-2)', 'background: var(--bg-surface)', 'font-family: var(--font-ui)', 'font-size: var(--fs-12)']) assert.ok(rule.includes(expected), expected);
  assert.ok(source.css.includes('.sql-editor :is(.cm-search, .cm-goto-line) .cm-button:focus-visible'));
  assert.doesNotMatch(rule, /gradient/);
});
test('F08: group rename explicitly sets input type and retains the existing rename callback', () => {
  const method = methods.get('cmdRenameGroup');
  assert.match(method, /input\.type = "text"/); assert.ok(method.includes('api.renameGroup'));
});
test('F09: host explanation uses the shared field column and tertiary text tokens', () => {
  assert.match(source.user, /body\.appendChild\(field\("", note\)\)/);
  assert.ok(source.user.includes('font-size:var(--fs-11);color:var(--text-tertiary)'));
  assert.doesNotMatch(source.user, /margin:-2px 0 6px 110px/);
  assert.ok(source.user.includes('const hostInput = textInput("%", 200)'));
});
test('F11: read-only SQL preview references the actual monospace token', () => {
  assert.ok(source.builder.includes('font-family:var(--font-mono)'));
  assert.ok(source.builder.includes('preview.readOnly = true'));
  assert.doesNotMatch(source.builder, /font-family:var\(--mono\)/);
});
test('F12: monitoring uses a single current-session/SQL table and retains its sampling lifecycle', () => {
  assert.ok(source.monitor.includes('table.className = "monitor-table"'));
  assert.ok(source.monitor.includes('row.dataset.sessionId = process.id'));
  assert.doesNotMatch(source.monitor, /header\.className = "tabs-header"|tabs-tab active|source\.serverInfo|source\.executeSql/);
  assert.ok(source.monitor.includes('controller.dispose()') && source.monitor.includes('controller.start(interval.valueAsNumber)') && source.monitor.includes('controller.stop()'));
});
test('F14: no-work-page title does not alter the open-connection lifecycle or actions', () => {
  const render = [...methods.values()].find(method => method.includes('empty.className = "empty-state"'));
  assert.ok(render, 'the real no-work-page renderer exists');
  assert.ok(render.includes('title.textContent = "尚未打开工作页"'));
  assert.ok(render.includes('mkAction("＋ 新建连接", true, "new-connection")'));
  assert.ok(render.includes('mkAction("打开现有连接", false, "open-connection")'));
  assert.ok(render.includes('b.addEventListener("click", () => void this.runCommand(cmd))'));
  assert.doesNotMatch(render, /title\.textContent = "还没有打开的连接"/);
});
test('F15: collation keeps its value and is labelled 排序规则, not row format', async t => {
  const { launchDesignerBrowser, columnOrderPage } = await import('./column-order-browser-fixture.mjs');
  const browser = await launchDesignerBrowser(); t.after(() => browser.close());
  const page = await columnOrderPage(browser); t.after(() => page.close());
  await page.evaluate(async () => { fixture.snapshot.design.options.row_format = 'DYNAMIC'; fixture.snapshot.options.row_format = 'DYNAMIC'; await view.reloadAfterLeave(); });
  await page.getByRole('tab', { name: '选项', exact: true }).click();
  await page.waitForFunction(() => designer.optionChoices.values !== null);
  const collation = page.getByRole('combobox', { name: '排序规则', exact: true }), rowFormat = page.getByRole('combobox', { name: '行格式', exact: true });
  assert.equal(await collation.count(), 1); assert.equal(await rowFormat.count(), 1);
  assert.equal(await collation.inputValue(), 'utf8mb4_bin'); assert.equal(await rowFormat.inputValue(), 'DYNAMIC');
  assert.equal(await collation.isEnabled(), true); assert.equal(await rowFormat.isEnabled(), true);
  assert.equal(await collation.evaluate(el => el.closest('label').querySelector('span').textContent), '排序规则');
  assert.equal(await rowFormat.evaluate(el => el.closest('label').querySelector('span').textContent), '行格式');
  await collation.selectOption('utf8mb4_general_ci');
  assert.deepEqual(await page.evaluate(() => ({ collation: designer.draft.options.collation, row_format: designer.draft.options.row_format, baseline: designer.snapshot.design.options.collation, dirty: view.draftDirty() })), { collation: 'utf8mb4_general_ci', row_format: 'DYNAMIC', baseline: 'utf8mb4_bin', dirty: true });
  await rowFormat.selectOption('COMPACT'); assert.equal(await collation.inputValue(), 'utf8mb4_general_ci');
  await page.evaluate(() => view.discardDraft());
  assert.equal(await collation.inputValue(), 'utf8mb4_bin'); assert.equal(await rowFormat.inputValue(), 'DYNAMIC'); assert.equal(await page.evaluate(() => view.draftDirty()), false);
});
test('second revision is retained: stacked schedule, full-row SQL and both real transfer wizards', () => {
  assert.ok(source.schedule.includes('"form-stacked", "schedule-job-dialog"'));
  // 任务型重设计：任务信息 / 任务配置（按动作类型 groupbox）/ 定时配置 三段式。
  assert.ok(source.schedule.includes('groupbox('));
  assert.ok(source.schedule.includes('任务配置'));
  assert.ok(source.transfer.includes('"form-stacked"'));
  assert.equal((source.transfer.match(/new Wizard\(/g) ?? []).length, 3);
  assert.doesNotMatch(source.transfer, /cloneNode/);
});
test('updated product CSS still parses without errors', async () => {
  const result = await transform(source.css, { loader: 'css', logLevel: 'silent' });
  assert.deepEqual(result.warnings, []);
});
test('query completion keeps the note inline after the name instead of right-aligning it', () => {
  const detail = cssRule('.sql-editor .cm-tooltip.cm-tooltip-autocomplete.sql-query-completion > ul > li .cm-completionDetail');
  assert.match(detail, /margin-left: 0/);
  assert.match(detail, /padding-left: 0/);
  assert.match(detail, /text-overflow: ellipsis/);
  const label = cssRule('.sql-editor .cm-tooltip.cm-tooltip-autocomplete.sql-query-completion > ul > li .cm-completionLabel');
  assert.match(label, /flex: 0 0 auto/);
  // 选中仍是蓝底白字，灰字降为半透明白，不另造颜色。
  assert.match(source.css, /li\[aria-selected="true"\] \.cm-completionDetail \{\s*color: rgba\(255, 255, 255, 0\.82\)/);
});
