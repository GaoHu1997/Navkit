import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { build } from "esbuild";
import ts from "typescript";
import vm from "node:vm";
const output = await build({ entryPoints: ["src/core/datasourceSync.ts"], bundle: true, platform: "node", format: "esm", write: false });
const { DatasourceSync, invalidatedDatasourceIds } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString("base64")}`);
const config = () => ({ url: "https://example.test/dav", user: "test", auto_every: -1, remember_password: true, password_set: true, passphrase_set: true, headers_set: false, last_sync: null });
const plan = () => ({ plan_id: "plan-1", conflicts: [], summary: { added: 1, updated: 0, deleted: 0, organization: false }, warnings: [], remote_exists: false, has_changes: true, requires_confirmation: false });
const result = () => ({ connections: [], groups: [], changed_ids: [], deleted_ids: [], warnings: [], last_sync: "2026-11-06T10:00:00Z" });
function fixture(overrides = {}, ready = () => true) {
  const calls = [], statuses = [], applied = [], timers = new Map(); let serial = 0;
  const source = {
    getConfig: async () => config(),
    setConfig: async c => { calls.push(["save", c]); return { ...config(), ...c }; },
    test: async () => "OK",
    preview: async () => { calls.push(["preview"]); return plan(); },
    commit: async (id, choices, automatic) => { calls.push(["commit", id, choices, automatic]); return result(); },
    cancel: async id => { calls.push(["cancel", id]); }, ...overrides,
  };
  const sync = new DatasourceSync(source, { ready, applied: r => applied.push(r), status: (...s) => statuses.push(s) }, { set: (cb, ms) => { const id = ++serial; timers.set(id, { cb, ms }); return id; }, clear: id => timers.delete(id) });
  return { sync, calls, statuses, applied, timers };
}
test("partial local apply invalidates pages but never records success, in manual and automatic paths", async () => {
  for (const automatic of [false, true]) {
    const partial = { ...result(), last_sync: "", recovery_required: true, warnings: ["基线保存失败，请恢复"] };
    const f = fixture({ commit: async () => partial }); await f.sync.start();
    if (automatic) await f.sync.automatic();
    else { const p = await f.sync.preview(); await f.sync.commit(p.plan_id, {}); }
    assert.equal(f.applied.length, 1); assert.equal(f.applied[0], partial);
    assert.equal(f.sync.config.last_sync, null);
    assert.match(f.statuses.at(-1)[0], /尚未完成.*恢复/);
    assert.equal(f.statuses.at(-1)[1], true);
  }
});
test("manual default and unsupported intervals create no timer", async () => {
  for (const n of [-1, 0, 1, 7]) { const f = fixture({ getConfig: async () => ({ ...config(), auto_every: n }) }); await f.sync.start(); assert.equal(f.timers.size, 0); }
});
test("automatic interval starts after cadence, never during startup", async () => {
  const f = fixture({ getConfig: async () => ({ ...config(), auto_every: 15 }) }); await f.sync.start(); assert.equal(f.calls.length, 0); assert.equal([...f.timers.values()][0].ms, 900000); f.sync.stop(); assert.equal(f.timers.size, 0);
});
test("nested pause is idempotent and cancels timers until all dialogs release", async () => {
  const f = fixture({ getConfig: async () => ({ ...config(), auto_every: 3 }) }); await f.sync.start(); const a = f.sync.pause(), b = f.sync.pause(); assert.equal(f.timers.size, 0); a(); a(); assert.equal(f.timers.size, 0); await f.sync.automatic(); assert.equal(f.calls.length, 0); b(); assert.equal(f.timers.size, 1);
});
test("automatic clean merge uses backend automatic flag and applies returned snapshot", async () => {
  const f = fixture(); await f.sync.start(); await f.sync.automatic(); assert.deepEqual(f.calls.map(c => c[0]), ["preview", "commit"]); assert.equal(f.calls[1][3], true); assert.equal(f.applied.length, 1); assert.equal(f.sync.config.last_sync, result().last_sync);
});
test("automatic never resolves conflicts or takes a confirmation-required plan", async () => {
  for (const p of [{ ...plan(), requires_confirmation: true }, { ...plan(), conflicts: [{ key: "id", label: "conflict", local_summary: "a", remote_summary: "b" }] }]) {
    const f = fixture({ preview: async () => p }); await f.sync.start(); await f.sync.automatic(); assert.deepEqual(f.calls, [["cancel", "plan-1"]]); assert.equal(f.applied.length, 0); assert.match(f.statuses.at(-1)[0], /手动确认/);
  }
});
test("active work postpones automatic without reading remote", async () => {
  const f = fixture({}, () => false); await f.sync.start(); await f.sync.automatic(); assert.equal(f.calls.length, 0); assert.match(f.statuses.at(-1)[0], /延后/);
});
test("activity beginning during preview cancels instead of committing", async () => {
  let ready = true; const f = fixture({ preview: async () => { ready = false; return plan(); } }, () => ready); await f.sync.start(); await f.sync.automatic(); assert.deepEqual(f.calls, [["cancel", "plan-1"]]);
});
test("manual preview is never taken by automatic and cancellation never applies", async () => {
  const f = fixture(); await f.sync.start(); await f.sync.preview(); await f.sync.automatic(); await f.sync.cancel(); assert.deepEqual(f.calls.map(c => c[0]), ["preview", "cancel"]); assert.equal(f.applied.length, 0);
});
test("all and only exact conflict keys are required, with no default selection", async () => {
  const f = fixture({ preview: async () => ({ ...plan(), conflicts: [{ key: "connection:id", label: "x", local_summary: "a", remote_summary: "b" }] }) });
  const p = await f.sync.preview(); await assert.rejects(f.sync.commit(p.plan_id, {}), /明确选择/); await assert.rejects(f.sync.commit(p.plan_id, { "connection:id": "remote", extra: "local" }), /明确选择/); assert.equal(f.applied.length, 0);
  await f.sync.commit(p.plan_id, { "connection:id": "remote" }); assert.equal(f.calls.at(-1)[3], false);
});
test("commit is refused while local unsaved non-SQL edits remain", async () => {
  const f = fixture({}, () => false); const p = await f.sync.preview(); await assert.rejects(f.sync.commit(p.plan_id, {}), /未提交/); assert.equal(f.calls.filter(c => c[0] === "commit").length, 0);
});
test("failed consumed commit cannot be reused and never applies local result", async () => {
  const f = fixture({ commit: async () => { throw new Error("412 remote changed"); } }); const p = await f.sync.preview(); await assert.rejects(f.sync.commit(p.plan_id, {}), /412/); await assert.rejects(f.sync.commit(p.plan_id, {}), /预览已失效/); assert.equal(f.applied.length, 0); assert.equal(f.sync.busy, false);
});
test("shared lock rejects reentrant preview and save", async () => {
  let finish; const pending = new Promise(resolve => finish = resolve); const f = fixture({ preview: () => pending }); const p = f.sync.preview(); await assert.rejects(f.sync.preview(), /正在执行/); await assert.rejects(f.sync.save(config()), /正在执行/); finish(plan()); await p; await f.sync.cancel();
});
test("new preview and changed config cancel old backend secret-bearing plan", async () => {
  const f = fixture(); await f.sync.preview(); await f.sync.preview(); await f.sync.save({ url: "https://next.test", user: "u", auto_every: -1, remember_password: false, headers: "" }); assert.deepEqual(f.calls.map(c => c[0]), ["preview", "cancel", "preview", "cancel", "save"]);
});
test("automatic errors are generic, do not leak remote or auth details and keep old state", async () => {
  const f = fixture({ preview: async () => { throw new Error("Authorization secret-token"); } }); await f.sync.start(); await f.sync.automatic(); assert.equal(f.applied.length, 0); assert(!JSON.stringify(f.statuses).includes("secret-token")); assert.equal(f.statuses.at(-1)[1], true);
});
test("organization only preserves runtime pools; real connection changes invalidate", () => {
  const c = { id: "a", name: "old", group: "g", kind: "mysql", host: "old", port: 3306, user: "u", password: "ciphertext", database: "db", file: "", params: "", read_only: false, keep_alive: true };
  assert.equal(invalidatedDatasourceIds([c], [{ ...c, name: "new", group: "other" }]).size, 0);
  for (const change of [{ host: "new" }, { password: "newCipher" }, { read_only: true }, { params: "new" }, { keep_alive: false }]) assert.deepEqual([...invalidatedDatasourceIds([c], [{ ...c, ...change }])], ["a"]);
  assert.deepEqual([...invalidatedDatasourceIds([c], [])], ["a"]);
});
test("IPC and dialog are separate from SQL/backup synchronization and never prefill secrets", async () => {
  const api = await readFile("src/core/api.ts", "utf8"), tab = await readFile("src/dialogs/DatasourceSyncTab.ts", "utf8"), app = await readFile("src/core/App.ts", "utf8");
  assert.match(api, /datasource_sync_commit.*planId, choices, automatic/);
  assert.match(tab, /password\.value = passphrase\.value = headers\.value = ""/);
  assert.match(tab, /已设置，留空保留/);
  assert(!tab.includes("relayEnabled"));
  // 同步 tab 仅连接配置：无预览/合并/测试按钮，无说明文字与清除复选框；秘密始终本机加密保存。
  assert(!tab.includes("清除已保存"));
  assert(!tab.includes("remember.checked"));
  assert(!tab.includes("预览同步"));
  assert(!tab.includes("确认合并"));
  assert(!tab.includes("测试连接"));
  assert(!tab.includes("不同步"));
  assert.match(app, /if \(!tab\.datasourceStale\) view\.activate\(\)/);
  assert.match(app, /async execute\(sql: string, _selectionOnly: boolean\): Promise<void> \{\s*if \(this\.tab\.datasourceStale\)/);
  // 菜单栏独立入口已移除：WebDAV 同步配置只在「设置」对话框的对应选项卡内。
  assert.ok(!app.includes('"datasource-sync"'));
});

const appSource = await readFile("src/core/App.ts", "utf8");
const parsedApp = ts.createSourceFile("App.ts", appSource, ts.ScriptTarget.ES2021, true);
function methods(className, names) {
  const cls = parsedApp.statements.find(node => ts.isClassDeclaration(node) && node.name?.text === className);
  return names.map(name => cls.members.find(node => ts.isMethodDeclaration(node) && node.name.getText(parsedApp) === name).getText(parsedApp)).join("\n");
}
class Element {
  children = []; parent = null; inert = false;
  get firstChild() { return this.children[0] ?? null; }
  append(...nodes) {
    for (const node of nodes) {
      if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1);
      this.children.push(node); node.parent = this;
    }
  }
  setAttribute() {}
}
function appFixture() {
  class Query { el = new Element(); sql = "SELECT '未保存的 SQL'"; invalidated = 0; invalidateDatasource() { this.invalidated++; } }
  const context = vm.createContext({ invalidatedDatasourceIds, QueryTabView: Query, document: { createElement: () => new Element() } });
  vm.runInContext(ts.transpileModule(`class SyncApp { ${methods("App", ["applyDatasourceSync"])} } globalThis.SyncApp = SyncApp;`, { compilerOptions: { target: ts.ScriptTarget.ES2021 } }).outputText, context);
  const c = id => ({ id, name: id, group: "g", kind: "mysql", host: "old", port: 3306, user: "u", password: "cipher", database: "db", file: "", params: "", read_only: false, keep_alive: true });
  const query = new Query(), data = { el: new Element() }; const draft = new Element(); data.el.append(draft);
  const treeA = { id: "conn:a", type: "connection", children: [{ id: "db:old", children: [] }], loaded: true, expanded: true, connected: true };
  const treeB = { id: "conn:b", type: "connection", children: [{ id: "db:keep", children: [] }], loaded: true, expanded: true, connected: true };
  const app = new context.SyncApp(); let refreshed = 0;
  Object.assign(app, { connections: [c("a"), c("b")], groups: ["g"], infoRequest: 7,
    openConnections: new Set(["a", "b"]), openDbs: new Set(["a/db", "b/db"]), tableMetaCache: new Map([["a/db/t", {}], ["b/db/t", {}]]),
    treeNodes: [treeA, { id: "group:g", type: "group", children: [treeB] }],
    tabs: [{ id: "q", connectionId: "a", kind: "query", dirty: true }, { id: "d", connectionId: "a", kind: "data" }, { id: "keep", connectionId: "b", kind: "data" }],
    views: new Map([["q", query], ["d", data]]), refreshTree() { refreshed++; }, renderTabs() { refreshed++; },
  });
  return { app, c, query, data, draft, treeA, treeB, refreshed: () => refreshed };
}
test("real App applies structural changes without closing pools or invalidating open pages", () => {
  const f = appFixture(), childrenA = f.treeA.children, childrenB = f.treeB.children;
  f.app.applyDatasourceSync({ ...result(), connections: [{ ...f.c("b"), group: "other" }, { ...f.c("a"), name: "new" }], groups: ["g", "other", "empty"] });
  assert.deepEqual([...f.app.openConnections], ["a", "b"]); assert.equal(f.treeA.children, childrenA); assert.equal(f.treeB.children, childrenB);
  assert.equal(f.query.invalidated, 0); assert.equal(f.data.el.firstChild, f.draft); assert.equal(f.app.tabs.some(t => t.datasourceStale), false); assert.equal(f.refreshed(), 2);
});
test("real App invalidates only changed/deleted runtime state, preserves SQL and freezes existing data DOM", () => {
  const f = appFixture(), childrenB = f.treeB.children;
  f.app.applyDatasourceSync({ ...result(), connections: [{ ...f.c("a"), host: "remote" }, f.c("b")], groups: ["g"] });
  assert.deepEqual([...f.app.openConnections], ["b"]); assert.deepEqual([...f.app.openDbs], ["b/db"]); assert.deepEqual([...f.app.tableMetaCache.keys()], ["b/db/t"]);
  assert.equal(f.treeA.children.length, 0); assert.equal(f.treeA.connected, false); assert.equal(f.treeA.loaded, false); assert.equal(f.treeB.children, childrenB);
  assert.equal(f.query.invalidated, 1); assert.equal(f.query.sql, "SELECT '未保存的 SQL'"); assert.equal(f.app.tabs[0].dirty, true);
  assert.match(f.app.tabs[0].datasourceStale, /旧页面已暂停/); assert.equal(f.app.tabs[2].datasourceStale, undefined);
  assert.equal(f.data.el.children[1].inert, true); assert.equal(f.data.el.children[1].firstChild, f.draft); assert.equal(f.app.infoRequest, 8);
});
test("actual stale Query execute and completion entry points never reconnect or read metadata", async () => {
  const notices = [], context = vm.createContext({});
  vm.runInContext(ts.transpileModule(`class PausedQuery { ${methods("QueryTabView", ["execute", "refreshCompletionData", "loadCompletionData"])} } globalThis.PausedQuery = PausedQuery;`, { compilerOptions: { target: ts.ScriptTarget.ES2021 } }).outputText, context);
  const query = new context.PausedQuery(); query.tab = { datasourceStale: "旧连接已暂停", connectionId: "a", schema: "db" }; query.app = { toast: (...args) => notices.push(args) };
  query.completionRequest = 0; query.editor = { setCompletionData() {} };
  await query.execute("DELETE FROM important", false); await query.refreshCompletionData(); await query.loadCompletionData();
  assert.equal(notices.length, 1); assert.match(notices[0][1], /旧连接已暂停/);
});
test("real App readiness includes nested metadata loading, queries, transactions and unsaved drafts", () => {
  class Query { constructor(running) { this.running = running; } isExecuting() { return this.running; } }
  class Data { constructor(pending) { this.pending = pending; } hasPendingChanges() { return this.pending; } }
  const context = vm.createContext({ QueryTabView: Query, DataTabView: Data });
  vm.runInContext(ts.transpileModule(`class ReadyApp { ${methods("App", ["datasourceSyncReady"])} } globalThis.ReadyApp = ReadyApp;`, { compilerOptions: { target: ts.ScriptTarget.ES2021 } }).outputText, context);
  const app = new context.ReadyApp(); let active = false;
  Object.assign(app, { treeNodes: [], tabs: [], views: new Map(), manualTransactionController: { hasActive: () => active } });
  assert.equal(app.datasourceSyncReady(), true);
  app.treeNodes = [{ children: [{ loading: true, children: [] }] }]; assert.equal(app.datasourceSyncReady(), false); app.treeNodes = [];
  active = true; assert.equal(app.datasourceSyncReady(), false); active = false;
  app.tabs = [{ id: "q", kind: "query", dirty: true }]; app.views.set("q", new Query(false)); assert.equal(app.datasourceSyncReady(), true, "unsaved SQL text is preserved, not a database edit");
  app.views.set("q", new Query(true)); assert.equal(app.datasourceSyncReady(), false);
  app.tabs = [{ id: "d", kind: "data" }]; app.views.set("d", new Data(true)); assert.equal(app.datasourceSyncReady(), false);
  app.tabs = [{ id: "design", kind: "table-design", dirty: true }]; assert.equal(app.datasourceSyncReady(), false);
});

test("driver mutation entry points acquire activity before touching the driver store", async () => {
  const source = await readFile("src-tauri/src/driver_manager.rs", "utf8");
  assert.match(source, /pub fn save_custom_driver\(state: State<'_, AppState>, driver: DriverDefinition\)[\s\S]*?state\.activity\.enter\(\)[\s\S]*?persist_custom_driver\(driver\)/);
  assert.match(source, /pub fn delete_custom_driver\(state: State<'_, AppState>, id: String\)[\s\S]*?state\.activity\.enter\(\)[\s\S]*?STORE_LOCK\.lock\(\)/);
});
