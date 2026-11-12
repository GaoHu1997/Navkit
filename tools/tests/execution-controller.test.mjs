import assert from "node:assert/strict";
import test, { afterEach } from "node:test";
const controllers = new Set();
afterEach(() => { for (const controller of controllers) controller.dispose(); controllers.clear(); });
import { build } from "esbuild";
import { webcrypto } from "node:crypto";
const bundle = await build({ entryPoints: ["src/core/execution.ts"], bundle: true, platform: "node", format: "esm", write: false, logLevel: "silent" });
const { ExecutionController, IpcError } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);
const apiBundle = await build({ entryPoints: ["src/core/api.ts"], bundle: true, platform: "node", format: "esm", write: false, logLevel: "silent" });
const { api } = await import(`data:text/javascript;base64,${Buffer.from(apiBundle.outputFiles[0].text).toString("base64")}`);
const defer = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const request = () => ({ execution_id: webcrypto.randomUUID(), connection_id: "saved1", namespace: "main", owner_id: "tab1", sql: "SELECT 1", timeout_ms: 0 });
const outcome = (req, state = "succeeded") => ({ execution_id: req.execution_id, state, result: state === "succeeded" ? { sets: [], messages: [], elapsed_ms: 1 } : null, error: null, deadline: { execution_id: req.execution_id, budget_ms: null, elapsed_ms: 1, deadline_reached: state === "timed_out", stop_confirmed: ["timed_out", "cancelled"].includes(state) }, effects: state === "succeeded" ? "completed" : "possibly_applied", transaction_state: "none" });
function fixture() {
  const work = defer(), ack = defer(), changes = [], calls = [];
  let remote = "running", resourcesPending = false;
  const source = {
    executeSqlV2: req => { calls.push(["run", req]); return work.promise; },
    cancelExecution: ctx => { calls.push(["cancel", ctx]); remote = "cancel_requested"; return ack.promise; },
    executionStatus: async ctx => { calls.push(["status", ctx]); return { execution_id: ctx.execution_id, state: remote, resources_pending: resourcesPending }; },
  };
  const controller = new ExecutionController(source, state => changes.push(state)); controllers.add(controller);
  return { controller, source, work, ack, changes, calls, remote: (state, pending = false) => { remote = state; resourcesPending = pending; } };
}
test("双击只发一次真实执行；冻结连接/模式/所有者与0预算", async () => {
  const f = fixture(), req = request(), running = f.controller.run(req);
  req.connection_id = "other"; req.namespace = "other"; req.owner_id = "other";
  assert.equal(f.controller.state, "queued"); assert.equal(f.controller.busy, true);
  await assert.rejects(f.controller.run(request()), /仍在执行/);
  const sent = f.calls[0][1]; assert.equal(Object.isFrozen(sent), true); assert.equal(sent.connection_id, "saved1"); assert.equal(sent.timeout_ms, 0);
  f.work.resolve(outcome(sent)); assert.equal((await running).state, "succeeded"); assert.equal(f.controller.busy, false);
  assert.equal(await f.controller.cancel(), null); assert.equal(f.calls.filter(c => c[0] === "run").length, 1);
});
test("停止回执不是终态；重复点击共享控制请求，等完整outcome才结束", async () => {
  const f = fixture(), req = request(), running = f.controller.run(req);
  const a = f.controller.cancel(), b = f.controller.cancel();
  assert.equal(f.calls.filter(c => c[0] === "cancel").length, 1);
  f.ack.resolve({ execution_id: req.execution_id, accepted: true, state: "cancel_requested", message: "等待DB" }); await a; await b;
  assert.equal(f.controller.busy, true); assert.equal(f.controller.state, "cancel_requested");
  await assert.rejects(f.controller.run(request()));
  f.work.resolve(outcome(req, "cancelled")); assert.equal((await running).deadline.stop_confirmed, true); assert.equal(f.controller.busy, false);
});
test("后端deadline/unknown不由客户端timer推断，不重放写入", async () => {
  for (const state of ["timed_out", "unknown", "failed"]) {
    const f = fixture(), req = request(), running = f.controller.run(req);
    f.work.resolve(outcome(req, state)); assert.equal((await running).state, state);
    assert.deepEqual(f.changes, ["queued", state]); assert.equal(f.calls.filter(c => c[0] === "run").length, 1);
  }
});
test("传输失败保留结构化错误并标unknown；不把object变成字符串", async () => {
  const error = new IpcError({ message: "拒绝越权", code: "EXECUTION_FORBIDDEN", operation_id: "id", retryable: false });
  assert.equal(error.message, "拒绝越权"); assert.equal(error.code, "EXECUTION_FORBIDDEN"); assert.equal(error.operation_id, "id"); assert.equal(new IpcError("旧错误").message, "旧错误");
  const f = fixture(), run = f.controller.run(request()); f.work.reject(error); await assert.rejects(run, e => e.code === "EXECUTION_FORBIDDEN"); assert.equal(f.controller.state, "unknown");
});
test("dispose及不匹配ID的迟到结果不会发布到新页", async () => {
  const f = fixture(), req = request(), running = f.controller.run(req); f.controller.dispose(); f.work.resolve(outcome(req));
  assert.equal(await running, null); assert.deepEqual(f.changes, ["queued"]);
  const other = fixture(), req2 = request(), run2 = other.controller.run(req2); other.work.resolve(outcome(request()));
  await assert.rejects(run2, error => error.code === "EXECUTION_RESULT_MISMATCH"); assert.deepEqual(other.changes, ["queued", "unknown"]);
});
test("已完成的停止回执仍等待完整结果，不提早解锁或允许第二执行", async () => {
  const f = fixture(), req = request(), running = f.controller.run(req), cancelling = f.controller.cancel();
  f.ack.resolve({ execution_id: req.execution_id, accepted: false, state: "succeeded", message: "执行结束" }); await cancelling;
  assert.equal(f.controller.busy, true); await assert.rejects(f.controller.run(request()));
  f.work.resolve(outcome(req)); await running; assert.equal(f.controller.busy, false);
});
test("上一执行的迟到停止回执或错误不能抢占下一执行的停止请求", async () => {
  for (const lateFailure of [false, true]) {
    const works = [defer(), defer()], acks = [defer(), defer()], calls = [], states = [];
    const source = {
      executeSqlV2: req => { calls.push(["run", req]); return works[calls.filter(c => c[0] === "run").length - 1].promise; },
      cancelExecution: ctx => { calls.push(["cancel", ctx]); return acks[calls.filter(c => c[0] === "cancel").length - 1].promise; },
      executionStatus: async ctx => ({ execution_id: ctx.execution_id, state: "running" }),
    };
    const controller = new ExecutionController(source, state => states.push(state)); controllers.add(controller);
    const first = request(), run1 = controller.run(first), cancel1 = controller.cancel();
    works[0].resolve(outcome(first)); await run1;
    const next = request(), run2 = controller.run(next), cancel2 = controller.cancel();
    assert.equal(calls.filter(c => c[0] === "cancel").length, 2);
    if (lateFailure) acks[0].reject(new IpcError({ code: "EXECUTION_NOT_FOUND", message: "旧执行" }));
    else acks[0].resolve({ execution_id: first.execution_id, state: "cancel_requested", accepted: true });
    await cancel1;
    assert.equal(controller.state, "queued");
    const repeated = controller.cancel(); assert.equal(calls.filter(c => c[0] === "cancel").length, 2);
    acks[1].resolve({ execution_id: next.execution_id, state: "cancel_requested", accepted: true });
    await cancel2; await repeated; assert.equal(controller.state, "cancel_requested");
    works[1].resolve(outcome(next, "cancelled")); await run2; assert.equal(controller.busy, false);
  }
});
test("不匹配执行ID的停止回执拒绝发布，主执行仍等待真实结果", async () => {
  const f = fixture(), req = request(), running = f.controller.run(req), stopping = f.controller.cancel();
  f.ack.resolve({ execution_id: request().execution_id, state: "cancel_requested", accepted: true });
  await assert.rejects(stopping, e => e.code === "EXECUTION_CANCEL_ACK_MISMATCH");
  assert.equal(f.controller.busy, true); assert.equal(f.controller.state, "queued");
  f.work.resolve(outcome(req)); await running;
});
test("旧静态预览沿用显式fixture，取消和期限不伪造真实能力", async t => {
  const previous = globalThis.window; t.after(() => { globalThis.window = previous; });
  const calls = [], result = { sets: [], messages: ["preview"], elapsed_ms: 1 };
  globalThis.window = { __PREVIEW_MOCK__: { execute_sql: args => { calls.push(args); return result; } } };
  const caps = await api.executionCapabilities("saved1"); assert.equal(caps.cancel.level, "unsupported"); assert.equal(caps.deadline.level, "unsupported");
  const req = request(), res = await api.executeSqlV2(req); assert.equal(res.execution_id, req.execution_id); assert.equal(res.result, result); assert.equal(res.effects, "preview_fixture"); assert.equal(calls[0].sql, req.sql);
});
test("桌面Tauri bridge始终调用真实IPC，结构化拒绝保留code和message", async t => {
  const previous = globalThis.window; t.after(() => { globalThis.window = previous; });
  const req = request(), nativeResult = outcome(req), calls = []; let fixtureCalls = 0;
  globalThis.window = {
    __PREVIEW_MOCK__: { execute_sql_v2: () => { fixtureCalls++; return outcome(req, "failed"); } },
    __TAURI_INTERNALS__: { invoke: async (cmd, args) => { calls.push([cmd, args]); return nativeResult; } },
  };
  assert.equal(await api.executeSqlV2(req), nativeResult); assert.equal(fixtureCalls, 0);
  assert.equal(calls[0][0], "execute_sql_v2"); assert.equal(calls[0][1].request, req);
  globalThis.window.__TAURI_INTERNALS__.invoke = async () => { throw { message: "后端拒绝只读写入", code: "READ_ONLY_VIOLATION", operation_id: req.execution_id, retryable: false }; };
  // api 和 controller 在此 fixture 中分别 bundle，Error 子类构造器身份不同；验证真实错误契约。
  await assert.rejects(api.executeSqlV2(req), e => e instanceof Error && e.name === "IpcError" && e.message === "后端拒绝只读写入" && e.code === "READ_ONLY_VIOLATION" && e.operation_id === req.execution_id && e.retryable === false);
});
test("注册与停止竞态由状态确认重试同一ID，预算非法不发送", async () => {
  const f = fixture(), req = request(); let attempts = 0;
  f.source.cancelExecution = async ctx => {
    f.calls.push(["cancel", ctx]); attempts++;
    if (attempts === 1) throw new IpcError({ code: "EXECUTION_NOT_FOUND", message: "注册中" });
    return { execution_id: ctx.execution_id, accepted: true, state: "cancel_requested", message: "等待DB" };
  };
  const running = f.controller.run(req); assert.equal(await f.controller.cancel(), null);
  await new Promise(resolve => setTimeout(resolve, 240));
  assert.equal(f.controller.state, "cancel_requested"); assert.equal(attempts, 2); assert(f.calls.filter(c => c[0] === "cancel").every(c => c[1].execution_id === req.execution_id));
  f.work.resolve(outcome(req, "cancelled")); await running;
  for (const ms of [-1, NaN, Infinity, 86400001, 1.5]) await assert.rejects(f.controller.run({ ...request(), timeout_ms: ms }));
});
const signed = req => ({ ...req, execution_id: webcrypto.randomUUID(), execution_grant: "opaque-backend-grant" });
const waitFor = async condition => {
  const deadline = Date.now() + 1500;
  while (!condition()) { assert(Date.now() < deadline, "控制器状态等待超出测试上限"); await new Promise(resolve => setTimeout(resolve, 10)); }
};
test("grant协商冻结工作上下文，全部执行/停止/状态使用后台签发ID", async () => {
  const f = fixture(), grant = defer();
  f.source.issueGrant = req => { f.calls.push(["grant", req]); return grant.promise; };
  const req = { ...request(), transaction_owner: "tab1", origin: "query" }, running = f.controller.run(req);
  assert.equal(f.calls.length, 1); assert.equal(f.calls[0][0], "grant"); assert(Object.isFrozen(f.calls[0][1]));
  await assert.rejects(f.controller.run(request()));
  req.sql = "DROP TABLE items"; req.namespace = "other"; req.connection_id = "other";
  const granted = signed(f.calls[0][1]); grant.resolve(granted);
  await waitFor(() => f.calls.some(c => c[0] === "run"));
  const sent = f.calls.find(c => c[0] === "run")[1]; assert(Object.isFrozen(sent));
  assert.equal(sent.execution_id, granted.execution_id); assert.equal(sent.execution_grant, granted.execution_grant);
  assert.equal(sent.sql, "SELECT 1"); assert.equal(sent.namespace, "main"); assert.equal(sent.connection_id, "saved1");
  const cancelling = f.controller.cancel(); f.ack.resolve({ execution_id: granted.execution_id, state: "cancel_requested", accepted: true }); await cancelling;
  await waitFor(() => f.calls.some(c => c[0] === "status"));
  assert(f.calls.filter(c => ["run", "cancel", "status"].includes(c[0])).every(c => c[1].execution_id === granted.execution_id));
  f.work.resolve(outcome(granted, "cancelled")); await running; assert.equal(f.controller.busy, false);
});
test("grant协商期间停止只追加发送前停止标志，不抢先发送取消IPC", async () => {
  const f = fixture(), grant = defer(), req = request();
  f.source.issueGrant = original => { f.calls.push(["grant", original]); return grant.promise; };
  const running = f.controller.run(req);
  assert.equal(await f.controller.cancel(), null); assert.equal(await f.controller.cancel(), null);
  assert.equal(f.controller.state, "cancel_requested"); assert.equal(f.calls.filter(c => c[0] === "cancel").length, 0);
  assert.equal(f.calls.filter(c => c[0] === "run").length, 0);
  const granted = signed(f.calls[0][1]); grant.resolve(granted);
  await waitFor(() => f.calls.some(c => c[0] === "run"));
  const sent = f.calls.find(c => c[0] === "run")[1]; assert.equal(sent.cancel_before_send, true); assert.equal(sent.execution_id, granted.execution_id);
  f.work.resolve({ ...outcome(granted, "cancelled"), effects: "no_effect", resources_pending: false });
  assert.equal((await running).effects, "no_effect"); assert.equal(f.controller.busy, false);
  assert.equal(f.calls.filter(c => c[0] === "cancel").length, 0);
});
test("grant工作内容/端点/所有者/预算变更或空令牌明确拒绝，零执行", async () => {
  const changes = [
    { connection_id: "other" }, { namespace: "other" }, { owner_id: "other" }, { sql: "DELETE FROM items" },
    { timeout_ms: 100 }, { transaction_owner: "tab1" }, { origin: "job" }, { execution_grant: "" },
    { execution_id: "00000000-0000-0000-0000-000000000000" },
  ];
  for (const change of changes) {
    const f = fixture(); f.source.issueGrant = async req => ({ ...signed(req), ...change });
    await assert.rejects(f.controller.run(request()), error => error.code === "EXECUTION_GRANT_CONTEXT_MISMATCH");
    assert.equal(f.controller.state, "failed"); assert.equal(f.controller.busy, false);
    assert.equal(f.calls.filter(c => c[0] === "run").length, 0); assert.equal(f.calls.filter(c => c[0] === "cancel").length, 0);
  }
  const f = fixture(); f.source.issueGrant = async () => { throw new IpcError({ code: "EXECUTION_GRANT_INVALID", message: "授权被拒绝" }); };
  await assert.rejects(f.controller.run(request()), error => error.code === "EXECUTION_GRANT_INVALID");
  assert.equal(f.controller.state, "failed"); assert.equal(f.controller.busy, false); assert.equal(f.calls.length, 0);
});
test("完整Unknown仍有隔离资源时保持busy，排空状态不能把Unknown翻成成功", async () => {
  const f = fixture(), req = request(), running = f.controller.run(req);
  f.remote("unknown", true); f.work.resolve({ ...outcome(req, "unknown"), resources_pending: true });
  assert.equal((await running).state, "unknown"); assert.equal(f.controller.busy, true);
  await assert.rejects(f.controller.run(request()));
  await waitFor(() => f.calls.some(c => c[0] === "status"));
  assert.equal(f.controller.busy, true); assert.equal(f.controller.state, "unknown");
  f.remote("running", false);
  const polled = f.calls.filter(c => c[0] === "status").length;
  await waitFor(() => f.calls.filter(c => c[0] === "status").length > polled);
  assert.equal(f.controller.busy, true); assert.equal(f.controller.state, "unknown");
  f.remote("succeeded", false); await waitFor(() => !f.controller.busy);
  assert.equal(f.controller.state, "unknown"); assert(!f.changes.includes("succeeded")); assert.equal(f.calls.filter(c => c[0] === "run").length, 1);
});
test("发送后的主传输断开保留同一上下文直到授权终态排空，不自动重放", async () => {
  const f = fixture(), req = request(), running = f.controller.run(req);
  f.work.reject(new IpcError({ code: "NETWORK_ERROR", message: "主传输断开" })); await assert.rejects(running);
  assert.equal(f.controller.state, "unknown"); assert.equal(f.controller.busy, true); await assert.rejects(f.controller.run(request()));
  f.remote("unknown", false); await waitFor(() => !f.controller.busy);
  assert.equal(f.controller.state, "unknown"); assert.equal(f.calls.filter(c => c[0] === "run").length, 1);
});
test("异身份状态不能解锁或停止轮询；dispose不再发送协商中的SQL", async () => {
  const f = fixture(), req = request(), running = f.controller.run(req);
  f.source.executionStatus = async ctx => { f.calls.push(["status", ctx]); return { execution_id: request().execution_id, state: "succeeded", resources_pending: false }; };
  f.work.resolve({ ...outcome(req, "unknown"), resources_pending: true }); await running;
  await waitFor(() => f.calls.filter(c => c[0] === "status").length >= 2);
  assert.equal(f.controller.busy, true); assert.equal(f.controller.state, "unknown");
  f.controller.dispose();
  const other = fixture(), grant = defer(); other.source.issueGrant = original => { other.calls.push(["grant", original]); return grant.promise; };
  const pending = other.controller.run(request()); other.controller.dispose(); grant.resolve(signed(other.calls[0][1]));
  assert.equal(await pending, null); assert.equal(other.calls.filter(c => c[0] === "run").length, 0); assert.deepEqual(other.changes, ["queued"]);
});
test("桌面grant协商调用真实issue_execution_grant，不使用静态预览冒充授权", async t => {
  const previous = globalThis.window; t.after(() => { globalThis.window = previous; });
  const req = request(), granted = signed(req), calls = []; let previewCalls = 0;
  globalThis.window = {
    __PREVIEW_MOCK__: { issue_execution_grant: () => { previewCalls++; return signed(req); } },
    __TAURI_INTERNALS__: { invoke: async (cmd, args) => { calls.push([cmd, args]); return granted; } },
  };
  assert.equal(await api.issueGrant(req), granted); assert.equal(previewCalls, 0);
  assert.equal(calls[0][0], "issue_execution_grant"); assert.equal(calls[0][1].request, req);
});
test("迟到终态停止ACK不能解锁完整Unknown仍待排空的执行", async () => {
  for (const state of ["succeeded", "cancelled"]) {
    const f = fixture(), req = request(), running = f.controller.run(req), stopping = f.controller.cancel();
    f.remote("unknown", true); f.work.resolve({ ...outcome(req, "unknown"), resources_pending: true }); await running;
    f.ack.resolve({ execution_id: req.execution_id, accepted: false, state, resources_pending: false }); await stopping;
    assert.equal(f.controller.busy, true); assert.equal(f.controller.state, "unknown");
    await assert.rejects(f.controller.run(request())); assert.equal(await f.controller.cancel(), null);
    f.remote("unknown", false); await waitFor(() => !f.controller.busy);
    assert.equal(f.controller.state, "unknown"); assert.equal(f.calls.filter(c => c[0] === "run").length, 1);
    assert(!f.changes.includes("succeeded") && !f.changes.includes("cancelled"));
  }
});
test("明确C1注册前拒绝不把未注册请求永久锁为Unknown，既不重签也不重放", async () => {
  for (const code of ["EXECUTION_GRANT_INVALID", "EXECUTION_ID_INVALID", "EXECUTION_CONTEXT_INVALID", "EXECUTION_OWNER_MISMATCH", "EXECUTION_BUDGET_INVALID", "EXECUTION_BUSY", "EXECUTION_QUARANTINED"]) {
    const f = fixture(), running = f.controller.run(request());
    f.work.reject(new IpcError({ code, message: "注册前拒绝，没有发送SQL" })); await assert.rejects(running, error => error.code === code);
    assert.equal(f.controller.state, "failed"); assert.equal(f.controller.busy, false);
    assert.equal(f.calls.filter(c => c[0] === "run").length, 1); assert.equal(f.calls.filter(c => c[0] === "cancel").length, 0);
    assert.equal(await f.controller.cancel(), null);
  }
  const f = fixture(), running = f.controller.run(request());
  f.work.reject(new IpcError({ code: "EXECUTION_ID_DUPLICATE", message: "已有执行可能仍在进行" })); await assert.rejects(running);
  assert.equal(f.controller.state, "unknown"); assert.equal(f.controller.busy, true); await assert.rejects(f.controller.run(request()));
});
