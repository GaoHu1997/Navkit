import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';
import ts from 'typescript';

const built = await build({ stdin: { contents: 'export { iconSvgByName, svg, ICON_INSERT_LEFT, ICON_KEY_GOLD, ICON_DB_TX, ICON_CROSS } from "./src/ui/icons.ts";', resolveDir: process.cwd(), loader: 'ts' }, bundle: true, write: false, platform: 'node', format: 'esm', logLevel: 'silent' });
const icons = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].text).toString('base64'));
const body = svg => svg.slice(svg.indexOf('>') + 1, svg.lastIndexOf('</svg>'));

test('insert has its dedicated icon without changing either primary-key owner', () => {
  assert.equal(body(icons.iconSvgByName('insert_field')), body(icons.svg(icons.ICON_INSERT_LEFT)));
  assert.equal(body(icons.iconSvgByName('dbn_edit')), body(icons.svg(icons.ICON_KEY_GOLD)));
  assert.notEqual(body(icons.iconSvgByName('insert_field')), body(icons.iconSvgByName('dbn_edit')));
});

test('transaction start and rollback resolve real registered icons, not a save icon or fallback', () => {
  assert.equal(body(icons.iconSvgByName('dbn_applyupdates')), body(icons.svg(icons.ICON_DB_TX)));
  assert.equal(body(icons.iconSvgByName('dbn_cancel')), body(icons.svg(icons.ICON_CROSS)));
});

const appText = await readFile('src/core/App.ts', 'utf8');
const app = ts.createSourceFile('App.ts', appText, ts.ScriptTarget.Latest, true);
const actions = [];
function scan(node) {
  if (ts.isObjectLiteralExpression(node)) {
    const values = Object.fromEntries(node.properties.filter(ts.isPropertyAssignment).filter(p => ts.isStringLiteral(p.initializer)).map(p => [p.name.getText(app).replaceAll('"', ''), p.initializer.text]));
    if (values.id) actions.push(values);
  }
  ts.forEachChild(node, scan);
}
scan(app);
test('actual toolbar commands reference the dedicated field and transaction aliases', () => {
  for (const [id, icon] of [['insert-field', 'insert_field'], ['primary-key', 'dbn_edit'], ['begin-transaction', 'dbn_applyupdates'], ['rollback-transaction', 'dbn_cancel']]) {
    assert.ok(actions.some(action => action.id === id && action.icon === icon), id);
  }
});

const transferText = await readFile('src/dialogs/ImportExportDialog.ts', 'utf8');
const transfer = ts.createSourceFile('ImportExportDialog.ts', transferText, ts.ScriptTarget.Latest, true);
const wizards = [], clones = [];
function scanTransfer(node) {
  if (ts.isNewExpression(node) && node.expression.getText(transfer) === 'Wizard') wizards.push(node);
  if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === 'cloneNode') clones.push(node);
  ts.forEachChild(node, scanTransfer);
}
scanTransfer(transfer);
test('all three transfer wizards mount existing panels and await their real submit callback', () => {
  // 导入向导 / 导出向导 / 结果导出向导（结果导出改为两步向导）。
  assert.equal(wizards.length, 3);
  assert.equal(clones.length, 0, 'visual-preview clones must not ship as product implementation');
  for (const [index, action] of ['runImport', 'runExport'].entries()) {
    const opts = wizards[index].arguments[0];
    const finish = opts.properties.find(p => ts.isPropertyAssignment(p) && p.name.getText(transfer) === 'onFinish');
    assert.match(finish.initializer.getText(transfer), new RegExp('await\\s+' + action + '\\s*\\('));
    assert.match(opts.getText(transfer), /host\.appendChild\(panels\[index\]\)/);
  }
  const resultFinish = wizards[2].arguments[0].properties.find(p => ts.isPropertyAssignment(p) && p.name.getText(transfer) === 'onFinish');
  assert.match(resultFinish.initializer.getText(transfer), /await\s+runExport\s*\(/);
});
