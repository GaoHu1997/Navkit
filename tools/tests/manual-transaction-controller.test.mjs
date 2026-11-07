import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";

const bundle = await build({ entryPoints: ["src/core/ManualTransactions.ts"], bundle: true, platform: "node", format: "esm", write: false, logLevel: "silent" });
const { ManualTransactions } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);
const inactive = () => ({ active: false, schema: "", owner: "", failed: false });
const defer = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };

function fixture() {
  const context = { id: "q1", connectionId: "c1", schema: "main", kind: "query", running: false, pendingEdits: false, prepare: async () => {} };
  const contexts = new Map([[context.id, context]]);
  const calls = [], notices = [], removed = [];
  let current = context.id;
  let remote = inactive();
  let confirmation = async () => true;
  const source = {
    startManualTransaction: async (id, schema, owner) => { calls.push(["start", id, schema, owner]); remote = { active: true, schema, owner, failed: false }; return { ...remote }; },
    manualTransactionStatus: async id => { calls.push(["status", id]); return { ...remote }; },
    finishManualTransaction: async (id, owner, commit) => { calls.push(["finish", id, owner, commit]); remote = inactive(); return { ...remote }; },
    closeConnection: async id => { calls.push(["close", id]); remote = inactive(); },
  };
  const host = {
    context: id => { const item = contexts.get(id || current); return item && { ...item }; },
    connection: id => ({ id, name: "测试", kind: "sqlite", params: "", read_only: false }),
    createQuery: async (id, schema) => { const item = { ...context, id: "q-new", connectionId: id, schema, kind: "query", pendingEdits: false }; contexts.set(item.id, item); current = item.id; calls.push(["create-query", id, schema]); return { ...item }; },
    removeTab: id => { removed.push(id); contexts.delete(id); },
    focus: id => { current = id; },
    changed: () => {},
    toast: (kind, message) => notices.push([kind, message]),
  };
  const controller = new ManualTransactions(source, host, (...args) => confirmation(...args));
  return { controller, source, host, context, contexts, calls, notices, removed,
    confirm: fn => { confirmation = fn; }, current: id => { current = id; }, remote: state => { remote = { ...state }; } };
}
const operations = f => f.calls.filter(call => call[0] !== "status");

test("取消开始零写入；确认期间连续开始只创建一次真实事务", async () => {
  const f = fixture();
  f.confirm(async () => false);
  assert.equal(await f.controller.begin(), false);
  assert.deepEqual(operations(f), []);
  assert.equal(f.controller.busy("c1"), false);
  const answer = defer(); f.confirm(() => answer.promise);
  const first = f.controller.begin();
  assert.equal(f.controller.busy("c1"), true);
  assert.equal(await f.controller.begin(), false);
  answer.resolve(true);
  assert.equal(await first, true);
  assert.deepEqual(operations(f), [["start", "c1", "main", "q1"]]);
  assert.equal(f.controller.status("c1").owner, "q1");
  assert.equal(f.controller.busy("c1"), false);
});

test("晚到的自动提交状态不能覆盖已开始事务；晚到活跃状态不能复活已提交事务", async () => {
  const f = fixture(), lateInactive = defer();
  const ordinaryStatus = f.source.manualTransactionStatus;
  f.source.manualTransactionStatus = () => lateInactive.promise;
  const first = f.controller.refresh("c1");
  f.source.manualTransactionStatus = ordinaryStatus;
  assert.equal(await f.controller.begin(), true);
  lateInactive.resolve(inactive()); await first;
  assert.equal(f.controller.status("c1").active, true);
  const old = { ...f.controller.status("c1") }, lateActive = defer();
  f.source.manualTransactionStatus = () => lateActive.promise;
  const second = f.controller.refresh("c1");
  f.source.manualTransactionStatus = ordinaryStatus;
  assert.equal(await f.controller.finish(true), true);
  lateActive.resolve(old); await second;
  assert.equal(f.controller.status("c1"), undefined);
});

test("非所有者、错误模式、执行中和失败事务不能提交，失败事务仍可回滚", async () => {
  const f = fixture(); assert.equal(await f.controller.begin(), true);
  f.contexts.set("q2", { ...f.context, id: "q2" }); f.current("q2");
  assert.equal(await f.controller.finish(true), false);
  f.current("q1"); f.context.schema = "other";
  assert.equal(await f.controller.finish(false), false);
  f.context.schema = "main"; f.context.running = true;
  assert.equal(await f.controller.finish(true), false);
  f.context.running = false;
  f.remote({ ...f.controller.status("c1"), failed: true }); await f.controller.refresh("c1");
  assert.equal(await f.controller.finish(true), false);
  assert.equal(f.calls.filter(call => call[0] === "finish").length, 0);
  assert.equal(await f.controller.finish(false), true);
  assert.deepEqual(f.calls.find(call => call[0] === "finish"), ["finish", "c1", "q1", false]);
});

test("确认后切页和准备期间模式变化均取消开始，不偷换数据库", async () => {
  const f = fixture();
  f.contexts.set("q2", { ...f.context, id: "q2" });
  f.confirm(async () => { f.current("q2"); return true; });
  assert.equal(await f.controller.begin(), false);
  assert.deepEqual(operations(f), []);
  f.current("q1"); f.confirm(async () => true);
  f.context.prepare = async () => { f.context.schema = "other"; };
  assert.equal(await f.controller.begin(), false);
  assert.deepEqual(operations(f), []);
  assert(f.notices.some(([, message]) => /原数据库|页面/.test(message)));
});

test("表数据待编辑不开始；干净表必须创建独立查询并准备真实元数据", async () => {
  const f = fixture(); f.context.kind = "table"; f.context.pendingEdits = true;
  assert.equal(await f.controller.begin(), false);
  assert.deepEqual(operations(f), []);
  f.context.pendingEdits = false;
  assert.equal(await f.controller.begin(), true);
  assert.deepEqual(operations(f), [["create-query", "c1", "main"], ["start", "c1", "main", "q-new"]]);
  assert.equal(f.controller.status("c1").owner, "q-new");
  assert.equal(await f.controller.finish(false), true);
});

test("关闭事务页取消保留；回滚失败不关闭，实际回滚确认成功才删除页面", async () => {
  const f = fixture(); assert.equal(await f.controller.begin(), true);
  f.confirm(async () => false);
  assert.equal(await f.controller.closeOwner({ ...f.context }), false);
  assert.deepEqual(f.removed, []);
  assert(f.controller.status("c1"));
  f.confirm(async () => true);
  const finish = f.source.finishManualTransaction;
  f.source.finishManualTransaction = async () => { throw new Error("回滚断连"); };
  assert.equal(await f.controller.closeOwner({ ...f.context }), false);
  assert.deepEqual(f.removed, []); assert(f.controller.status("c1"));
  f.source.finishManualTransaction = finish;
  assert.equal(await f.controller.closeOwner({ ...f.context }), true);
  assert.deepEqual(f.removed, ["q1"]);
  assert.equal(f.controller.status("c1"), undefined);
});

test("关闭连接取消零关闭；原生关闭失败不伪造结束，重新读取实际状态", async () => {
  const f = fixture(); assert.equal(await f.controller.begin(), true);
  f.confirm(async () => false);
  assert.equal(await f.controller.closeConnection("c1"), false);
  assert.equal(f.calls.filter(call => call[0] === "close").length, 0);
  f.confirm(async () => true);
  const close = f.source.closeConnection;
  f.source.closeConnection = async () => { f.remote(inactive()); throw new Error("连接关闭失败"); };
  assert.equal(await f.controller.closeConnection("c1"), false);
  assert.equal(f.controller.status("c1"), undefined, "失败后只依据重新读取的权威状态，不保留已失效的活跃缓存");
  assert(f.notices.some(([kind, message]) => kind === "error" && /连接关闭失败/.test(message)));
  f.source.closeConnection = close;
  assert.equal(await f.controller.closeConnection("c1"), true);
});

test("未收到事务结束确认不报成功；状态查询断连不会清空已有活跃状态", async () => {
  const f = fixture(); assert.equal(await f.controller.begin(), true);
  f.source.finishManualTransaction = async () => ({ ...f.controller.status("c1") });
  assert.equal(await f.controller.finish(true), false);
  assert(f.controller.status("c1"));
  assert(!f.notices.some(([, message]) => message === "事务已提交"));
  f.source.manualTransactionStatus = async () => { throw new Error("状态网络错误"); };
  await assert.rejects(f.controller.refresh("c1"), /状态网络错误/);
  assert(f.controller.status("c1"));
  assert.equal(await f.controller.closeConnection("c1"), false);
  assert.equal(f.calls.filter(call => call[0] === "close").length, 0);
});
