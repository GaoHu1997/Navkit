// Native Tauri IPC + real SQLite regression. The executable and portable data are isolated.
import assert from "node:assert/strict";
import path from "node:path";
import { copyFile, mkdir, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { chromium } from "playwright-core";

const exe = path.resolve(process.env.NAVKIT_EXE || "src-tauri/target/debug/navkit.exe");
const directory = path.resolve("artifacts", `native-tools-${Date.now()}`);
await mkdir(directory, { recursive: true });
const isolatedExe = path.join(directory, "navkit.exe");
await copyFile(exe, isolatedExe);
const basePort = Number(process.env.NATIVE_CDP_PORT || 9236);
let startCount = 0;
let processHandle, browser, page;
const passed = [];
const ok = label => { passed.push(label); console.log(`PASS ${label}`); };
const start = async () => {
  const port = basePort + startCount++;
  processHandle = spawn(isolatedExe, [], {
    cwd: directory, stdio: "inherit",
    env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
  });
  let lastError;
  const deadline = Date.now() + 60000;
  while (Date.now() < deadline) {
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 1000 }); break; }
    catch (error) {
      lastError = error;
      if (processHandle.exitCode !== null) throw new Error(`Native app exited during startup: ${processHandle.exitCode}`, { cause: error });
      await new Promise(resolve => setTimeout(resolve, 250));
    }
  }
  if (!browser) throw lastError;
  page = browser.contexts()[0].pages()[0];
  await page.waitForFunction(() => !!window.__APP__, { timeout: 30000 });
  assert.equal(await page.evaluate(() => !!window.__PREVIEW_MOCK__), false, "Native acceptance must never use a preview mock");
};
const stop = async () => {
  if (browser) { await browser.close().catch(() => {}); browser = undefined; }
  if (processHandle && processHandle.exitCode === null) {
    const exited = new Promise(resolve => processHandle.once("exit", resolve));
    processHandle.kill();
    await exited;
  }
};
const invoke = async (command, args = {}) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
try {
  await start();
  for (const [id, name, group] of [["source", "测试源", "源分组"], ["target", "测试目标", "目标分组"], ["other", "测试其它", "目标分组"]]) {
    await invoke("save_connection", { config: {
      id, name, group, kind: "sqlite", host: "", port: 0, user: "", password: "", database: "",
      file: path.join(directory, `${id}.sqlite`), params: "", read_only: false, keep_alive: false,
    }, password: "" });
  }
  const sourceOpen = await invoke("open_connection", { id: "source" });
  assert.equal(sourceOpen.ok, true);
  await invoke("execute_sql", { id: "source", database: "main", sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, display_name TEXT NOT NULL); CREATE VIEW active_users AS SELECT id, display_name FROM users;" });
  // Close first: generation must reconnect and resolve main, not rely on an existing pool.
  await invoke("close_connection", { id: "source" });
  const inserted = await invoke("insert_generated_rows", { req: {
    connection_id: "source", schema: "main", table: "users", columns: ["id", "display_name"],
    rows: [["1", "张三"], ["2", "SELECT 'quoted'; --仅数据"]],
  } });
  assert.equal(inserted, 2);
  const sourceRows = await invoke("execute_sql", { id: "source", database: "main", sql: "SELECT id, display_name FROM users ORDER BY id" });
  assert.deepEqual(sourceRows.sets[0].rows, [["1", "张三"], ["2", "SELECT 'quoted'; --仅数据"]]);
  ok("数据生成：关闭连接后重连、真实 SQLite 写入并保留特殊文本");
  const completion = await invoke("completion_data", { id: "source", schemaName: "main" });
  assert(completion.__tables__.includes("users"));
  assert.deepEqual(completion.__views__, ["active_users"]);
  assert.deepEqual(completion.__schemas__, ["main"]);
  assert.deepEqual(completion.users, ["id", "display_name"]);
  assert.deepEqual(completion.active_users, ["id", "display_name"]);
  ok("SQL提示：原生读取表、视图、真实字段和模式");
  const sync = await invoke("run_sync", { opts: {
    task_id: null, source_connection: "source", source_schema: "main", source_table: "users",
    target_connection: "target", target_schema: "main", target_table: "copied_users",
    mode: "both", write_mode: "append", create_target: true, columns: [], where_clause: null,
  } });
  assert.equal(sync.rows, 2);
  assert.equal(sync.created, true);
  const targetRows = await invoke("execute_sql", { id: "target", database: "main", sql: "SELECT id, display_name FROM copied_users ORDER BY id" });
  assert.deepEqual(targetRows.sets[0].rows, sourceRows.sets[0].rows);
  ok("数据同步：真实创建目标表并逐值核对写入结果");
  await assert.rejects(invoke("insert_generated_rows", { req: {
    connection_id: "source", schema: "temp", table: "users", columns: ["id", "display_name"], rows: [["3", "wrong"]],
  } }), /main|附加库|SQLite|数据库\/模式不存在或没有访问权限/);
  const afterRejectedWrite = await invoke("execute_sql", { id: "source", database: "main", sql: "SELECT id, display_name FROM users ORDER BY id" });
  assert.deepEqual(afterRejectedWrite.sets[0].rows, sourceRows.sets[0].rows);
  ok("数据生成：拒绝错误 SQLite 命名空间且真实零写入");
  await invoke("move_connection", { id: "source", group: "目标分组", targetId: "target", position: "before" });
  await invoke("move_connection", { id: "other", group: "目标分组", targetId: "source", position: "after" });
  const moved = await invoke("list_connections");
  assert.deepEqual(moved.map(c => c.id), ["source", "other", "target"]);
  assert(moved.every(c => c.group === "目标分组"));
  ok("数据源：跨分组、before/after排序使用原生持久化命令");
  await stop();
  await start();
  const restored = await invoke("list_connections");
  assert.deepEqual(restored.map(c => c.id), ["source", "other", "target"]);
  assert(restored.every(c => c.group === "目标分组"));
  ok("数据源：真实重启后分组和顺序保留");
  const selectedQuery = "SELECT display_name FROM users WHERE id = 1;";
  const fullQuery = `SELECT 111 AS ignored;\n${selectedQuery}`;
  await page.evaluate(sql => window.__APP__.openQueryTab("source", sql, "main"), fullQuery);
  await page.waitForFunction(() => window.__APP__.activeView().completionLoaded);
  await page.evaluate(({ sql, selected }) => {
    const editor = window.__APP__.activeView().editor;
    editor.view.dispatch({ selection: { anchor: sql.indexOf(selected), head: sql.length } });
    editor.focus();
  }, { sql: fullQuery, selected: selectedQuery });
  await page.keyboard.press("F5");
  await page.waitForFunction(() => {
    const view = window.__APP__.activeView();
    return !view.running && view.tab.results?.sets.length === 1;
  });
  const selectedResult = await page.evaluate(() => window.__APP__.activeView().tab.results);
  assert.deepEqual(selectedResult.sets[0].rows, [["张三"]]);
  ok("查询：真实键盘 F5 仅执行选区并显示 SQLite 结果");
  await page.evaluate(() => {
    const editor = window.__APP__.activeView().editor;
    editor.view.dispatch({ selection: { anchor: editor.view.state.doc.length } });
    editor.focus();
  });
  await page.keyboard.press("Control+Enter");
  await page.waitForFunction(() => {
    const view = window.__APP__.activeView();
    return !view.running && view.tab.results?.sets.length === 2;
  });
  const fullResult = await page.evaluate(() => window.__APP__.activeView().tab.results);
  assert.deepEqual(fullResult.sets[0].rows, [["111"]]);
  assert.deepEqual(fullResult.sets[1].rows, [["张三"]]);
  ok("查询：无选区 Ctrl+Enter 真实执行两条完整 SQL");
  await page.getByRole("button", { name: "查询创建工具", exact: true }).click();
  const builder = page.getByRole("dialog", { name: "查询创建工具", exact: true });
  await builder.waitFor();
  await page.waitForFunction(() => document.querySelector('[aria-label="查询表或视图"]')?.options.length > 0);
  await builder.getByLabel("查询表或视图", { exact: true }).selectOption("users");
  await builder.getByLabel("输出字段 display_name", { exact: true }).waitFor();
  await builder.getByLabel("输出字段 id", { exact: true }).uncheck();
  await builder.getByRole("button", { name: "添加 AND 条件", exact: true }).click();
  await builder.getByLabel("条件 1 字段", { exact: true }).selectOption("display_name");
  await builder.getByLabel("条件 1 值", { exact: true }).fill("张三");
  await builder.getByRole("button", { name: "生成 SQL 预览", exact: true }).click();
  const builtSql = await builder.getByLabel("待插入 SQL 预览", { exact: true }).inputValue();
  assert(builtSql.includes('"main"."users"'));
  await builder.getByRole("button", { name: "插入查询编辑器", exact: true }).click();
  await builder.waitFor({ state: "detached" });
  await page.evaluate(sql => {
    const editor = window.__APP__.activeView().editor;
    const from = editor.getSql().lastIndexOf(sql);
    if (from < 0) throw new Error("Constructed SQL not inserted");
    editor.view.dispatch({ selection: { anchor: from, head: from + sql.length } });
    editor.focus();
  }, builtSql);
  await page.keyboard.press("F5");
  await page.waitForFunction(() => {
    const view = window.__APP__.activeView();
    return !view.running && view.tab.results?.sets.length === 1;
  });
  const builtResult = await page.evaluate(() => window.__APP__.activeView().tab.results);
  assert.deepEqual(builtResult.sets[0].rows, [["张三"]]);
  ok("查询创建工具：真实元数据构造 SELECT、仅插入，手动执行结果正确");
  // A separate native connection to the same file observes only committed data.
  await invoke("save_connection", { config: {
    id: "observer", name: "事务隔离观察者", group: "", kind: "sqlite", host: "", port: 0, user: "", password: "", database: "",
    file: path.join(directory, "source.sqlite"), params: "", read_only: true, keep_alive: false,
  }, password: "" });
  const observedCount = async () => {
    const result = await invoke("execute_sql", { id: "observer", database: "main", sql: "SELECT COUNT(*) FROM users" });
    return Number(result.sets[0].rows[0][0]);
  };
  const editAndRun = async sql => {
    await page.evaluate(sql => { const editor = window.__APP__.activeView().editor; editor.setSql(sql); editor.view.dispatch({ selection: { anchor: editor.view.state.doc.length } }); editor.focus(); }, sql);
    await page.keyboard.press("F5");
    await page.waitForFunction(() => !window.__APP__.activeView().running);
  };
  const startTransaction = async () => {
    await page.getByRole("button", { name: "开始事务", exact: true }).click();
    const confirmation = page.getByRole("dialog", { name: "开始手动事务", exact: true });
    await confirmation.getByRole("button", { name: "开始事务", exact: true }).click();
    await confirmation.waitFor({ state: "detached" });
    await page.waitForFunction(() => window.__APP__.manualTransactionFor("source")?.active);
  };
  const finishTransaction = async commit => {
    await page.getByRole("button", { name: commit ? "提交事务" : "回滚事务", exact: true }).click();
    const confirmation = page.getByRole("dialog", { name: commit ? "提交事务" : "回滚事务", exact: true });
    await confirmation.getByRole("button", { name: commit ? "提交" : "回滚", exact: true }).click();
    await confirmation.waitFor({ state: "detached" });
    await page.waitForFunction(() => !window.__APP__.manualTransactionFor("source")?.active);
  };
  await startTransaction();
  const explainDuringTransaction = page.getByRole("button", { name: /解释/ });
  assert(await explainDuringTransaction.isDisabled());
  assert.match(await explainDuringTransaction.getAttribute("title"), /先提交|先.*回滚/);
  await editAndRun("INSERT INTO users (id, display_name) VALUES (3, '未提交事务'); SELECT COUNT(*) FROM users;");
  assert.deepEqual(await page.evaluate(() => window.__APP__.activeView().tab.results.sets.at(-1).rows), [["3"]]);
  assert.equal(await observedCount(), 2);
  await assert.rejects(invoke("execute_sql", { id: "source", database: "main", transactionOwner: "other-query", sql: "SELECT COUNT(*) FROM users" }), /所有者|查询页|owner|事务/);
  await assert.rejects(invoke("insert_generated_rows", { req: { connection_id: "source", schema: "main", table: "users", columns: ["id", "display_name"], rows: [["99", "不应写入"]] } }), /事务/);
  assert.equal((await invoke("manual_transaction_status", { id: "source" })).failed, false);
  ok("手动事务：真实跨IPC同连接，外部观察者看不到未提交记录，拒其它页和自动写入");
  await page.screenshot({ path: path.join(directory, "native-manual-transaction.png"), fullPage: true });
  await finishTransaction(false);
  assert.equal(await observedCount(), 2);
  ok("手动事务：明确回滚后外部观察者与库文件保留原记录");
  await startTransaction();
  await editAndRun("INSERT INTO users (id, display_name) VALUES (3, '已提交事务');");
  assert.equal(await observedCount(), 2);
  await finishTransaction(true);
  assert.equal(await observedCount(), 3);
  ok("手动事务：明确提交后新记录才对独立连接可见");
  await startTransaction();
  await editAndRun("INSERT INTO no_such_table (id) VALUES (1);");
  await page.waitForFunction(() => window.__APP__.manualTransactionFor("source")?.failed);
  assert(await page.getByRole("button", { name: "提交事务", exact: true }).isDisabled());
  await finishTransaction(false);
  assert.equal(await observedCount(), 3);
  ok("手动事务：SQL错误进入失败态，禁提交且仅回滚恢复");
  await startTransaction();
  await editAndRun("INSERT INTO users (id, display_name) VALUES (4, '关闭时应回滚');");
  const ownerTab = await page.evaluate(() => window.__APP__.activeTab.id);
  await page.locator(".tab.active").hover();
  await page.locator(".tab.active .tab-close").click();
  const closing = page.getByRole("dialog", { name: "关闭查询并回滚", exact: true });
  await closing.getByRole("button", { name: "取消", exact: true }).click();
  assert((await invoke("manual_transaction_status", { id: "source" })).active);
  await page.locator(".tab.active").hover();
  await page.locator(".tab.active .tab-close").click();
  await closing.getByRole("button", { name: "回滚并关闭", exact: true }).click();
  await page.waitForFunction(id => !window.__APP__.tabs.some(tab => tab.id === id), ownerTab);
  assert.equal((await invoke("manual_transaction_status", { id: "source" })).active, false);
  assert.equal(await observedCount(), 3);
  ok("手动事务：关闭所有者可取消，确认后真实回滚再关闭页面");
  await page.evaluate(() => window.__APP__.openQueryTab("source", "SELECT id, display_name FROM users ORDER BY id", "main"));
  await page.waitForFunction(() => window.__APP__.activeView().completionLoaded);
  await editAndRun("SELECT id, display_name FROM users ORDER BY id");
  await page.getByRole("button", { name: "创建图表", exact: true }).click();
  await page.waitForSelector(".chart-panel .chart-plot svg");
  assert.equal(await page.evaluate(() => window.__APP__.activeTab.kind), "chart");
  const chart = page.locator(".chart-panel");
  assert((await chart.getByRole("status").innerText()).includes("已绘制 3 行"));
  await chart.getByLabel("图表类型", { exact: true }).selectOption("line");
  assert.equal(await chart.locator("svg").count(), 1);
  await chart.locator("summary").click();
  assert.equal(await chart.locator("tbody tr").count(), 3);
  assert((await chart.locator("tbody").innerText()).includes("3"));
  await page.screenshot({ path: path.join(directory, "native-chart.png"), fullPage: true });
  await chart.getByLabel("数值字段", { exact: true }).selectOption("1");
  assert((await chart.getByRole("status").innerText()).match(/数值|有效/));
  assert.equal(await chart.locator(".chart-plot").isVisible(), false);
  await page.locator(".tab.active").hover();
  await page.locator(".tab.active .tab-close").click();
  await page.waitForFunction(() => window.__APP__.activeTab.kind === "query");
  assert.equal(await observedCount(), 3);
  ok("图表：真实查询三行快照、折线切换、数据替代内容、非数值提示和关闭返回");
  await page.screenshot({ path: path.join(directory, "native-restarted.png"), fullPage: true });
  const report = { mode: "native-tauri-real-sqlite", executable: exe, isolatedDirectory: directory, passed };
  await writeFile(path.join(directory, "report.json"), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
} catch (error) {
  if (page) await page.screenshot({ path: path.join(directory, "native-failure.png"), fullPage: true }).catch(() => {});
  await writeFile(path.join(directory, "report-failed.json"), JSON.stringify({ mode: "native-tauri-real-sqlite", executable: exe, isolatedDirectory: directory, passed, error: String(error?.stack || error) }, null, 2));
  throw error;
} finally { await stop(); }
