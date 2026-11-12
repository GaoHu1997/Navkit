// Run only against a freshly built, explicitly pinned native executable:
// NAVKIT_EXE=<absolute exe> NAVKIT_EXE_SHA256=<sha256> node tools/tests/database-workflows-desktop-native.mjs
// No preview server, transport replacement, shared connection, or external DB is used.
import assert from "node:assert/strict";
import { createHash, randomUUID } from "node:crypto";
import { createServer } from "node:net";
import { copyFile, mkdir, readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const source = process.env.NAVKIT_EXE;
const expectedHash = process.env.NAVKIT_EXE_SHA256?.toLowerCase();
assert.ok(source && path.isAbsolute(source), "NAVKIT_EXE must name this build's absolute native executable; there is no fallback executable");
assert.match(expectedHash || "", /^[a-f0-9]{64}$/, "NAVKIT_EXE_SHA256 must explicitly pin this build's executable");
const directory = path.join(root, "tools", "test-artifacts", `database-workflows-native-${randomUUID()}`);
await mkdir(directory, { recursive: true });
const report = { started_at: new Date().toISOString(), source, expected_sha256: expectedHash, directory, checks: [], batches: {}, operations: {}, exports: {}, cleanup_errors: [] };
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const hash = bytes => createHash("sha256").update(bytes).digest("hex");
const identifier = value => `"${value.replaceAll('"', '""')}"`;
const literal = value => `'${value.replaceAll("'", "''")}'`;
const check = message => { report.checks.push(message); console.log(`PASS ${message}`); };
const errorInfo = error => ({ name: error?.name, message: error?.message || String(error), code: error?.code, stack: error?.stack });
let child, browser, page, spawnError, failure;
const id = `native-workflows-${randomUUID()}`;
const owner = `database-workflows-native:${randomUUID()}`;
const namespace = "main";

async function freePort() {
  const server = createServer();
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const port = server.address().port;
  await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  return port;
}
async function invoke(command, args = {}) {
  const reply = await page.evaluate(async ({ command, args }) => {
    try { return { ok: true, value: await window.__TAURI_INTERNALS__.invoke(command, args) }; }
    catch (error) { return { ok: false, error: typeof error === "string" ? { message: error } : { code: error?.code, message: error?.message || JSON.stringify(error) } }; }
  }, { command, args });
  if (!reply.ok) throw Object.assign(new Error(`${command}: ${reply.error.message}`), { code: reply.error.code });
  return reply.value;
}
async function grant(sql) {
  const request = { execution_id: randomUUID(), connection_id: id, namespace, owner_id: owner, sql, timeout_ms: 15000, origin: "database-workflows-native" };
  const granted = await invoke("issue_execution_grant", { request });
  assert.equal(granted.sql, sql, "C1 grant must bind the exact SQL/typed operation marker");
  assert.equal(granted.connection_id, id);
  assert.equal(granted.namespace, namespace);
  assert.equal(granted.owner_id, owner);
  assert.ok(granted.execution_id && granted.execution_grant);
  return granted;
}
async function sqlOutcome(sql) { return invoke("execute_sql_v2", { request: await grant(sql) }); }
async function execute(sql) {
  const out = await sqlOutcome(sql);
  assert.equal(out.state, "succeeded", JSON.stringify(out));
  assert.equal(out.resources_pending, false, "SQL success must include settled resources");
  assert.ok(out.result?.sets?.length, "real SQL execution must have result sets");
  for (const set of out.result.sets) assert.equal(set.ok, true, JSON.stringify(set));
  return out.result;
}
async function query(sql) {
  const result = await execute(sql);
  const sets = result.sets;
  assert.equal(sets.length, 1, `expected one result for ${sql}`);
  assert.equal(sets[0].total, sets[0].rows.length, "small fixture must be read back completely");
  return sets[0].rows;
}
async function scalar(sql) {
  const rows = await query(sql);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].length, 1);
  return rows[0][0];
}
function metrics(snapshot, previous) {
  for (const key of ["scanned", "executed", "errors", "elapsed_ms"]) {
    assert.ok(Number.isSafeInteger(snapshot.metrics[key]) && snapshot.metrics[key] >= 0, `${key} must be a nonnegative integer`);
    if (previous) assert.ok(snapshot.metrics[key] >= previous[key], `${key} must not go backwards`);
  }
  assert.ok(snapshot.metrics.errors <= snapshot.metrics.executed);
  assert.ok(snapshot.metrics.executed <= snapshot.metrics.scanned);
}
async function batch(label, task, onProgress) {
  const request = { connection_id: id, namespace, owner_id: owner, timeout_ms: 30000, task };
  const ticket = await invoke("register_batch_execution", { request });
  assert.equal(ticket.connection_id, id);
  assert.equal(ticket.namespace, namespace);
  assert.equal(ticket.owner_id, owner);
  assert.equal(ticket.task_kind, task.kind);
  assert.ok(ticket.execution_id && ticket.execution_grant);
  const context = { execution_id: ticket.execution_id, connection_id: id, namespace, owner_id: owner };
  let snapshot = await invoke("run_batch_execution", { ticket });
  let previous, sequence = 0;
  const logs = [];
  const deadline = Date.now() + 45000;
  while (true) {
    assert.deepEqual(snapshot.context, context);
    assert.equal(snapshot.kind, task.kind);
    metrics(snapshot, previous); previous = { ...snapshot.metrics };
    for (const line of snapshot.logs) { assert.ok(line.sequence > sequence); sequence = line.sequence; logs.push(line); }
    if (onProgress) await onProgress(snapshot, context);
    if (snapshot.outcome && !snapshot.status.resources_pending) break;
    assert.ok(Date.now() < deadline, `batch ${label} did not publish a settled outcome`);
    await delay(80);
    snapshot = await invoke("batch_execution_status", { context, afterSequence: sequence });
  }
  assert.equal(snapshot.outcome.execution_id, ticket.execution_id);
  assert.equal(snapshot.outcome.state, snapshot.status.state);
  assert.equal(snapshot.outcome.resources_pending, false);
  report.batches[label] = { ticket: { ...context, task_kind: ticket.task_kind }, snapshot: { ...snapshot, logs } };
  await assert.rejects(() => invoke("run_batch_execution", { ticket }), error => error.code === "EXECUTION_GRANT_REPLAYED", "typed batch ticket must not replay after completion");
  return { ...snapshot, logs };
}
async function snapshot(table) { return invoke("read_table_design_snapshot", { id, schema: namespace, table, ownerId: owner }); }
async function operate(kind, table, target) {
  // Each DDL changes SQLite's schema generation; obtain a new baseline for every operation.
  const before = await snapshot(table);
  const capability = before.capabilities[kind];
  assert.equal(capability?.supported, true, JSON.stringify(before.capabilities));
  const operation = { connection_id: id, schema: namespace, table, kind, base_revision: before.revision, source_fingerprint: before.source_fingerprint, target_table: target ?? null };
  const preview = await invoke("preview_table_operation", { operation, ownerId: owner });
  assert.match(preview.request_sql, /^TABLE_OPERATION [a-f0-9]+$/);
  assert.deepEqual(preview.operation, operation);
  assert.ok(preview.steps.length > 0);
  const response = await invoke("execute_table_operation", { request: await grant(preview.request_sql), operation });
  assert.equal(response.outcome.state, "succeeded", JSON.stringify(response));
  assert.equal(response.outcome.resources_pending, false);
  const receipt = response.receipt;
  assert.ok(receipt, "successful table operation must have its own receipt");
  assert.equal(receipt.kind, kind);
  assert.equal(receipt.source_table, table);
  assert.equal(receipt.target_table ?? null, target ?? null);
  assert.equal(receipt.transaction, "committed");
  assert.equal(receipt.verified, true);
  assert.deepEqual(receipt.mismatches, []);
  assert.ok(receipt.steps.length > 0 && receipt.steps.every(step => step.status === "applied"));
  assert.ok(receipt.snapshot?.revision);
  report.operations[`${kind}:${table}`] = { before_revision: before.revision, preview, response };
  return receipt;
}
async function createNativeTrigger(table, name, timing, event, statement) {
  const before = await snapshot(table);
  const design = structuredClone(before.design);
  design.triggers = [...(design.triggers ?? []), { name, orig_name: null, timing, event, statement, definition: null }];
  const preview = await invoke("preview_table_design", { id, design, ownerId: owner });
  const response = await invoke("execute_table_design", { request: await grant(preview.request_sql), design });
  assert.equal(response.outcome.state, "succeeded", JSON.stringify(response));
  assert.equal(response.outcome.resources_pending, false);
  assert.equal(response.receipt.verified, true);
  report.operations[`create_trigger:${table}:${name}`] = { preview, response };
}
function parseCsv(text) {
  const records = []; let row = [], cell = "", quoted = false;
  for (let index = 0; index < text.length; index++) {
    const value = text[index];
    if (quoted) {
      if (value === '"' && text[index + 1] === '"') { cell += '"'; index++; }
      else if (value === '"') quoted = false;
      else cell += value;
    } else if (value === '"') { assert.equal(cell, ""); quoted = true; }
    else if (value === ",") { row.push(cell); cell = ""; }
    else if (value === "\n" || value === "\r") { if (value === "\r" && text[index + 1] === "\n") index++; row.push(cell); records.push(row); row = []; cell = ""; }
    else cell += value;
  }
  assert.equal(quoted, false, "CSV must close every quoted field");
  if (cell !== "" || row.length) { row.push(cell); records.push(row); }
  return records;
}
async function stopOwnedChild() {
  if (!child || child.exitCode !== null || child.signalCode !== null || !child.pid) return;
  const exited = new Promise(resolve => child.once("exit", resolve));
  child.kill();
  await Promise.race([exited, delay(5000)]);
  if (child.exitCode === null && child.signalCode === null) {
    if (process.platform === "win32") {
      const killer = spawn("taskkill", ["/PID", String(child.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore" });
      await new Promise((resolve, reject) => { killer.once("error", reject); killer.once("exit", code => code === 0 ? resolve() : reject(new Error(`owned-child taskkill exited ${code}`))); });
    } else child.kill("SIGKILL");
    await Promise.race([exited, delay(5000)]);
    assert.ok(child.exitCode !== null || child.signalCode !== null, "only this script's child must finish before cleanup returns");
  }
}

try {
  const sourceInfo = await stat(source);
  assert.ok(sourceInfo.isFile());
  const sourceHash = hash(await readFile(source));
  assert.equal(sourceHash, expectedHash, "explicit current-build executable hash must match before launch");
  const executable = path.join(directory, path.basename(source));
  await copyFile(source, executable);
  const copiedHash = hash(await readFile(executable));
  assert.equal(copiedHash, sourceHash);
  assert.equal(hash(await readFile(source)), sourceHash, "source build must not change while being copied");
  report.executable = { path: executable, sha256: copiedHash, source_mtime: sourceInfo.mtime.toISOString(), bytes: sourceInfo.size };
  const port = await freePort();
  report.debug_port = port;
  child = spawn(executable, [], { cwd: directory, windowsHide: true, env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-allow-origins=*` }, stdio: ["ignore", "pipe", "pipe"] });
  child.once("error", error => { spawnError = error; });
  child.stdout.on("data", chunk => { report.stdout = ((report.stdout || "") + chunk).slice(-100000); });
  child.stderr.on("data", chunk => { report.stderr = ((report.stderr || "") + chunk).slice(-100000); });
  const deadline = Date.now() + 45000;
  while (!browser && Date.now() < deadline) {
    if (spawnError) throw spawnError;
    assert.equal(child.exitCode, null, `native child exited before CDP was ready: ${child.exitCode}`);
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 1500 }); }
    catch { await delay(200); }
  }
  assert.ok(browser, "this script's copied native executable must expose its own WebView2 CDP endpoint");
  page = browser.contexts().flatMap(context => context.pages())[0];
  assert.ok(page);
  await page.waitForFunction(() => !!window.__APP__ && !!window.__TAURI_INTERNALS__?.invoke, null, { timeout: 30000 });
  assert.equal(await page.evaluate(() => !!window.__PREVIEW_MOCK__), false);
  const frontendHtml = await readFile(path.join(root, "dist", "index.html"), "utf8");
  const frontendAssets = [...frontendHtml.matchAll(/(?:src|href)="([^"]*assets\/[^"?#]+\.(?:js|css))"/g)].map(match => match[1].replace(/^\.?\//, ""));
  assert.ok(frontendAssets.length >= 2, "current frontend build must identify its JS and CSS assets");
  const embeddedAssets = await page.evaluate(() => [...document.querySelectorAll("script[src],link[href]")].map(node => new URL(node.src || node.href, location.href).pathname.replace(/^\//, "")).filter(value => value.startsWith("assets/")));
  report.frontend_assets = [];
  for (const asset of frontendAssets) {
    assert.ok(embeddedAssets.includes(asset), `native executable must embed the current build asset ${asset}`);
    const expected = hash(await readFile(path.join(root,"dist",asset)));
    const actual = await page.evaluate(async asset => {
      const response = await fetch(`/${asset}`); if (!response.ok) throw Error(`native asset fetch failed: ${response.status}`);
      return [...new Uint8Array(await crypto.subtle.digest("SHA-256",await response.arrayBuffer()))].map(value => value.toString(16).padStart(2,"0")).join("");
    }, asset);
    assert.equal(actual, expected, `native embedded ${asset} must match the current dist bytes`);
    report.frontend_assets.push({ asset, sha256: actual });
  }
  assert.equal(await readFile(path.join(root,"dist","index.html"),"utf8"), frontendHtml, "frontend build must remain stable during embedded asset verification");
  check("Native executable embeds the exact current frontend JS/CSS hashes, not an old frontend or preview server");
  assert.deepEqual(await invoke("list_connections"), [], "portable copy must not see any user connections");
  check("Pinned native executable, UUID portable data, real Tauri IPC, empty connection catalog, no preview mock");
  const config = { id, name: "Native database workflows isolated fixture", kind: "sqlite", host: "", port: 0, user: "", password: "", database: namespace, file: path.join(directory, "fixture.sqlite"), params: "", group: "", read_only: false, keep_alive: false };
  await invoke("save_connection", { config, password: "" });
  assert.equal((await invoke("open_connection", { id })).ok, true);
  assert.equal((await invoke("list_connections")).length, 1);

  const generatedColumns = ["name", "amount", "active", "payload"];
  for (const table of ["generated_good", "generated_rollback"]) await execute(`CREATE TABLE main.${identifier(table)}(id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL UNIQUE,amount REAL NOT NULL CHECK(amount>=0),active INTEGER NOT NULL,payload TEXT NOT NULL,twice REAL GENERATED ALWAYS AS(amount*2))`);
  const generatedRows = Array.from({ length: 25 }, (_, index) => { const i = index + 1; return [`sensitive_generated_${i}`, `${i}.25`, String(i % 2), JSON.stringify({ i })]; });
  const good = await batch("generated_25", { kind: "generated_rows", request: { connection_id: id, schema: namespace, table: "generated_good", columns: generatedColumns, rows: generatedRows } });
  assert.equal(good.status.state, "succeeded");
  assert.equal(good.transaction_state, "committed");
  assert.equal(good.scan_complete, true);
  assert.deepEqual([good.metrics.scanned, good.metrics.executed, good.metrics.errors, good.affected_rows], [25, 25, 0, 25]);
  const expectedRows = generatedRows.map((row, index) => [String(index + 1), row[0], row[1], row[2], row[3], ((index + 1.25) * 2).toFixed(2)]);
  assert.deepEqual(await query("SELECT id,name,printf('%.2f',amount),active,payload,printf('%.2f',twice) FROM main.generated_good ORDER BY id"), expectedRows);
  assert.deepEqual(await query("SELECT count(*),count(DISTINCT id),printf('%.2f',sum(amount)),printf('%.2f',sum(twice)) FROM main.generated_good"), [["25", "25", "331.25", "662.50"]]);
  assert.ok(good.logs.some(line => line.sql_template?.includes("?") && /INSERT/i.test(line.sql_template)));
  assert.equal(JSON.stringify(good.logs).includes("sensitive_generated_"), false, "batch logs must not publish bound values");
  const invalidRows = generatedRows.map(row => [...row]); invalidRows[5][1] = "-1";
  const bad = await batch("generated_rollback", { kind: "generated_rows", request: { connection_id: id, schema: namespace, table: "generated_rollback", columns: generatedColumns, rows: invalidRows } });
  assert.equal(bad.status.state, "failed");
  assert.equal(bad.transaction_state, "rolled_back");
  assert.equal(bad.outcome.effects, "no_effect");
  assert.deepEqual([bad.metrics.scanned, bad.metrics.executed, bad.metrics.errors, bad.affected_rows], [25, 6, 1, 0]);
  assert.equal(await scalar("SELECT count(*) FROM main.generated_rollback"), "0");
  check("Typed generated-row batch inserts 25 exact bound rows; sixth-row CHECK failure rolls back the entire independent batch");

  await execute("CREATE TABLE main.generated_cancel(id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL UNIQUE,amount REAL NOT NULL CHECK(amount>=0),active INTEGER NOT NULL,payload TEXT NOT NULL)");
  await createNativeTrigger("generated_cancel", "generated_cancel_slow", "BEFORE", "INSERT", "SELECT (WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n)");
  let generatedStop;
  const cancelledRows = await batch("generated_real_vm_cancel", { kind: "generated_rows", request: { connection_id: id, schema: namespace, table: "generated_cancel", columns: generatedColumns, rows: generatedRows } }, async (current, context) => {
    if (!generatedStop && current.transaction_state === "active" && current.metrics.scanned === 25) {
      await delay(80);
      generatedStop = await invoke("cancel_execution", { executionId: context.execution_id, connectionId: id, ownerId: owner, namespace });
      assert.equal(generatedStop.accepted, true, "current generated VM must accept a real stop request");
      report.batches.generated_cancel_ack = generatedStop;
    }
  });
  assert.ok(generatedStop, "cancel must be requested during the active real generated transaction");
  assert.equal(cancelledRows.status.state, "cancelled", JSON.stringify(cancelledRows));
  assert.equal(cancelledRows.transaction_state, "rolled_back");
  assert.equal(cancelledRows.outcome.effects, "no_effect");
  assert.equal(cancelledRows.affected_rows, 0);
  assert.equal(await scalar("SELECT count(*) FROM main.generated_cancel"), "0");
  check("Real generated-row VM cancellation settles resources and confirms whole-batch rollback with zero durable rows");

  const runFile = async (name, sql) => {
    const file = path.join(directory, `${name}.sql`); await writeFile(file, sql, "utf8");
    return batch(name, { kind: "sql_file", path: file, encoding: "utf-8", stop_on_error: true });
  };
  const fileGood = await runFile("file_success", "CREATE TABLE main.file_rows(id INTEGER PRIMARY KEY,value TEXT NOT NULL);\nINSERT INTO main.file_rows VALUES(1,'prefix 中文; literal');\nINSERT INTO main.file_rows VALUES(2,'second');\n");
  assert.equal(fileGood.status.state, "succeeded");
  assert.equal(fileGood.scan_complete, true);
  assert.deepEqual([fileGood.metrics.scanned, fileGood.metrics.executed, fileGood.metrics.errors, fileGood.affected_rows], [3, 3, 0, 2]);
  assert.deepEqual(await query("SELECT id,value FROM main.file_rows ORDER BY id"), [["1", "prefix 中文; literal"], ["2", "second"]]);
  const fileFailed = await runFile("file_error_prefix", "INSERT INTO main.file_rows VALUES(3,'committed-prefix');\nINSERT INTO main.file_rows VALUES(3,'duplicate-must-fail');\nINSERT INTO main.file_rows VALUES(4,'must-not-be-sent');\n");
  assert.equal(fileFailed.status.state, "failed");
  assert.equal(fileFailed.transaction_state, "none");
  assert.deepEqual([fileFailed.metrics.executed, fileFailed.metrics.errors, fileFailed.affected_rows], [2, 1, 1]);
  assert.ok(fileFailed.metrics.scanned >= 2 && fileFailed.metrics.scanned <= 3);
  assert.deepEqual(await query("SELECT id,value FROM main.file_rows ORDER BY id"), [["1", "prefix 中文; literal"], ["2", "second"], ["3", "committed-prefix"]]);
  const fileParse = await runFile("file_parse_prefix", "INSERT INTO main.file_rows VALUES(4,'before-parse-error');\nSELECT 'unterminated");
  assert.equal(fileParse.status.state, "failed");
  assert.ok(fileParse.parse_error, "scanner failure must be reported separately from execution errors");
  assert.deepEqual([fileParse.metrics.scanned, fileParse.metrics.executed, fileParse.metrics.errors, fileParse.affected_rows], [1, 1, 0, 1]);
  assert.equal(await scalar("SELECT value FROM main.file_rows WHERE id=4"), "before-parse-error");
  check("Real SQL files report scanned/executed/errors/elapsed_ms; success, acknowledged error, and parse error preserve the exact committed prefix without replay");

  await execute("CREATE TABLE main.file_cancel_rows(id INTEGER PRIMARY KEY,value TEXT NOT NULL)");
  const cancelFile = path.join(directory, "streaming_real_cancel.sql");
  const neverSentTail = Array.from({ length: 9999 }, (_, index) => `INSERT INTO main.file_cancel_rows VALUES(${index + 2},'never-sent');`).join("\n");
  await writeFile(cancelFile, "INSERT INTO main.file_cancel_rows VALUES(1,'committed-prefix');\nWITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n;\n" + neverSentTail, "utf8");
  let fileStop;
  const stoppedFile = await batch("file_streaming_real_cancel", { kind: "sql_file", path: cancelFile, encoding: "utf-8", stop_on_error: true }, async (current, context) => {
    if (!fileStop && current.metrics.executed >= 1 && !current.outcome) {
      assert.equal(current.scan_complete, false, "first real write must execute before the entire large file is scanned");
      assert.ok(current.metrics.scanned < 10001, "scanner must remain bounded, not prescan the whole file");
      await delay(80);
      fileStop = await invoke("cancel_execution", { executionId: context.execution_id, connectionId: id, ownerId: owner, namespace });
      assert.equal(fileStop.accepted, true);
      report.batches.file_cancel_ack = fileStop;
    }
  });
  assert.ok(fileStop, "stop must be requested while the real SQL file is running");
  assert.equal(stoppedFile.status.state, "cancelled", JSON.stringify(stoppedFile));
  assert.equal(stoppedFile.transaction_state, "none");
  assert.equal(stoppedFile.scan_complete, false);
  assert.ok(stoppedFile.metrics.scanned < 10001);
  assert.equal(stoppedFile.affected_rows, 1);
  assert.deepEqual(await query("SELECT id,value FROM main.file_cancel_rows ORDER BY id"), [["1", "committed-prefix"]]);
  check("Large SQL file starts real writes before scan completion, cancels the active VM, preserves only the committed prefix, and never replays the unscanned tail");

  await execute("CREATE TABLE main.workflow_parent(id INTEGER PRIMARY KEY); INSERT INTO main.workflow_parent VALUES(1); CREATE TABLE main.workflow_audit(id INTEGER PRIMARY KEY AUTOINCREMENT,observed_id INTEGER NOT NULL)");
  await execute("CREATE TABLE main.workflow_source(id INTEGER PRIMARY KEY AUTOINCREMENT,parent_id INTEGER NOT NULL REFERENCES workflow_parent(id),amount REAL NOT NULL CONSTRAINT positive_amount CHECK(amount>=0),label TEXT NOT NULL DEFAULT 'fixture 默认',twice REAL GENERATED ALWAYS AS(amount*2)); CREATE INDEX workflow_amount_idx ON workflow_source(amount)");
  await createNativeTrigger("workflow_source", "workflow_source_audit", "AFTER", "INSERT", "INSERT INTO workflow_audit(observed_id) VALUES(NEW.id)");
  await execute("INSERT INTO workflow_source(parent_id,amount,label) VALUES(1,1.25,'one'),(1,2.5,'two'); INSERT INTO workflow_source(id,parent_id,amount) VALUES(50,1,9); DELETE FROM workflow_source WHERE id=50");
  const sourceDefinition = await query("SELECT type,name,tbl_name,sql FROM main.sqlite_schema WHERE tbl_name='workflow_source' ORDER BY type,name");
  const sourceRows = await query("SELECT id,parent_id,printf('%.2f',amount),label,printf('%.2f',twice) FROM main.workflow_source ORDER BY id");
  assert.equal(await scalar("SELECT seq FROM main.sqlite_sequence WHERE name='workflow_source'"), "50");
  assert.equal(await scalar("SELECT count(*) FROM main.workflow_audit"), "3");
  const structure = "workflow_structure_副本", copied = "workflow_data_副本";
  const structureReceipt = await operate("copy_structure", "workflow_source", structure);
  assert.equal(structureReceipt.copied_rows, 0);
  assert.equal(await scalar(`SELECT count(*) FROM main.${identifier(structure)}`), "0");
  const copyReceipt = await operate("copy_data", "workflow_source", copied);
  assert.equal(copyReceipt.copied_rows, 2);
  assert.deepEqual(await query(`SELECT id,parent_id,printf('%.2f',amount),label,printf('%.2f',twice) FROM main.${identifier(copied)} ORDER BY id`), sourceRows);
  assert.equal(await scalar(`SELECT seq FROM main.sqlite_sequence WHERE name=${literal(copied)}`), "50", "copy must preserve advanced AUTOINCREMENT sequence, not only max(id)");
  assert.equal(await scalar("SELECT count(*) FROM main.workflow_audit"), "3", "copying data must not run the newly installed target INSERT trigger");
  for (const table of [structure, copied]) {
    assert.equal(await scalar(`SELECT count(*) FROM pragma_foreign_key_list(${literal(table)}, 'main')`), "1");
    assert.equal(await scalar(`SELECT count(*) FROM main.sqlite_schema WHERE type='index' AND tbl_name=${literal(table)} AND sql IS NOT NULL`), "1");
    assert.equal(await scalar(`SELECT count(*) FROM main.sqlite_schema WHERE type='trigger' AND tbl_name=${literal(table)}`), "1");
    const tableSql = await scalar(`SELECT sql FROM main.sqlite_schema WHERE type='table' AND name=${literal(table)}`);
    for (const feature of [/AUTOINCREMENT/i, /CHECK\s*\(/i, /DEFAULT\s+'fixture 默认'/i, /GENERATED\s+ALWAYS/i, /REFERENCES\s+workflow_parent/i]) assert.match(tableSql, feature);
    const beforeCount = await scalar(`SELECT count(*) FROM main.${identifier(table)}`);
    const invalid = await sqlOutcome(`INSERT INTO main.${identifier(table)}(parent_id,amount) VALUES(1,-1)`);
    assert.equal(invalid.state, "failed", "copied CHECK must reject an actual invalid write");
    assert.equal(await scalar(`SELECT count(*) FROM main.${identifier(table)}`), beforeCount);
    const invalidFk = await sqlOutcome(`INSERT INTO main.${identifier(table)}(parent_id,amount) VALUES(999,1)`);
    assert.equal(invalidFk.state, "failed", "copied FK must reject an actual invalid write");
    assert.equal(await scalar(`SELECT count(*) FROM main.${identifier(table)}`), beforeCount);
    await execute(`INSERT INTO main.${identifier(table)}(parent_id,amount) VALUES(1,7.5)`);
    const expectedId = table === copied ? "51" : "1";
    assert.deepEqual(await query(`SELECT id,label,printf('%.2f',twice) FROM main.${identifier(table)} ORDER BY id DESC LIMIT 1`), [[expectedId, "fixture 默认", "15.00"]]);
  }
  assert.equal(await scalar("SELECT count(*) FROM main.workflow_audit"), "5", "each copied target's real trigger must fire once after installation");
  const copiedDefinition = await query(`SELECT type,name,tbl_name,sql FROM main.sqlite_schema WHERE tbl_name=${literal(copied)} ORDER BY type,name`);
  const clearReceipt = await operate("clear", copied);
  assert.equal(clearReceipt.affected_rows, 3);
  assert.equal(await scalar(`SELECT count(*) FROM main.${identifier(copied)}`), "0");
  assert.deepEqual(await query(`SELECT type,name,tbl_name,sql FROM main.sqlite_schema WHERE tbl_name=${literal(copied)} ORDER BY type,name`), copiedDefinition);
  assert.equal(await scalar(`SELECT seq FROM main.sqlite_sequence WHERE name=${literal(copied)}`), "51", "clear uses DELETE and must retain AUTOINCREMENT state");
  assert.deepEqual(await query("SELECT type,name,tbl_name,sql FROM main.sqlite_schema WHERE tbl_name='workflow_source' ORDER BY type,name"), sourceDefinition);
  assert.deepEqual(await query("SELECT id,parent_id,printf('%.2f',amount),label,printf('%.2f',twice) FROM main.workflow_source ORDER BY id"), sourceRows);
  assert.equal(await scalar("SELECT count(*) FROM pragma_foreign_key_check"), "0");
  check("Fresh snapshot/preview/C1/execute clears and copies complete SQLite definitions, rows, generated values, FK, CHECK, index, trigger, advanced sequence, and verified committed receipts");

  await execute("CREATE TABLE main.design_parent(id INTEGER PRIMARY KEY); INSERT INTO main.design_parent VALUES(1); CREATE TABLE main.design_ordered(id INT NOT NULL,x TEXT,g TEXT GENERATED ALWAYS AS(x||' literal') STORED,PRIMARY KEY(id),CONSTRAINT kept_unique UNIQUE(x),FOREIGN KEY(id) REFERENCES design_parent(id)); INSERT INTO main.design_ordered(rowid,id,x) VALUES(42,1,'kept')");
  const designBefore = await snapshot("design_ordered");
  const design = structuredClone(designBefore.design);
  const newColumn = { ...design.columns[1], name: "inserted", orig_name: null, default_value: "'new default'" };
  design.columns.splice(1, 0, newColumn);
  design.checks = [{ name: "positive", orig_name: null, expression: "id>0", generated_name: false }];
  const designPreview = await invoke("preview_table_design", { id, design, ownerId: owner });
  assert.ok(designPreview.steps[0].sql.indexOf('"inserted"') < designPreview.steps[0].sql.indexOf("PRIMARY KEY"));
  const designResponse = await invoke("execute_table_design", { request: await grant(designPreview.request_sql), design });
  assert.equal(designResponse.outcome.state, "succeeded", JSON.stringify(designResponse));
  assert.equal(designResponse.outcome.resources_pending, false);
  assert.equal(designResponse.receipt.verified, true);
  assert.deepEqual(designResponse.receipt.snapshot.schema.columns.map(column => column.name), ["id", "inserted", "x", "g"]);
  assert.deepEqual(await query("SELECT rowid,id,inserted,x,g FROM main.design_ordered"), [["42", "1", "new default", "kept", "kept literal"]]);
  assert.equal(designResponse.receipt.snapshot.schema.foreign_keys.length, 1);
  assert.equal((await sqlOutcome("INSERT INTO main.design_ordered(id,x) VALUES(-1,'bad')")).state, "failed");
  assert.equal((await sqlOutcome("INSERT INTO main.design_ordered(id,x) VALUES(2,'kept')")).state, "failed");
  report.operations.ordered_design = { before_revision: designBefore.revision, preview: designPreview, response: designResponse };
  check("Production table-design preview/C1/execute inserts columns before table constraints and reads back preserved rows, rowid, generated values, FK, UNIQUE and new CHECK");

  await execute("CREATE TABLE main.sensitive_ranges(x TEXT COLLATE NOCASE,n INTEGER); INSERT INTO main.sensitive_ranges VALUES('B',2),('b',10),('a',1)");
  report.range_filters = [];
  for (const [operator, expected] of [["<", ["B"]], ["<=", ["B", "a"]], [">", ["b"]], [">=", ["a", "b"]]]) {
    const req = { connection_id: id, schema: namespace, table: "sensitive_ranges", offset: 0, limit: 100, filters: [{ conds: [{ column: "x", operator, value: "a", case_sensitive: true }] }] };
    const result = await invoke("page_table", { req });
    assert.deepEqual(result.rows.map(row => row[0]).sort(), expected, JSON.stringify({ req, result }));
    report.range_filters.push({ operator, rows: result.rows });
  }
  assert.equal(await scalar("SELECT count(*) FROM main.sensitive_ranges WHERE x<'a'"), "0", "sensitive paging must not change the table's original NOCASE semantics");
  check("Real page_table IPC applies case-sensitive text < <= > >= while preserving the original NOCASE table collation");

  const exportRows = [["1", "comma,quote\"中文"], ["2", "line one\nline two"]];
  await execute("CREATE TABLE main.export_table(id INTEGER PRIMARY KEY,value TEXT NOT NULL)");
  for (const row of exportRows) await execute(`INSERT INTO main.export_table VALUES(${row[0]},${literal(row[1])})`);
  await execute("CREATE VIEW main.export_view AS SELECT id,value FROM main.export_table WHERE id=2");
  const outputDir = path.join(directory, "exports"); await mkdir(outputDir);
  for (const format of ["csv", "json"]) {
    const request = { connection_id: id, namespace, objects: [{ name: "export_table", object_type: "table" }, { name: "export_view", object_type: "view" }], format, destination: { mode: "files", base_file: path.join(outputDir, `workflow.${format}`) }, encoding: "utf-8", delimiter: ",", has_header: true, task_id: `native-export-${randomUUID()}` };
    const result = await invoke("run_batch_export", { request });
    assert.equal(result.items.length, 2);
    const fileChecks = [];
    for (let index = 0; index < result.items.length; index++) {
      const item = result.items[index], expected = index === 0 ? exportRows : [exportRows[1]];
      assert.deepEqual(item.object, request.objects[index]);
      assert.equal(item.status, "completed", JSON.stringify(item));
      assert.equal(item.error, null);
      assert.equal(item.rows, expected.length);
      assert.ok(path.isAbsolute(item.file));
      assert.equal(path.dirname(item.file), outputDir, "export must stay in this run's own UUID directory");
      const bytes = await readFile(item.file), text = bytes.toString("utf8").replace(/^\uFEFF/, "");
      assert.ok(bytes.length > 0);
      if (format === "csv") assert.deepEqual(parseCsv(text), [["id", "value"], ...expected]);
      else assert.deepEqual(JSON.parse(text), expected.map(([id, value]) => ({ id, value })));
      fileChecks.push({ file: item.file, bytes: bytes.length, sha256: hash(bytes), rows: expected.length });
    }
    report.exports[format] = { result, files: fileChecks };
  }
  check("Native batchExport writes and reads back CSV/JSON files for a real table and view, including quotes, delimiters, Unicode, and embedded newlines");

  await execute("CREATE TABLE main.native_ui_edit(id INTEGER PRIMARY KEY,name TEXT,d DATE,t TIME,stamp DATETIME); INSERT INTO main.native_ui_edit VALUES(1,'before','2026-11-11','08:00:01.123456','2026-11-11 08:00:01.654321')");
  // Reload reads the connection just saved through real IPC into the real App.
  // Do not inject a catalog, replace a transport, or fabricate edit callbacks.
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.__APP__?.connections?.length === 1, null, { timeout: 30000 });
  await page.evaluate(async ({ id, namespace }) => window.__APP__.openObjectsTab(id, namespace, "folder"), { id, namespace });
  await page.waitForFunction(() => document.querySelector(".object-browser")?.getAttribute("aria-busy") === "false", null, { timeout: 30000 });
  assert.equal(await page.evaluate(() => window.__APP__.activeTab.selectedRows.size), 0, "objects export must start without selecting any table");
  await page.locator('[data-command="object-search"]').click();
  const objectSearch = page.getByRole("searchbox", { name: "筛选对象名称（Esc 清空并收起）" });
  await objectSearch.fill("export_table");
  await page.waitForFunction(() => window.__APP__.activeView().filteredCount() === 1);
  await objectSearch.press("Escape");
  await page.waitForFunction(() => window.__APP__.activeView().filteredCount() > 1);
  await page.locator('[data-command="object-export"]').click();
  const exportDialog = page.getByRole("dialog", { name: "批量导出表与视图", exact: true });
  await exportDialog.getByText("真实表与视图清单已加载。", { exact: true }).waitFor();
  const tableChoice = exportDialog.getByRole("checkbox", { name: /\bexport_table\b/ });
  const viewChoice = exportDialog.getByRole("checkbox", { name: /\bexport_view\b/ });
  assert.equal(await tableChoice.isChecked(), false); assert.equal(await viewChoice.isChecked(), false);
  await tableChoice.check(); await viewChoice.check();
  await exportDialog.getByRole("button", { name: /下一步/ }).click();
  const exportPath = exportDialog.getByLabel("批量导出完整保存路径");
  await page.waitForFunction(() => /^(?:[A-Za-z]:[\\/]|\\\\|\/)/.test(document.querySelector('[aria-label="批量导出完整保存路径"]')?.value || ""));
  const defaultExportPath = await exportPath.inputValue(); assert.ok(path.isAbsolute(defaultExportPath)); assert.equal(await exportPath.getAttribute("title"), defaultExportPath);
  await exportPath.fill(path.join(outputDir,"native_ui_batch.csv"));
  await exportDialog.getByRole("button", { name: /下一步/ }).click();
  await exportDialog.getByRole("button", { name: "开始导出", exact: true }).click();
  await exportDialog.locator('[aria-label="逐对象导出结果"]').getByText(/^export_table：完成/).waitFor();
  await exportDialog.locator('[aria-label="逐对象导出结果"]').getByText(/^export_view：完成/).waitFor();
  const uiExportFiles = [];
  for (const [index, name, expected] of [[1,"export_table",exportRows],[2,"export_view",[exportRows[1]]]]) {
    const file = path.join(outputDir,`native_ui_batch_${String(index).padStart(3,"0")}_${name}.csv`);
    const bytes = await readFile(file); assert.deepEqual(parseCsv(bytes.toString("utf8").replace(/^\uFEFF/,"")), [["id","value"],...expected]);
    uiExportFiles.push({ file, sha256: hash(bytes), rows: expected.length });
  }
  report.native_ui_export = { default_path: defaultExportPath, files: uiExportFiles, transport: "real objects search/empty-selection wizard + real Tauri IPC + real CSV readback" };
  await exportDialog.locator(".dialog-footer").getByRole("button", { name: "关闭", exact: true }).click();
  check("Real objects-page search opens and clears; export opens with no selection, offers table/view choices, shows a full default path and writes both exact CSV files");
  await page.evaluate(async ({ id, namespace }) => window.__APP__.openTableTab(id, namespace, "native_ui_edit", "table"), { id, namespace });
  await page.waitForFunction(() => window.__APP__.activeView()?.grid?.rows?.length === 1 && !window.__APP__.activeView().loading, null, { timeout: 30000 });
  const nativeCell = column => page.locator(`.grid-table tbody tr[data-row="0"] td[data-col="${column}"]`);
  await nativeCell(1).dblclick();
  let editor = page.locator(".grid-cell-input");
  await editor.waitFor({ state: "visible" });
  await editor.fill("after 中文");
  await editor.press("Enter");
  await page.waitForFunction(() => window.__APP__.activeView().pendingUpdates.size === 1);
  await page.evaluate(async () => window.__APP__.runCommand("save"));
  await page.waitForFunction(() => window.__APP__.activeView().pendingUpdates.size === 0 && !window.__APP__.activeView().loading);
  assert.equal(await scalar("SELECT name FROM main.native_ui_edit WHERE id=1"), "after 中文", "real double-click and App save must change SQLite, not only the grid draft");
  for (const [column, pickerType, picked, expected] of [[2,"date","2026-12-25","2026-12-25"],[3,"time","14:15:16","14:15:16.123456"],[4,"datetime-local","2026-12-25T14:15:16","2026-12-25 14:15:16.654321"]]) {
    await nativeCell(column).dblclick();
    editor = page.locator(".grid-cell-input");
    const picker = page.locator(".grid-cell-picker");
    await editor.waitFor({ state: "visible" });
    assert.equal(await picker.getAttribute("type"), pickerType);
    await picker.fill(picked);
    assert.equal(await editor.inputValue(), expected, "native picker must retain original fractional precision and datetime separator");
    await editor.press("Enter");
  }
  await page.evaluate(async () => window.__APP__.runCommand("save"));
  await page.waitForFunction(() => window.__APP__.activeView().pendingUpdates.size === 0 && !window.__APP__.activeView().loading);
  const finalNativeRows = await query("SELECT id,name,d,t,stamp FROM main.native_ui_edit");
  assert.deepEqual(finalNativeRows, [["1","after 中文","2026-12-25","14:15:16.123456","2026-12-25 14:15:16.654321"]]);
  report.native_ui_edit = { rows: finalNativeRows, screenshot: path.join(directory,"native-ui-edit.png"), transport: "real App UI + real Tauri IPC + actual SQLite readback" };
  await page.screenshot({ path: report.native_ui_edit.screenshot });
  check("Real desktop double-click edit/App save and date/time/datetime pickers update SQLite while retaining fractional seconds");
  report.passed = true;
} catch (error) {
  failure = error;
  report.passed = false;
  report.error = errorInfo(error);
  if (page && !page.isClosed()) {
    report.ui_diagnostic = await page.evaluate(() => ({ dialogs: [...document.querySelectorAll('[role="dialog"]')].map(el => el.textContent), active: window.__APP__?.activeTab, grid: window.__APP__?.activeView()?.grid?.rows, body: document.body.innerText.slice(-6000) })).catch(error => errorInfo(error));
    await page.screenshot({ path: path.join(directory,"failure.png"), timeout: 3000 }).catch(() => {});
  }
} finally {
  if (browser) try { await browser.close(); } catch (error) { report.cleanup_errors.push(errorInfo(error)); }
  try { await stopOwnedChild(); } catch (error) { report.cleanup_errors.push(errorInfo(error)); }
  if (child) report.child = { pid: child.pid, exit_code: child.exitCode, signal: child.signalCode };
  report.finished_at = new Date().toISOString();
  if (report.cleanup_errors.length) { report.passed = false; failure ||= new Error("Native run completed with owned-child cleanup errors; see report"); }
  await writeFile(path.join(directory, "report.json"), `${JSON.stringify(report, null, 2)}\n`, "utf8");
  console.log(`Native workflow report: ${path.join(directory, "report.json")}`);
}
if (failure) throw failure;
console.log(`${report.checks.length} native workflow groups passed`);
