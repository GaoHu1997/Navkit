// 后台任务中心服务（需求 1.3）：注册 / 进度 / 完成 / 失败 / 清空已完成 / opener。
import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

const bundle = await build({
  entryPoints: ["src/core/backgroundTasks.ts"], bundle: true, write: false,
  platform: "node", format: "esm", logLevel: "silent",
});
const { taskCenter, trackTask, TASK_KIND_LABEL } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);

test("注册任务进入列表且运行计数正确", () => {
  taskCenter.reset();
  const handle = taskCenter.register("import", "导入 · sys_user");
  const { tasks, running } = taskCenter.snapshot();
  assert.equal(tasks.length, 1);
  assert.equal(running, 1);
  assert.equal(tasks[0].title, "导入 · sys_user");
  assert.equal(tasks[0].status, "running");
  assert.equal(TASK_KIND_LABEL.import, "导入");
  handle.finish("共导入 100 行");
  assert.equal(taskCenter.snapshot().running, 0);
  assert.equal(taskCenter.snapshot().tasks[0].status, "success");
  assert.equal(taskCenter.snapshot().tasks[0].detail, "共导入 100 行");
});

test("进度更新与订阅回调", () => {
  taskCenter.reset();
  const seen = [];
  const unsubscribe = taskCenter.subscribe(() => seen.push(taskCenter.snapshot().tasks.length));
  const handle = taskCenter.register("export", "导出 · sys_role");
  handle.update({ done: 40, total: 100, message: "正在导出" });
  assert.ok(seen.length >= 2, "注册与更新都应触发订阅");
  assert.equal(taskCenter.snapshot().tasks[0].progress.done, 40);
  assert.equal(taskCenter.snapshot().tasks[0].progress.total, 100);
  unsubscribe();
  handle.update({ done: 60, total: 100 });
  const after = taskCenter.snapshot().tasks[0].progress.done;
  handle.finish();
  assert.equal(after, 60);
});

test("失败与取消终态不可被后续更新覆盖", () => {
  taskCenter.reset();
  const handle = taskCenter.register("backup", "备份 · navkit_demo");
  handle.fail("连接已断开");
  handle.update({ done: 5, total: 10 });
  handle.finish("迟到的成功");
  const task = taskCenter.snapshot().tasks[0];
  assert.equal(task.status, "error");
  assert.equal(task.detail, "连接已断开");
  assert.equal(task.progress, undefined);
});

test("清空已完成保留运行中的任务", () => {
  taskCenter.reset();
  const runningHandle = taskCenter.register("sync", "整库同步");
  const doneHandle = taskCenter.register("dump", "SQL 转储");
  const failed = taskCenter.register("backup", "备份 · x");
  doneHandle.finish();
  failed.fail("失败");
  taskCenter.clearFinished();
  const tasks = taskCenter.snapshot().tasks;
  assert.equal(tasks.length, 1);
  assert.equal(tasks[0].title, "整库同步");
  assert.equal(taskCenter.snapshot().running, 1);
  runningHandle.finish();
  taskCenter.clearFinished();
  assert.equal(taskCenter.snapshot().tasks.length, 0);
});

test("点击任务命中 opener；清空后 opener 一并清理", () => {
  taskCenter.reset();
  const handle = taskCenter.register("dictionary", "数据字典导出");
  let opened = 0;
  handle.setOpener(() => { opened += 1; });
  const [task] = taskCenter.snapshot().tasks;
  assert.equal(taskCenter.open(task), true);
  assert.equal(opened, 1);
  // 无 opener 的任务返回 false（由调用方打开详情弹窗）
  taskCenter.register("other", "其他任务");
  const [otherTask] = taskCenter.snapshot().tasks;
  assert.equal(taskCenter.open(otherTask), false);
  handle.finish();
  taskCenter.clearFinished();
  const stale = { ...task, status: "success" };
  assert.equal(taskCenter.open(stale), false);
});

test("trackTask 包装 promise 成败", async () => {
  taskCenter.reset();
  const value = await trackTask("transfer", "数据传输", async handle => {
    handle.update({ done: 1, total: 2 });
    return 42;
  });
  assert.equal(value, 42);
  assert.equal(taskCenter.snapshot().tasks[0].status, "success");
  await assert.rejects(
    trackTask("transfer", "失败任务", async () => { throw new Error("boom"); }),
    /boom/,
  );
  assert.equal(taskCenter.snapshot().tasks[0].status, "error");
  assert.match(taskCenter.snapshot().tasks[0].detail, /boom/);
});

test("任务列表上限 50 条，最新在前", () => {
  taskCenter.reset();
  const handles = [];
  for (let i = 0; i < 55; i += 1) handles.push(taskCenter.register("other", `任务 ${i}`));
  const tasks = taskCenter.snapshot().tasks;
  assert.equal(tasks.length, 50);
  assert.equal(tasks[0].title, "任务 54");
  for (const handle of handles) handle.finish();
  taskCenter.clearFinished();
  assert.equal(taskCenter.snapshot().tasks.length, 0);
});
