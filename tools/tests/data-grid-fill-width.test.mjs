import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';
import ts from 'typescript';

// Execute the production methods verbatim, with only DOM/layout reads stubbed.
const source = await readFile(new URL('../../src/ui/DataGrid.ts', import.meta.url), 'utf8');
const tree = ts.createSourceFile('DataGrid.ts', source, ts.ScriptTarget.Latest, true);
const cls = tree.statements.find(n => ts.isClassDeclaration(n) && n.name?.text === 'DataGrid');
const names = ['effectiveColWidths', 'rowHeaderPx', 'onResizeKeyDown', 'onMouseMove'];
const members = names.map(name => cls.members.find(n => n.name?.getText(tree) === name).getText(tree));
const context = vm.createContext({ Set });
vm.runInContext(ts.transpile(`class ActualGrid { ${members.join('\n')} }; globalThis.ActualGrid = ActualGrid;`, { target: ts.ScriptTarget.ES2021 }), context);
function grid(widths = [120, 80, 200], available = 1000) {
  const value = new context.ActualGrid();
  Object.assign(value, {
    opts: { fillWidth: true, showRowNumbers: false, rows: [] },
    colWidths: [...widths], baseWidths: [...widths], userSizedCols: new Set(),
    scroll: { clientWidth: available }, defaultWidth: 96,
    applyColumnWidths() {}, thead: { querySelector: () => null },
  });
  return value;
}
const widths = g => Array.from(g.effectiveColWidths());
function key(g, value, col = 0) {
  const th = { dataset: { col: String(col) } };
  const handle = { closest: () => th };
  g.onResizeKeyDown({ key: value, target: { closest: () => handle }, preventDefault() {}, stopPropagation() {} });
}

test('fillWidth distributes free space proportionally without mutating content widths', () => {
  const g = grid();
  assert.deepEqual(widths(g), [300, 200, 500]);
  assert.deepEqual(g.colWidths, [120, 80, 200]);
  g.scroll.clientWidth = 600;
  assert.deepEqual(widths(g), [180, 120, 300]);
  g.scroll.clientWidth = 1000;
  assert.deepEqual(widths(g), [300, 200, 500]);
});
test('opt-out, detached and overflowing grids preserve their widths', () => {
  const g = grid();
  g.opts.fillWidth = false;
  assert.deepEqual(widths(g), [120, 80, 200]);
  g.opts.fillWidth = true;
  for (const w of [0, 200, 400]) {
    g.scroll.clientWidth = w;
    assert.deepEqual(widths(g), [120, 80, 200]);
  }
});
test('row header and rounding are included in the exact table width', () => {
  const g = grid([65, 65, 65], 1001);
  g.opts.showRowNumbers = true;
  g.opts.rowHeaderWidth = 48;
  assert.equal(widths(g).reduce((a, b) => a + b, 0) + g.rowHeaderPx(), 1001);
  g.opts.rowHeaderWidth = undefined;
  g.opts.rowNumberOffset = 100000;
  assert.equal(g.rowHeaderPx(), 58);
});
test('manual widths stay fixed while remaining columns fill resized containers', () => {
  const g = grid();
  g.colWidths[0] = 350;
  g.userSizedCols.add(0);
  assert.equal(widths(g)[0], 350);
  assert.equal(widths(g).reduce((a, b) => a + b), 1000);
  g.scroll.clientWidth = 1500;
  assert.equal(widths(g)[0], 350);
  assert.equal(widths(g).reduce((a, b) => a + b), 1500);
  g.userSizedCols = new Set([0, 1, 2]);
  assert.deepEqual(widths(g), [350, 80, 200]);
});
test('first keyboard resize starts at the displayed width; Home restores auto sizing', () => {
  const g = grid();
  key(g, 'ArrowRight');
  assert.equal(g.colWidths[0], 308);
  assert.equal(widths(g)[0], 308);
  key(g, 'ArrowLeft');
  assert.equal(widths(g)[0], 300);
  key(g, 'Home');
  assert.equal(g.userSizedCols.has(0), false);
  assert.deepEqual(widths(g), [300, 200, 500]);
  g.opts.fillWidth = false;
  key(g, 'Home');
  assert.equal(g.colWidths[0], 96, 'existing grids retain the original Home behavior');
});
test('pointer resize starts at displayed width and a zero-motion event does not reallocate', () => {
  const g = grid();
  g.resizing = { col: 0, startX: 100, startW: 300 };
  g.onMouseMove({ clientX: 100 });
  assert.deepEqual(widths(g), [300, 200, 500]);
  assert.equal(g.userSizedCols.size, 0);
  g.onMouseMove({ clientX: 124 });
  assert.equal(widths(g)[0], 324);
  assert.equal(g.userSizedCols.has(0), true);
  g.onMouseMove({ clientX: 100 });
  assert.equal(widths(g)[0], 300);
});
