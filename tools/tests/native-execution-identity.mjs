// Real Tauri/WebView2 execution-identity regression; never reads user connections or databases.
// NAVKIT_EXE selects the binary. --probe records an old binary's behavior without requiring a pass.
import assert from "node:assert/strict";
import path from "node:path";
import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { chromium } from "playwright-core";

const probe = process.argv.includes("--probe");
const sourceExe = path.resolve(process.env.NAVKIT_EXE || "dist-portable/navkit.exe");
const directory = path.resolve("artifacts", `execution-identity-${Date.now()}`);
await mkdir(directory, { recursive: true });
const isolatedExe = path.join(directory, "navkit.exe");
await copyFile(sourceExe, isolatedExe);
const exeSha256 = createHash("sha256").update(await readFile(isolatedExe)).digest("hex");
const port = Number(process.env.NATIVE_CDP_PORT || 9379);
assert(Number.isInteger(port) && port > 0 && port <= 65535, "Invalid CDP port");
const child = spawn(isolatedExe, [], {
  cwd: directory, stdio: "inherit",
  env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
});
let browser, page, startupError;
child.on("error", error => { startupError = error; });
const report = { sourceExe, exeSha256, directory, mode: probe ? "probe" : "regression", result: "FAIL", checks: [] };
const check = label => { report.checks.push(label); console.log(`PASS ${label}`); };
const invoke = (command, args = {}) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
try {
  const deadline = Date.now() + 45000;
  let lastError;
  while (Date.now() < deadline) {
    if (startupError) throw startupError;
    if (child.exitCode !== null) throw new Error(`Native app exited during startup: ${child.exitCode}`);
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 1000 }); break; }
    catch (error) { lastError = error; await new Promise(resolve => setTimeout(resolve, 200)); }
  }
  if (!browser) throw lastError || new Error("Native startup timeout");
  page = browser.contexts()[0].pages()[0];
  await page.waitForFunction(() => !!window.__APP__, null, { timeout: 30000 });
  assert.equal(await page.evaluate(() => !!window.__PREVIEW_MOCK__), false);
  check("Actual Tauri bridge, no preview/mock");
  report.frontendAssets = await page.evaluate(() => [...document.scripts].map(script => script.src).filter(Boolean));
  assert.deepEqual(await invoke("list_connections"), [], "Portable test data must be isolated");
  check("Fresh isolated portable data, no user connections");
  const connection = {
    id: "execution-identity-test", name: "Execution identity fixture", kind: "sqlite",
    host: "", port: 0, user: "", password: "", database: "main",
    file: path.join(directory, "fixture.sqlite"), params: "", group: "",
    read_only: false, keep_alive: false,
  };
  await invoke("save_connection", { config: connection, password: "" });
  assert.equal((await invoke("open_connection", { id: connection.id })).ok, true);
  await invoke("execute_sql", { id: connection.id, database: "main", sql: "CREATE TABLE TradeRecord(id INTEGER PRIMARY KEY, amount INTEGER); INSERT INTO TradeRecord VALUES(1,17),(2,29)" });
  // IPC setup updates persisted data; reload so the real App loads that new connection.
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForFunction(id => window.__APP__?.connectionsValue().some(connection => connection.id === id), connection.id, { timeout: 15000 });
  await page.evaluate(id => window.__APP__.openQueryTab(id, "SELECT * FROM TradeRecord", "main"), connection.id);
  await page.waitForFunction(() => window.__APP__.activeView()?.completionLoaded, null, { timeout: 15000 });
  await page.evaluate(() => {
    // Tauri's internal invoke property is readonly. Observe the controller's real
    // transport instead, forwarding every call/result unchanged to native IPC.
    const controller = window.__APP__.activeView().execution;
    const original = controller.source;
    if (typeof original?.executeSqlV2 !== "function") throw new Error("Execution transport is unavailable");
    const instrumented = Object.create(original);
    window.__EXECUTION_IDENTITY_TRACE__ = [];
    for (const [method, command] of [["issueGrant", "issue_execution_grant"], ["executeSqlV2", "execute_sql_v2"], ["cancelExecution", "cancel_execution"], ["executionStatus", "execution_status"]]) {
      if (typeof original[method] !== "function") continue;
      Object.defineProperty(instrumented, method, { value: async request => {
        const row = { command, request_id: request.execution_id, grant_present: Boolean(request.execution_grant) };
        window.__EXECUTION_IDENTITY_TRACE__.push(row);
        try {
          const result = await original[method].call(original, request);
          row.response_id = result?.execution_id; row.state = result?.state;
          return result;
        } catch (error) {
          row.error = { code: error?.code, message: error?.message || String(error) };
          throw error;
        }
      } });
    }
    controller.source = instrumented;
    if (controller.source !== instrumented) throw new Error("Execution transport cannot be instrumented");
  });
  const execute = async sql => {
    await page.evaluate(sql => window.__APP__.activeView().execute(sql, false), sql);
    return page.evaluate(() => {
      const view = window.__APP__.activeView();
      return { state: view.execution?.state, busy: view.execution?.busy, running: view.running,
        rows: view.tab.results?.sets?.[0]?.rows ?? null, text: view.el?.textContent ?? document.querySelector("#app")?.textContent ?? "",
        trace: window.__EXECUTION_IDENTITY_TRACE__ };
    });
  };
  report.first = await execute("SELECT * FROM TradeRecord");
  report.first.text = report.first.text.slice(-4000);
  console.log(JSON.stringify({ phase: "first query", state: report.first.state, rows: report.first.rows, trace: report.first.trace }, null, 2));
  if (probe) {
    report.result = report.first.trace.some(row => row.command === "execute_sql_v2" && row.response_id && row.response_id !== row.request_id) ? "REPRODUCED_ID_MISMATCH" : "PROBED";
  } else {
    assert.deepEqual(report.first.rows, [["1", "17"], ["2", "29"]], "SELECT * FROM TradeRecord must display actual rows");
    assert.equal(report.first.state, "succeeded"); assert.equal(report.first.busy, false);
    const grant = report.first.trace.find(row => row.command === "issue_execution_grant");
    const runs = report.first.trace.filter(row => row.command === "execute_sql_v2");
    assert(grant?.response_id, "The desktop must negotiate an execution grant");
    assert.equal(runs.length, 1, "A query must be sent exactly once, without automatic replay");
    assert.equal(runs[0].grant_present, true);
    assert.equal(runs[0].request_id, grant.response_id); assert.equal(runs[0].response_id, grant.response_id);
    check("Grant ID = execute request ID = response ID, SELECT renders real rows exactly once");
    report.second = await execute("SELECT amount FROM TradeRecord WHERE id=2");
    report.second.text = report.second.text.slice(-4000);
    assert.deepEqual(report.second.rows, [["29"]]); assert.equal(report.second.state, "succeeded");
    const secondGrant = report.second.trace.filter(row => row.command === "issue_execution_grant").at(-1);
    const secondRun = report.second.trace.filter(row => row.command === "execute_sql_v2").at(-1);
    assert.notEqual(secondGrant.response_id, grant.response_id, "Each execution has a new sealed identity");
    assert.equal(secondRun.request_id, secondGrant.response_id); assert.equal(secondRun.response_id, secondGrant.response_id);
    check("Next query uses a fresh identity and renders the correct result");
    report.result = "PASS";
  }
} catch (error) {
  report.error = { message: error.message, stack: error.stack };
  console.error(error); process.exitCode = 1;
} finally {
  if (browser) await browser.close().catch(() => {});
  if (child.exitCode === null) {
    const exited = new Promise(resolve => child.once("exit", resolve));
    child.kill(); await exited;
  }
  await writeFile(path.join(directory, "report.json"), JSON.stringify(report, null, 2) + "\n");
  console.log(`RESULT ${report.result}; report: ${path.join(directory, "report.json")}`);
}
