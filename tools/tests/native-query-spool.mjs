// Native Tauri regression: one execution, cached pages, full exports, independent retry.
// Copies the tested executable to isolated portable data; never uses user connections.
import assert from "node:assert/strict";
import path from "node:path";
import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { chromium } from "playwright-core";

const sourceExe = path.resolve(process.env.NAVKIT_EXE || "src-tauri/target/release/navkit.exe");
const directory = path.resolve("artifacts", `query-spool-${Date.now()}`);
await mkdir(directory, { recursive: true });
const isolatedExe = path.join(directory, "navkit.exe");
await copyFile(sourceExe, isolatedExe);
const port = Number(process.env.NATIVE_CDP_PORT || 9381);
const report = { result: "FAIL", sourceExe, directory, exeSha256: createHash("sha256").update(await readFile(sourceExe)).digest("hex"), checks: [] };
const check = label => { report.checks.push(label); console.log(`PASS ${label}`); };
const child = spawn(isolatedExe, [], { cwd: directory, stdio: "inherit", env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` } });
let browser, page, startupError;
child.on("error", error => { startupError = error; });
const invoke = (command, args = {}) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
const state = () => page.evaluate(() => {
  const v = window.__APP__.activeView(); const set = v.currentSet();
  return { status: v.execution.state, running: v.running, busy: v.resultBusy(), spool: v.tab.results?.spool_id,
    result: v.tab.activeResult, rows: set?.rows.length, total: set?.total, first: set?.rows[0], last: set?.rows[set.rows.length - 1],
    range: v.el.querySelector(".query-page-range")?.textContent, note: v.el.querySelector(".result-preview-note")?.textContent,
    sent: window.__QUERY_SPOOL_TRACE__.map(row => row.sql) };
});
try {
  const deadline = Date.now() + 45000;
  let lastError;
  while (Date.now() < deadline) {
    if (startupError) throw startupError;
    if (child.exitCode !== null) throw new Error(`Native app exited: ${child.exitCode}`);
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 1000 }); break; }
    catch (error) { lastError = error; await new Promise(resolve => setTimeout(resolve, 200)); }
  }
  if (!browser) throw lastError || new Error("Native startup timeout");
  page = browser.contexts()[0].pages()[0];
  await page.waitForFunction(() => !!window.__APP__, null, { timeout: 30000 });
  assert.equal(await page.evaluate(() => !!window.__PREVIEW_MOCK__), false);
  assert.deepEqual(await invoke("list_connections"), []);
  check("Actual Tauri bridge with isolated portable data, no mock or user connection");
  const id = "query-spool-fixture";
  const config = { id, name: "Query spool regression", kind: "sqlite", host: "", port: 0, user: "", password: "", database: "main", file: path.join(directory, "fixture.sqlite"), params: "", group: "", read_only: false, keep_alive: false };
  await invoke("save_connection", { config, password: "" });
  assert.equal((await invoke("open_connection", { id })).ok, true);
  await invoke("execute_sql", { id, database: "main", sql: "CREATE TABLE t_big(id INTEGER PRIMARY KEY, payload TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<12005) INSERT INTO t_big SELECT x,'中文' FROM n" });
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForFunction(id => window.__APP__?.connectionsValue().some(c => c.id === id), id, { timeout: 15000 });
  await page.evaluate(id => window.__APP__.openQueryTab(id, "SELECT * FROM t_big ORDER BY id", "main"), id);
  await page.waitForFunction(() => window.__APP__.activeView()?.completionLoaded, null, { timeout: 15000 });
  await page.evaluate(() => {
    const v = window.__APP__.activeView(), original = v.execution.source;
    const traced = Object.create(original); window.__QUERY_SPOOL_TRACE__ = [];
    Object.defineProperty(traced, "executeSqlV2", { value: async request => {
      const row = { sql: request.sql, id: request.execution_id }; window.__QUERY_SPOOL_TRACE__.push(row);
      const out = await original.executeSqlV2.call(original, request); row.state = out.state; return out;
    } }); v.execution.source = traced;
    const sql = v.editor.getSql(); v.editor.view.dispatch({ selection: { anchor: 7, head: sql.length } });
  });
  await page.evaluate(() => window.__APP__.activeView().executeSelection(false));
  report.initial = await state();
  assert.equal(report.initial.status, "succeeded"); assert.equal(report.initial.total, 12005);
  assert.equal(report.initial.rows, 200); assert.deepEqual(report.initial.first, ["1", "中文"]);
  assert.deepEqual(report.initial.sent, ["SELECT * FROM t_big ORDER BY id"]);
  assert(report.initial.spool);
  check("Run with a residual selection sends the whole SQL exactly once; 12005 rows succeed with a 200-row page");
  await page.getByRole("button", { name: "下一页（读取本次执行缓存，不重新执行 SQL）", exact: true }).click();
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy());
  report.next = await state(); assert.equal(report.next.first[0], "201"); assert.equal(report.next.rows, 200);
  await page.getByRole("button", { name: "末页（读取本次执行缓存，不重新执行 SQL）", exact: true }).click();
  await page.waitForFunction(() => !window.__APP__.activeView().resultBusy());
  report.last = await state(); assert.equal(report.last.first[0], "12001"); assert.equal(report.last.last[0], "12005");
  assert.equal(report.last.rows, 5); assert.equal(report.last.sent.length, 1);
  check("Next and last buttons read the same cached execution beyond 10000 rows, without sending SQL again");
  for (const format of ["csv", "json", "sql"]) {
    const file = path.join(directory, `full.${format}`);
    const count = await page.evaluate(async ({ file, format }) => {
      const v = window.__APP__.activeView();
      return v.exportResultSnapshot({ connection_id: v.tab.connectionId, schema: "main", table: "query_result", file, format, has_header: true, delimiter: ",", encoding: "utf-8" }, v.captureResultExport());
    }, { file, format });
    assert.equal(count, 12005);
    const text = await readFile(file, "utf8");
    if (format === "json") { const rows = JSON.parse(text); assert.equal(rows.length, 12005); assert.equal(rows[12004].id, "12005"); }
    if (format === "csv") assert.equal(text.split(/\r?\n/).filter(Boolean).length, 12006);
    if (format === "sql") assert.equal((text.match(/INSERT INTO/g) || []).length, 12005);
    assert(text.includes("12005") && text.includes("中文"));
  }
  assert.equal((await state()).sent.length, 1);
  check("CSV, JSON and SQL each export all 12005 execution rows, including the last row, without a second query");
  await page.evaluate(() => window.__APP__.activeView().execute("SELECT * FROM missing_query_spool_table", false));
  report.failed = await state(); assert.equal(report.failed.status, "failed"); assert.equal(report.failed.spool, undefined);
  assert.equal(report.failed.result, -1); assert.equal(report.failed.running, false); assert.equal(report.failed.busy, false);
  assert.equal(await page.getByRole("button", { name: "结果 1", exact: true }).count(), 0);
  assert.equal(await page.evaluate(() => window.__APP__.activeView().tab.results), null);
  assert.equal(await page.evaluate(() => window.__APP__.activeView().resultPages.scope), null);
  check("A failed rerun clears the old snapshot and paging scope rather than exposing stale successful data");
  await page.evaluate(() => window.__APP__.activeView().execute("SELECT 42 AS value; SELECT '第二结果' AS label", false));
  report.recovered = await state(); assert.equal(report.recovered.status, "succeeded"); assert.deepEqual(report.recovered.first, ["42"]);
  assert.notEqual(report.recovered.spool, report.initial.spool);
  await page.getByRole("button", { name: "结果 2", exact: true }).click();
  assert.deepEqual((await state()).first, ["第二结果"]);
  await page.getByRole("button", { name: "结果 1", exact: true }).click();
  assert.deepEqual((await state()).first, ["42"]);
  check("The next execution is independent, releases the old snapshot, and keeps multiple result sets separate");
  report.frontendAssets = await page.evaluate(() => [...document.scripts].map(s => s.src).filter(Boolean));
  report.result = "PASS";
  await page.screenshot({ path: path.join(directory, "query-page.png") });
} catch (error) {
  report.error = { message: error.message, stack: error.stack }; console.error(error); process.exitCode = 1;
  if (page) await page.screenshot({ path: path.join(directory, "failure.png") }).catch(() => {});
} finally {
  if (browser) await browser.close().catch(() => {});
  if (child.exitCode === null) { const exited = new Promise(resolve => child.once("exit", resolve)); child.kill(); await exited; }
  await writeFile(path.join(directory, "report.json"), JSON.stringify(report, null, 2) + "\n");
  console.log(`RESULT ${report.result}; report: ${path.join(directory, "report.json")}`);
}
