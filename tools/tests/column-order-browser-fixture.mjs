import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
import { build } from 'esbuild';
import { designerPage, launchDesignerBrowser, finishSave } from './table-designer-fixture.mjs';

export { launchDesignerBrowser, finishSave };

// 只提取当前工作树的类；不启动 App 构造函数，不替换实际 commandUnavailableReason。
const source = fs.readFileSync('src/core/App.ts', 'utf8');
const ast = ts.createSourceFile('App.ts', source, ts.ScriptTarget.Latest, true);
const declaration = name => {
  const node = ast.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === name);
  assert.ok(node, `当前 App 缺少 ${name}`);
  return node.getText(ast).replace(/^export /, '');
};
const appScript = ts.transpile(`
const TableDesigner = DesignerModules.TableDesigner;
const { MANUAL_TRANSACTION_WRITE_COMMANDS, manualTransactionUnavailableReason, manualTransactionStatusText } = AppFixtureModules;
class QueryTabView {} class DataTabView {} class ObjectsTabView {} class ModelTabView {} class SqlDesignTabView {}
${declaration('App')}
${declaration('TabView')}
${declaration('StructureTabView')}
globalThis.AppFixtureClasses = { App, TabView, StructureTabView };
`, { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None });
const bindings = await build({
  stdin: { contents: `export * from './src/core/manualTransaction'; export { TabToolbar } from './src/ui/TabToolbar';`, resolveDir: process.cwd(), loader: 'ts' },
  bundle: true, platform: 'browser', format: 'iife', globalName: 'AppFixtureModules', write: false, logLevel: 'silent',
  // 能力模块仍用实际 parseDriverParams/kindMeta；仅原生 IPC 不进入无数据库浏览器。
  plugins: [{ name: 'no-native-ipc', setup(plugin) {
    plugin.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'native-ipc', namespace: 'fixture-ipc' }));
    plugin.onLoad({ filter: /.*/, namespace: 'fixture-ipc' }, () => ({ contents: 'export function invoke() { throw new Error("浏览器测试禁止原生 IPC"); }', loader: 'js' }));
  } }],
});

export async function columnOrderPage(browser, kind = 'mysql') {
  const page = await designerPage(browser, kind);
  await page.addScriptTag({ content: bindings.outputFiles[0].text });
  await page.addScriptTag({ content: appScript });
  await page.evaluate(async () => {
    const f = fixture;
    f.snapshotReads = [];
    const readSnapshot = __api.readTableDesignSnapshot;
    __api.readTableDesignSnapshot = async (...args) => { f.snapshotReads.push(structuredClone(args)); return readSnapshot(...args); };
    designer.destroy();
    const app = globalThis.app = Object.create(AppFixtureClasses.App.prototype);
    const cfg = globalThis.cfg = { id: 'c', kind: f.kind, params: '', read_only: false };
    Object.assign(app, {
      connections: [cfg], tabs: [tab], activeTabId: tab.id, views: new Map(),
      tree: { selectedNodeId: '', find: () => undefined },
      menu: { renderBar() {} }, ribbon: { updateDisabled() {} }, contextMenus: () => [], viewMenu: () => [],
      manualTransactionController: { status: () => f.transactionStatus, busy: () => !!f.transactionBusy },
      draftChanged: id => { f.draftChanges ??= []; f.draftChanges.push(id); },
      notifyToolbar: () => { f.toolbarUpdates = (f.toolbarUpdates ?? 0) + 1; if (app.tabToolbar && app.views.has(tab.id)) app.tabToolbar.setItems(app.guardedTabToolbarItems(tab)); },
      renderStatus: () => { f.statusUpdates = (f.statusUpdates ?? 0) + 1; },
      toast: (kind, text) => f.notices.push([kind, text]),
    });
    const view = globalThis.view = new AppFixtureClasses.StructureTabView(app, tab);
    view.el.id = 'designer'; document.querySelector('#designer').replaceWith(view.el);
    globalThis.designer = view.designer; app.views.set(tab.id, view);
    app.tabToolbar = new AppFixtureModules.TabToolbar({ onCommand: id => { globalThis.commandPromise = app.runCommand(id); } });
    document.body.insertBefore(app.tabToolbar.el, view.el);
    await view.reload();
  });
  return page;
}

export async function selectField(page, name) {
  await page.getByRole('textbox', { name: `字段 ${name} 名称`, exact: true }).focus();
}
export async function reorderColumns(page) {
  await selectField(page, 'computed');
  await page.evaluate(async () => { await app.runCommand('move-up'); await app.runCommand('move-up'); });
}
export async function startSave(page, command = false) {
  await page.evaluate(command => { globalThis.savePromise = command ? app.runCommand('save') : view.saveChanges(); }, command);
  await page.getByRole('dialog', { name: '保存表结构', exact: true }).waitFor();
}
