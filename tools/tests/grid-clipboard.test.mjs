import assert from 'node:assert/strict';
import test from 'node:test';
import { moduleAt, harness, Element, plain, deferred } from './draft-grid-fixture.mjs';
const { encodeGridCopy, CLIPBOARD_LIMIT } = await moduleAt('src/core/grid-clipboard.ts');
const snap = (rows, names = ['a', 'b']) => ({ generation: 1, columns: names.map(name => ({ name })), rows, cell: rows[0]?.[0] });
test('普通含头、rows 无头、cell 仅一格、显式含头契约', () => {
  const s = snap([['1', '2'], ['3', '4']]); assert.equal(encodeGridCopy(s, 'default'), 'a\tb\r\n1\t2\r\n3\t4'); assert.equal(encodeGridCopy(s, 'rows'), '1\t2\r\n3\t4'); assert.equal(encodeGridCopy(s, 'cell'), '1'); assert.equal(encodeGridCopy(s, 'rows_with_headers'), encodeGridCopy(s, 'default'));
});
test('NULL、空串、字面量 \\N、TSV tab CR LF 引号和 Unicode 原样编码', () => {
  assert.equal(encodeGridCopy(snap([[null, ''], ['\\N', 'a\tb\r\n"😀é"']]), 'rows'), '\\N\t\r\n"\\N"\t"a\tb\r\n""😀é"""');
  assert.equal(encodeGridCopy(snap([[null]], ['a']), 'cell'), 'NULL'); assert.equal(encodeGridCopy(snap([['']], ['a']), 'cell'), '');
});
test('bigint、decimal 保留十进制文字，typed binary 有明确 0x，不经过 Number', () => {
  assert.equal(encodeGridCopy(snap([[{ kind: 'integer', value: '90071992547409931234' }, { kind: 'decimal', value: '-0.000000000000123400' }]]), 'rows'), '90071992547409931234\t-0.000000000000123400');
  assert.equal(encodeGridCopy(snap([[{ kind: 'binary', encoding: 'base64', value: 'AP/+QQ==' }]], ['x']), 'cell'), '0x00fffe41');
  assert.throws(() => encodeGridCopy(snap([[{ kind: 'decimal', value: 1 }]], ['x']), 'cell'), /完整文本/);
});
test('安全表格模式显式转义危险文本，普通模式不改原值；10 MiB 超限拒绝', () => {
  assert.equal(encodeGridCopy(snap([['=1+2', '-5']]), 'rows'), '=1+2\t-5'); assert.match(encodeGridCopy(snap([['=1+2', { kind: 'integer', value: '-5' }]]), 'spreadsheet_safe'), /'=1\+2\t-5/);
  assert.equal(encodeGridCopy(snap([['x'.repeat(CLIPBOARD_LIMIT)]], ['x']), 'cell').length, CLIPBOARD_LIMIT);
  assert.throws(() => encodeGridCopy(snap([['😀'.repeat(CLIPBOARD_LIMIT / 4 + 1)]], ['x']), 'cell'), /未截断/);
});
test('实际 DataGrid 冻结显示顺序、多选、右键范围、命中 cell；刷新拒绝旧菜单', () => {
  const h = harness(), grid = new h.context.DataGrid(); grid.setData({ columns: [{ name: 'visible2' }, { name: 'visible1' }], rows: [['排序后B', '2'], ['排序后A', '1']], selectedRows: new Set([1, 0]) });
  grid.lastFocused = { row: 1, col: 1 }; assert.equal(encodeGridCopy(grid.freezeCopy(), 'rows'), '排序后B\t2\r\n排序后A\t1');
  grid.contextTarget = { row: 0, col: 1 }; grid.frozenContext = grid.captureCopy(true); assert.equal(encodeGridCopy(grid.freezeCopy(true), 'cell'), '2');
  const frozen = grid.freezeCopy(true); grid.opts.rows[0][0] = '稍后改变'; assert.equal(frozen.rows[0][0], '排序后B');
  grid.setData({ columns: [{ name: 'new' }], rows: [['new']] }); assert.throws(() => grid.freezeCopy(true), /旧菜单选择已失效/);
});
test('右键未选行只用命中行；无焦点/空结果拒绝，不回退 SQL', () => {
  const h = harness(), grid = new h.context.DataGrid(); grid.setData({ columns: [{ name: 'a' }], rows: [['a'], ['b']], selectedRows: new Set([0]) });
  grid.contextTarget = { row: 1, col: 0 }; assert.deepEqual(plain(grid.captureCopy(true).rows), [['b']]);
  grid.setData({ columns: [], rows: [], selectedRows: new Set() }); assert.throws(() => encodeGridCopy(grid.freezeCopy(), 'default'), /已加载/);
});
test('实际 App 按 grid/editor owner 分流，菜单结果不复制 SQL 全文，异步载荷冻结', async () => {
  const writes = [], pending = deferred(), h = harness({ encodeGridCopy }); const app = Object.create(h.context.App.prototype), tab = { id: 'q', kind: 'query' };
  const view = Object.create(h.context.QueryTabView.prototype), grid = new h.context.DataGrid(); grid.setData({ columns: [{ name: '结果' }], rows: [['真实结果']], selectedRows: new Set([0]) });
  view.grid = grid; view.editor = { hasSelection: () => true, getSelection: () => '选择的 SQL', getSql: () => 'SQL 全文' };
  Object.assign(app, { tabs: [tab], views: new Map([['q', view]]), activeTabId: 'q', copyOwner: { tabId: 'q', kind: 'grid', grid }, toast() {}, writeClipboard: async text => { writes.push(text); await pending.promise; } });
  const copying = app.cmdCopy(); grid.setData({ columns: [{ name: '新结果' }], rows: [['新值']] }); pending.resolve(); await copying; assert.deepEqual(writes, ['结果\r\n真实结果']);
  app.copyOwner = { tabId: 'q', kind: 'editor' }; await app.cmdCopy(); assert.equal(writes[1], '选择的 SQL'); view.editor.hasSelection = () => false; await app.cmdCopy(); assert.equal(writes.length, 2);
});
test('实际剪贴板失败显示手动文本，不能假 toast；不保存历史', async () => {
  let shown; const notices = [], h = harness({ navigator: { clipboard: { writeText: async () => { throw new Error('拒绝'); } } }, showDraftText: (...args) => { shown = args; } });
  const app = Object.create(h.context.App.prototype); app.toast = (...args) => notices.push(args); await app.writeClipboard('要手动复制的值'); assert.equal(shown[1], '要手动复制的值'); assert.equal(notices.length, 0);
});
