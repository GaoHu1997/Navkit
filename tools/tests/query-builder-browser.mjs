// Query-builder browser UI acceptance with explicit fixture IPC; native SQLite is tested separately.
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { chromium } from "playwright-core";
const browser = await chromium.launch({ executablePath: process.env.BROWSER_EXE || "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", headless: true });
const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
const errors = [], passed = [];
page.on("pageerror", error => errors.push(error.message));
await page.addInitScript(() => {
  let mock, firstUsers = true;
  window.__QB_EXECUTIONS__ = [];
  Object.defineProperty(window, "__PREVIEW_MOCK__", { configurable: true, get: () => mock, set: value => {
    mock = value;
    mock["plugin:event|listen"] = () => 1; mock["plugin:event|unlisten"] = () => null;
    mock.list_connections = () => [{ id: "c1", name: "构造器SQLite", kind: "sqlite", host: "", port: 0, user: "", password: "", database: "", file: "fixture.sqlite", params: "", group: "", read_only: false, keep_alive: false }];
    mock.ensure_connection = () => ({ ok: true, message: "fixture connected", elapsed_ms: 0 });
    mock.list_databases = () => [["main", "database"]];
    mock.list_tables = () => [["users", "table"], ["orders", "table"], ["active_users", "view"]];
    mock.completion_data = () => ({ __tables__: ["users", "orders", "active_users"], __schemas__: ["main"], __views__: ["active_users"], users: ["id", "display_name"], orders: ["id", "amount"], active_users: ["id"] });
    mock.get_table_schema = async args => {
      if (args.table === "users" && firstUsers) {
        firstUsers = false;
        await new Promise(resolve => { window.__QB_RELEASE_OLD__ = resolve; });
        window.__QB_OLD_RESOLVED__ = true;
      }
      return { table: args.table, columns: (args.table === "orders" ? ["id", "amount"] : ["id", "display_name"]).map(name => ({ name, type: name === "display_name" ? "TEXT" : "INTEGER", nullable: true, primary_key: name === "id", auto_increment: false, default: null, comment: "" })), primary_keys: ["id"], indexes: [], foreign_keys: [], ddl: "" };
    };
    mock.execute_sql = args => { window.__QB_EXECUTIONS__.push(args.sql); return { sets: [], messages: [], elapsed_ms: 0 }; };
  } });
});
const ok = message => { passed.push(message); console.log(`PASS ${message}`); };
try {
  await page.goto(`${process.env.PREVIEW_URL || "http://localhost:5183"}/preview.html`);
  await page.waitForFunction(() => !!window.__APP__);
  const original = "SELECT 7;\n-- keep this text\n";
  await page.evaluate(sql => window.__APP__.openQueryTab("c1", sql, "main"), original);
  await page.waitForFunction(() => window.__APP__.activeView().completionLoaded);
  const launch = page.getByRole("button", { name: "查询创建工具", exact: true });
  assert(await launch.isEnabled());
  await launch.click();
  let dialog = page.getByRole("dialog", { name: "查询创建工具", exact: true });
  await dialog.waitFor();
  await page.waitForFunction(() => typeof window.__QB_RELEASE_OLD__ === "function");
  await dialog.getByLabel("查询表或视图").selectOption("orders");
  await dialog.getByLabel("输出字段 amount").waitFor();
  await page.evaluate(() => window.__QB_RELEASE_OLD__());
  await page.waitForFunction(() => window.__QB_OLD_RESOLVED__);
  assert.equal(await dialog.getByLabel("输出字段 display_name").count(), 0);
  assert(await dialog.getByLabel("输出字段 amount").isChecked());
  ok("创建工具读取真实表视图元数据，旧表异步响应不覆盖新表");
  await dialog.getByLabel("查询表或视图").selectOption("users");
  await dialog.getByLabel("输出字段 display_name").waitFor();
  await dialog.getByLabel("输出字段 id", { exact: true }).uncheck();
  await dialog.getByRole("button", { name: "添加 AND 条件", exact: true }).click();
  await dialog.getByLabel("条件 1 字段", { exact: true }).selectOption("display_name");
  const hostile = "张三'; DROP TABLE users; --";
  await dialog.getByLabel("条件 1 值", { exact: true }).fill(hostile);
  await dialog.getByLabel("排序字段", { exact: true }).selectOption("display_name");
  await dialog.getByLabel("排序方向", { exact: true }).selectOption("desc");
  await dialog.getByLabel("最大查询行数", { exact: true }).fill("20");
  await dialog.getByRole("button", { name: "生成 SQL 预览", exact: true }).click();
  const sql = await dialog.getByLabel("待插入 SQL 预览", { exact: true }).inputValue();
  assert(sql.includes('"main"."users"'));
  assert(sql.includes("张三''; DROP TABLE users; --"));
  assert(sql.includes('ORDER BY "display_name" DESC') && sql.endsWith("LIMIT 20;"));
  assert.deepEqual(await page.evaluate(() => window.__QB_EXECUTIONS__), []);
  ok("字段白名单、过滤值转义、排序与行数生成SQL预览但不自动执行");
  await dialog.getByLabel("最大查询行数", { exact: true }).fill("21");
  assert(await dialog.getByRole("button", { name: "插入查询编辑器", exact: true }).isDisabled());
  assert.equal(await dialog.getByLabel("待插入 SQL 预览", { exact: true }).inputValue(), "");
  await dialog.getByRole("button", { name: "生成 SQL 预览", exact: true }).click();
  const finalSql = await dialog.getByLabel("待插入 SQL 预览", { exact: true }).inputValue();
  await mkdir("artifacts", { recursive: true });
  await page.screenshot({ path: "artifacts/query-builder.png", fullPage: true });
  await dialog.getByRole("button", { name: "插入查询编辑器", exact: true }).click();
  await dialog.waitFor({ state: "detached" });
  const inserted = await page.evaluate(() => window.__APP__.activeView().editor.getSql());
  assert(inserted.includes(original)); assert(inserted.includes(finalSql));
  assert.deepEqual(await page.evaluate(() => window.__QB_EXECUTIONS__), []);
  ok("修改配置使旧预览失效；插入只修改光标位置并保留原SQL");
  await page.evaluate(() => {
    const editor = window.__APP__.activeView().editor;
    editor.setSql("SELECT 1;\nreplace_me\nSELECT 3;");
    editor.view.dispatch({ selection: { anchor: 10, head: 20 } });
  });
  await launch.click();
  dialog = page.getByRole("dialog", { name: "查询创建工具", exact: true });
  await dialog.getByLabel("输出字段 id", { exact: true }).waitFor();
  await dialog.getByRole("button", { name: "生成 SQL 预览", exact: true }).click();
  await dialog.getByRole("button", { name: "插入查询编辑器", exact: true }).click();
  const replaced = await page.evaluate(() => window.__APP__.activeView().editor.getSql());
  assert(replaced.startsWith("SELECT 1;\n")); assert(replaced.endsWith("\nSELECT 3;")); assert(!replaced.includes("replace_me"));
  ok("有选区时仅替换选区SQL并保留前后内容");
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ mode: "browser-mocked-ipc", passed, screenshot: "artifacts/query-builder.png" }, null, 2));
} finally { await browser.close(); }
