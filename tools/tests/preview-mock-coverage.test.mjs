/** 预览夹具与桌面 IPC 注册的命令覆盖防回归：
 *
 *  背景：前端每个 call()/invoke() 的命令必须在两处就绪，否则运行期弹「未实现的命令」IpcError：
 *    1. public/preview-bootstrap.js 的 __PREVIEW_MOCK__（浏览器静态预览）；
 *    2. src-tauri/src/lib.rs 的 generate_handler![...]（桌面真实 IPC）。
 *  本测试从 src 目录源码静态提取全部命令字面量，对两处注册表做集合差，缺一即失败。
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import vm from "node:vm";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

function listFiles(dir, ext, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) listFiles(full, ext, out);
    else if (entry.name.endsWith(ext)) out.push(full);
  }
  return out;
}

/** 从 src 目录的全部 .ts 提取 call("cmd") / call<T>("cmd") / invoke("cmd") 的命令字面量。 */
function frontendCommands() {
  const commands = new Set();
  const pattern = /\b(?:call|invoke)(?:<[\s\S]{0,200}?>)?\(\s*["'`]([a-z_][a-z_0-9]{1,80})["'`]/g;
  for (const file of listFiles(join(root, "src"), ".ts")) {
    const text = readFileSync(file, "utf8");
    for (const match of text.matchAll(pattern)) commands.add(match[1]);
  }
  return [...commands].sort();
}

/** 在 VM 里加载 preview-bootstrap.js，读取 window.__PREVIEW_MOCK__ 的命令清单。 */
function previewMockCommands() {
  const sandbox = {
    console: { warn() {}, error() {}, log() {} },
    document: {
      querySelectorAll: () => [],
      addEventListener() {},
      removeEventListener() {},
    },
    location: { search: "" },
    MouseEvent: class {},
    URLSearchParams,
    setTimeout: () => 0,
    clearTimeout() {},
  };
  sandbox.window = sandbox;
  const source = readFileSync(join(root, "public", "preview-bootstrap.js"), "utf8");
  vm.runInNewContext(source, sandbox, { filename: "preview-bootstrap.js" });
  const mock = sandbox.window.__PREVIEW_MOCK__;
  assert.ok(mock, "preview-bootstrap.js 必须注册 window.__PREVIEW_MOCK__");
  return Object.keys(mock).sort();
}

/** 从 lib.rs 的 generate_handler![...] 提取桌面注册的命令清单。 */
function desktopCommands() {
  const text = readFileSync(join(root, "src-tauri", "src", "lib.rs"), "utf8");
  const match = /generate_handler!\[([\s\S]*?)\]/.exec(text);
  assert.ok(match, "lib.rs 缺少 generate_handler! 注册表");
  const names = [];
  for (const entry of match[1].split(",")) {
    const tail = entry.trim().split("::").pop();
    if (tail) names.push(tail);
  }
  return names.sort();
}

test("前端调用的每个 IPC 命令都必须在预览 mock 中有响应（不再弹未实现命令的 IpcError）", () => {
  const used = frontendCommands();
  assert.ok(used.length > 100, `命令提取异常：只找到 ${used.length} 个命令，正则可能失效`);
  const mocked = new Set(previewMockCommands());
  const missing = used.filter((command) => !mocked.has(command));
  assert.deepEqual(
    missing,
    [],
    `预览 mock 缺少 ${missing.length} 个命令：${missing.join(", ")}。请在 public/preview-bootstrap.js 的 responses 中补齐同形夹具。`,
  );
});

test("前端调用的每个 IPC 命令都必须在桌面 generate_handler 中注册", () => {
  const used = frontendCommands();
  const registered = new Set(desktopCommands());
  const missing = used.filter((command) => !registered.has(command));
  assert.deepEqual(
    missing,
    [],
    `桌面端缺少 ${missing.length} 个命令注册：${missing.join(", ")}。请在 src-tauri/src/lib.rs 的 generate_handler! 中注册。`,
  );
});

test("关键命令必须出现在两侧注册表中（显式点名，防止提取正则漏报）", () => {
  const critical = [
    "read_table_design_snapshot",
    "preview_table_design",
    "execute_table_design",
    "query_result_snapshot",
    "scheduler_list_jobs",
    "register_batch_execution",
  ];
  const mocked = new Set(previewMockCommands());
  const registered = new Set(desktopCommands());
  for (const command of critical) {
    assert.ok(mocked.has(command), `预览 mock 缺少关键命令 ${command}`);
    assert.ok(registered.has(command), `桌面注册表缺少关键命令 ${command}`);
  }
});
