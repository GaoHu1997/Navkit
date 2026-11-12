// Browser UI regression only: explicit mocked IPC, not a native database acceptance test.
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { chromium } from "playwright-core";

const browser = await chromium.launch({
  executablePath: process.env.BROWSER_EXE || "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  headless: true,
});
const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
const errors = [];
page.on("pageerror", error => errors.push(error.message));
await page.addInitScript(() => {
  let mock;
  window.__QUERY_MENU_EVENTS__ = [];
  for (const type of ['focusin', 'pointerdown', 'blur']) {
    window.addEventListener(type, event => {
      const menu = document.querySelector('.sql-selection-menu');
      if (menu) window.__QUERY_MENU_EVENTS__.push({ type, target: event.target?.className || event.target?.nodeName || 'window', focused: document.activeElement?.className });
    }, true);
  }
  window.__QUERY_TEST_EXECUTIONS__ = [];
  Object.defineProperty(window, "__PREVIEW_MOCK__", {
    configurable: true,
    get: () => mock,
    set: value => {
      mock = value;
      mock["plugin:event|listen"] = () => 1;
      mock["plugin:event|unlisten"] = () => null;
      mock.completion_data = () => ({
        __tables__: ["users", "orders", "active_users"],
        __views__: ["active_users"], __schemas__: ["dbswitch"],
        users: ["id", "display_name"], orders: ["id", "user_id", "amount"],
        __meta__: {
          version: 2, schema: "dbswitch",
          relations: { users: { kind: "table", comment: "用户基础信息", columns_known: true } },
          columns: { users: { id: { type: "INTEGER", comment: "用户编号" }, display_name: { type: "TEXT", comment: null } } },
        },
      });
      mock.execute_sql = args => {
        window.__QUERY_TEST_EXECUTIONS__.push(args.sql);
        return { sets: [], messages: ["UI fixture executed"], elapsed_ms: 1 };
      };
    },
  });
});
const passed = [];
const ok = label => { passed.push(label); console.log(`PASS ${label}`); };
try {
  await page.goto(`${process.env.PREVIEW_URL || "http://localhost:5183"}/preview.html`);
  await page.waitForFunction(() => !!window.__APP__);
  const labels = await page.locator(".ribbon-btn").evaluateAll(nodes => nodes.map(n => n.getAttribute("aria-label")));
  const functionIndex = labels.indexOf("函数");
  assert.deepEqual(labels.slice(functionIndex, functionIndex + 3), ["函数", "查询", "备份"]);
  assert(!labels.includes("其它") && !labels.includes("图表"));
  ok("顶部顺序与隐藏入口");
  assert.equal(await page.locator('.ribbon-btn[aria-label="连接"] .ribbon-drop').count(), 0);
  ok("连接无展开标识");
  await page.evaluate(() => window.__APP__.openQueryTab("c1", "", "dbswitch"));
  await page.waitForSelector(".sql-editor .cm-content");
  await page.waitForFunction(() => window.__APP__.activeView().completionLoaded);
  assert.equal(await page.locator('.tab-toolbar [title="文本"]').count(), 0);
  ok("查询工具栏不展示文本操作");

  const setEditor = async (text, anchor = text.length, head = anchor) => {
    await page.evaluate(({ text, anchor, head }) => {
      const editor = window.__APP__.activeView().editor;
      editor.setSql(text);
      editor.view.dispatch({ selection: { anchor, head } });
      editor.focus();
    }, { text, anchor, head });
    // 等待编辑器状态（文本与选区）实际落定，避免按键落在旧状态上
    await page.waitForFunction(({ text, anchor, head }) => {
      const editor = window.__APP__.activeView().editor;
      const sel = editor.view.state.selection.main;
      return editor.getSql() === text && sel.from === anchor && sel.to === head;
    }, { text, anchor, head }, { timeout: 5000 });
  };
  const completion = page.locator(".cm-tooltip-autocomplete");
  await setEditor("SELECT * FROM us");
  await page.keyboard.press("Control+Space");
  await completion.waitFor({ state: "visible" });
  assert((await completion.innerText()).includes("users"));
  assert((await completion.innerText()).includes("用户基础信息"), "表候选行应跟真实表备注");
  await page.keyboard.press("Escape");
  await setEditor("SELECT id,  FROM users", 11, 11); // 光标位于 SELECT id, 之后（不含字面管道符）
  await page.keyboard.press("Control+Space");
  await completion.waitFor({ state: "visible" });
  // CodeMirror 候选详情在 .cm-completionDetail（可能与 label 分列），合并读取
  const columnText = (await completion.innerText()) + " | " + (await page.locator('.cm-completionDetail').allTextContents()).join(' | ');
  assert(columnText.includes("用户编号"), `字段候选行应跟备注：${columnText.slice(0,200)}`);
  assert(/INTEGER\s*·\s*用户编号/.test(columnText), `字段候选行为“类型 · 备注”：${columnText.slice(0,200)}`);
  ok("FROM 表名提示使用真实元数据");
  await page.keyboard.press("Escape");
  const aliasSql = "SELECT u. FROM users AS u";
  await setEditor(aliasSql, "SELECT u.".length);
  await page.keyboard.press("Control+Space");
  await completion.waitFor({ state: "visible" });
  const columnLabels = await completion.innerText();
  assert(columnLabels.includes("display_name") && !columnLabels.includes("amount"));
  ok("别名字段提示识别光标后的 FROM 并限定所属表");
  await mkdir("artifacts", { recursive: true });
  await page.screenshot({ path: "artifacts/query-completion.png", fullPage: true });
  await page.keyboard.press("Escape");

  const fullSql = "SELECT 1;\nSELECT 2;";
  const selectedSql = "SELECT 2;";
  const waitSql = async (count, expected) => {
    await page.waitForFunction(n => window.__QUERY_TEST_EXECUTIONS__.length >= n, count);
    assert.equal(await page.evaluate(() => window.__QUERY_TEST_EXECUTIONS__.at(-1)), expected);
    await page.waitForFunction(() => !window.__APP__.activeView().running);
  };
  // 审定语义：F5 与「运行」始终执行全文；只有「运行已选择的」读取选区（见 App.executeSelection 注释）
  await setEditor(fullSql, fullSql.indexOf(selectedSql), fullSql.length);
  await page.keyboard.press("F5");
  await waitSql(1, fullSql);
  ok("F5 执行全文（不因残留选区截断）");
  await setEditor(fullSql);
  await page.keyboard.press("Control+Enter");
  await waitSql(2, fullSql);
  ok("Ctrl+Enter 无选区执行全文");
  await setEditor(fullSql, fullSql.indexOf(selectedSql), fullSql.length);
  // A real right click inside the selected text must preserve and execute the selection.
  await page.locator(".cm-line").nth(1).click({ button: "right", position: { x: 24, y: 10 } });
  const menu = page.locator('.sql-selection-menu[role="menu"]');
  await menu.waitFor({ state: "visible" });
  await page.screenshot({ path: "artifacts/query-selection-menu.png", fullPage: true });
  await menu.locator(".menu-entry-label, .menu-entry").filter({ hasText: "运行已选择的" }).first().click();
  await waitSql(3, selectedSql);
  ok("选区右键悬浮菜单执行 SQL");
  await setEditor(fullSql, fullSql.indexOf(selectedSql), fullSql.length);
  await page.keyboard.press("Shift+F10");
  await menu.waitFor({ state: "visible" });
  await page.keyboard.press("Escape");
  await menu.waitFor({ state: "detached" });
  assert(await page.locator(".cm-content").evaluate(el => el.contains(document.activeElement) || el === document.activeElement));
  ok("选区菜单键盘打开、Esc关闭并恢复编辑器焦点");
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ mode: "browser-mocked-ipc", passed, screenshots: ["artifacts/query-completion.png", "artifacts/query-selection-menu.png"] }, null, 2));
} catch (error) {
  console.error('SQL menu diagnostics', await page.evaluate(() => window.__QUERY_MENU_EVENTS__).catch(() => []));
  throw error;
} finally {
  await browser.close();
}
