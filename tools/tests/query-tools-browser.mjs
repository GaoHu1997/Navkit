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
        active_users: ["id", "display_name"],
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
  };
  const completion = page.locator(".cm-tooltip-autocomplete");
  await setEditor("SELECT * FROM us");
  await page.keyboard.press("Control+Space");
  await completion.waitFor({ state: "visible" });
  assert((await completion.innerText()).includes("users"));
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
  await setEditor(fullSql, fullSql.indexOf(selectedSql), fullSql.length);
  await page.keyboard.press("F5");
  await waitSql(1, selectedSql);
  ok("F5 优先执行选区");
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
  await menu.getByRole("menuitem", { name: /运行已选择的/ }).click();
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
