/**
 * 静态审计：前端入口命令 id ↔ runCommand 分支 ↔ 后端 generate_handler! 三方比对。
 *
 * 用法:  node tools/acceptance/audit_entries.mjs [仓库根]
 * 输出:  stdout 报告 + docs/验收证据/audit_entries.json
 * 退出码: 发现「死入口 / 缺失命令」时为 2，否则 0（孤儿功能只记录不致败）。
 */
import fs from "node:fs";
import path from "node:path";

const root = path.resolve(process.argv[2] ?? process.cwd());
const read = (p) => fs.readFileSync(path.join(root, p), "utf8");

// ---------- 1. menu.ts：按导出结构分组收集全部入口 id ----------
const menuSrc = read("src/ui/menu.ts");
const menuGroups = {}; // structureName -> { entries: [{id,label,disabled}], contexts: {} }

function extractEntries(src) {
  const out = [];
  const re = /\{\s*id:\s*"([^"]+)"([^}]*)\}/g;
  let m;
  while ((m = re.exec(src))) {
    out.push({
      id: m[1],
      label: (src.slice(m.index, m.index + 120).match(/label:\s*"([^"]+)"/) || [])[1] ?? "",
      disabled: /disabled:\s*true/.test(m[2]),
    });
  }
  return out;
}

function spans(name) {
  // 切出 export const X ... / export function X ... 的文件段
  const re = /export\s+(?:const|function)\s+(\w+)/g;
  const found = [];
  let m;
  while ((m = re.exec(name))) found.push({ name: m[1], index: m.index });
  for (let i = 0; i < found.length; i++) {
    found[i].end = i + 1 < found.length ? found[i + 1].index : name.length;
  }
  return found;
}

for (const s of spans(menuSrc)) {
  const body = menuSrc.slice(s.index, s.end);
  if (s.name === "treeContextMenu") {
    // 按 case "type" 再切
    const ctx = {};
    const cre = /case\s+"([^"]+)":/g;
    const cases = [];
    let cm;
    while ((cm = cre.exec(body))) cases.push({ type: cm[1], index: cm.index });
    for (let i = 0; i < cases.length; i++) {
      const seg = body.slice(cases[i].index, i + 1 < cases.length ? cases[i + 1].index : body.length);
      if (!ctx[cases[i].type]) ctx[cases[i].type] = extractEntries(seg);
    }
    menuGroups.treeContextMenu = { entries: [], contexts: ctx };
  } else {
    menuGroups[s.name] = { entries: extractEntries(body), contexts: null };
  }
}

// ---------- 2. Ribbon.ts：RIBBON_MAIN 按钮 ----------
const ribbonSrc = read("src/ui/Ribbon.ts");
{
  var ribbonItems = [];
  const arrStart = ribbonSrc.indexOf("RIBBON_MAIN");
  const arrEnd = ribbonSrc.indexOf("];", arrStart);
  const block = ribbonSrc.slice(arrStart, arrEnd);
  const re = /\{\s*id:\s*"([^"]+)"([^}]*)\}/g;
  let m;
  while ((m = re.exec(block))) {
    ribbonItems.push({
      id: m[1],
      label: (m[2].match(/label:\s*"([^"]+)"/) || [])[1] ?? "",
      dropdown: /dropdown:\s*true/.test(m[2]),
      disabled: /disabled:\s*true/.test(m[2]),
    });
  }
}

// ---------- 3. App.ts：runCommand 全部 case + 字面量调用 + 工具栏条目 ----------
const appSrc = read("src/core/App.ts");

function funcSpan(src, header) {
  const i = src.indexOf(header);
  if (i < 0) return null;
  // 花括号配平
  let depth = 0,
    started = false;
  for (let j = i; j < src.length; j++) {
    if (src[j] === "{") {
      depth++;
      started = true;
    } else if (src[j] === "}") {
      depth--;
      if (started && depth === 0) return { start: i, end: j + 1 };
    }
  }
  return { start: i, end: src.length };
}

const runSpan = funcSpan(appSrc, "async runCommand(id: string)");
const runCases = [];
{
  const re = /case\s+"([^"]+)"/g;
  let m;
  while ((m = re.exec(appSrc.slice(runSpan.start, runSpan.end)))) runCases.push(m[1]);
}

// dispatchCommand 前缀路由
const dispatchSpan = funcSpan(appSrc, "private dispatchCommand(id: string)");
const dispatchPrefixes = [];
{
  const body = appSrc.slice(dispatchSpan.start, dispatchSpan.end);
  const re = /(?:startsWith|===)\s*\(?("([^"]+)")/g;
  let m;
  while ((m = re.exec(body))) dispatchPrefixes.push(m[2]);
}

// 全文件 runCommand("...") 字面量
const runLiteralCalls = [];
{
  const re = /runCommand\("([^"]+)"\)/g;
  let m;
  while ((m = re.exec(appSrc))) runLiteralCalls.push(m[1]);
}

// TabToolbar 条目（App.ts 中全部 { id: "..." } 对象字面量；disabled 可能是动态表达式）
const appObjIds = [];
{
  const re = /\{[^{}]*?id:\s*"([^"]+)"[^{}]*?\}/g;
  let m;
  while ((m = re.exec(appSrc))) {
    const body = m[0];
    appObjIds.push({
      id: m[1],
      label: (body.match(/label:\s*"([^"]*)"/) || [])[1] ?? "",
      disabled: /disabled:\s*true/.test(body) ? "static" : /disabled:/.test(body) ? "dynamic" : false,
      dropdown: /dropdown:\s*true/.test(body),
    });
  }
}

// 全部 switch case（任意函数），用于捕捉二级分发
const allCases = [];
{
  const re = /case\s+"([^"]+)"/g;
  let m;
  while ((m = re.exec(appSrc))) allCases.push(m[1]);
}

// ---------- 4. api.ts：前端→后端命令映射 + 实际使用点 ----------
const apiSrc = read("src/core/api.ts");
const apiCmds = []; // { method, cmd }
{
  // 找到每个 call 调用的命令名（泛型可含嵌套 <>），再向前回溯最近的方法名定义
  const callRe = /call(?:<(?:[^<>]|<[^<>]*>)*>)?\(\s*"([a-z_]+)"/g;
  const methodRe = /^ {2}(?:async )?(\w+):/gm;
  let m;
  const seen = new Set();
  while ((m = callRe.exec(apiSrc))) {
    methodRe.lastIndex = 0;
    let method = null,
      last;
    while ((last = methodRe.exec(apiSrc.slice(0, m.index)))) method = last[1];
    if (method && !seen.has(method)) {
      seen.add(method);
      apiCmds.push({ method, cmd: m[1] });
    }
  }
}

// src 下 api.method 实际调用点
const usedApiMethods = new Set();
{
  const files = ["src/core/App.ts", "src/dialogs", "src/ui"];
  const walk = (p) => {
    const abs = path.join(root, p);
    const st = fs.statSync(abs);
    if (st.isDirectory()) return fs.readdirSync(abs).forEach((f) => walk(path.join(p, f)));
    if (!p.endsWith(".ts")) return;
    const src = fs.readFileSync(abs, "utf8");
    const re = /api\.(\w+)\(/g;
    let m;
    while ((m = re.exec(src))) usedApiMethods.add(m[1]);
  };
  files.forEach(walk);
}

// ---------- 5. lib.rs：generate_handler! 清单 ----------
const libSrc = read("src-tauri/src/lib.rs");
const handlers = [];
{
  const i = libSrc.indexOf("generate_handler!");
  const re = /commands::(\w+)/g;
  let m;
  while ((m = re.exec(libSrc.slice(i)))) handlers.push(m[1]);
}

// commands.rs 中每个命令的定义（#[tauri::command] pub fn / async fn）
const cmdSrc = read("src-tauri/src/commands.rs");
const definedCmds = new Set();
{
  const re = /#\[\s*tauri::command\s*\]\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)/g;
  let m;
  while ((m = re.exec(cmdSrc))) definedCmds.add(m[1]);
}

// ---------- 6. 交叉比对 ----------
const entrySources = {};
const allEntryIds = new Map(); // id -> [{source, label, disabled}]
function addEntry(src, e) {
  if (!allEntryIds.has(e.id)) allEntryIds.set(e.id, []);
  allEntryIds.get(e.id).push({ source: src, label: e.label, disabled: !!e.disabled });
}
for (const [g, v] of Object.entries(menuGroups)) {
  for (const e of v.entries ?? []) addEntry(`menu.${g}`, e);
  if (v.contexts) for (const [t, es] of Object.entries(v.contexts)) for (const e of es) addEntry(`menu.treeContext(${t})`, e);
}
for (const e of ribbonItems) addEntry("ribbon.main", e);
for (const e of appObjIds) addEntry("app.tabToolbar/obj", e);
// Ribbon 下拉按钮在 ribbonDropdown() 处理（弹出菜单而非 runCommand）
const dropdownHandled = new Set(["new-connection", "open-other", "text"]);
// 前缀动态路由（排序/列/收藏/最近）
const prefixRoutes = ["sort-asc:", "sort-desc:", "col-toggle:", "fav-open:", "recent-open:"];

const runSet = new Set(runCases);
const dispatchSet = new Set(dispatchPrefixes);

const deadEntries = [];
for (const [id, locs] of allEntryIds) {
  const clickable = locs.filter((l) => !l.disabled);
  if (!clickable.length) continue; // 全部为禁用态 → 合法
  if (dropdownHandled.has(id) && locs.some((l) => l.source === "ribbon.main")) continue;
  const handled = runSet.has(id) || dispatchSet.has(id) || runLiteralCalls.includes(id) || allCases.includes(id) ||
    prefixRoutes.some((p) => id.startsWith(p));
  if (!handled) deadEntries.push({ id, where: locs.map((l) => l.source).join(","), label: clickable[0].label });
}

const orphanCases = runCases.filter((c) => !allEntryIds.has(c));

const usedCmdNames = new Set();
const missingBackend = [];
for (const { method, cmd } of apiCmds) {
  if (!usedApiMethods.has(method)) continue;
  usedCmdNames.add(cmd);
  if (!handlers.includes(cmd)) missingBackend.push({ method, cmd });
}
const unusedApi = apiCmds.filter((a) => !usedApiMethods.has(a.method));
const orphanHandlers = handlers.filter((h) => !usedCmdNames.has(h));
const undefinedHandlers = handlers.filter((h) => !definedCmds.has(h));

// ---------- 7. 输出 ----------
const result = {
  generatedAt: new Date().toISOString(),
  counts: {
    menuEntries: [...allEntryIds.keys()].length,
    runCommandCases: runCases.length,
    apiMethods: apiCmds.length,
    backendHandlers: handlers.length,
  },
  entries: Object.fromEntries([...allEntryIds]),
  runCommandCases: runCases,
  dispatchPrefixes,
  deadEntries,
  orphanCases,
  missingBackend,
  unusedApi,
  orphanHandlers,
  undefinedHandlers,
  apiMapping: apiCmds,
  usedApiMethods: [...usedApiMethods],
};
fs.mkdirSync(path.join(root, "docs/验收证据"), { recursive: true });
fs.writeFileSync(path.join(root, "docs/验收证据/audit_entries.json"), JSON.stringify(result, null, 2));

console.log(`=== 静态审计（${result.generatedAt}）===`);
console.log(`入口 id 总数: ${result.counts.menuEntries} | runCommand case: ${runCases.length} | api 方法: ${apiCmds.length} | 后端 handler: ${handlers.length}`);
console.log(`\n--- 死入口（可点击但无分支）: ${deadEntries.length} ---`);
for (const d of deadEntries) console.log(`  DEAD  ${d.id}  [${d.where}]  "${d.label}"`);
console.log(`\n--- runCommand 孤儿分支（无入口引用，仅记录）: ${orphanCases.length} ---`);
console.log("  " + orphanCases.join(", "));
console.log(`\n--- 前端已用命令缺后端 handler（运行期必报错）: ${missingBackend.length} ---`);
for (const x of missingBackend) console.log(`  MISS  api.${x.method} -> ${x.cmd}`);
console.log(`\n--- 未被前端调用的 api 方法（仅记录）: ${unusedApi.length} ---`);
for (const x of unusedApi) console.log(`  UNUSED api.${x.method} -> ${x.cmd}`);
console.log(`\n--- 后端孤儿命令（无前端调用，仅记录）: ${orphanHandlers.length} ---`);
console.log("  " + orphanHandlers.join(", "));
console.log(`\n--- 已注册但未定义的命令: ${undefinedHandlers.length} ---`);
console.log(undefinedHandlers.join(", ") || "(无)");
if (deadEntries.length || missingBackend.length || undefinedHandlers.length) process.exit(2);
