/**
 * TC-TREE 连接树测试 + 建立 PG/SQLite 连接。
 * 覆盖: TREE-01 懒加载展开 / TREE-02 双击 / TREE-03 库图标颜色 / TREE-04 搜索 /
 *       TREE-05 单击选中 / TREE-06 F2 重命名 / TREE-09 分割条 / 上下文菜单 6 类
 * 运行: node tools/acceptance/e2e/tree_tests.mjs
 */
import { connect, shot, sleep, EVID, log } from "./harness.mjs";
import fs from "node:fs";

const results = [];
const rec = (id, verdict, note) => {
  results.push({ id, verdict, note });
  log(`[TREE] ${id} ${verdict} ${note}`);
};
const { page } = await connect();

async function cleanDialogs() {
  for (let i = 0; i < 5 && (await page.locator(".dialog").count()); i++) {
    await page.locator(".dialog .dialog-title-close").last().click().catch(() => {});
    await sleep(150);
  }
}

// ------------------------------------------------ 建立 PG(5433) 与 SQLite(testdata/accept.db) 连接
async function addConn(kind, name, fields, cat) {
  await cleanDialogs();
  await page.keyboard.press("Escape");
  await page.click('.menubar-item:text-is("文件")');
  await sleep(120);
  await page.click('.menu-popup .menu-entry:has-text("新建连接")');
  await sleep(350);
  const dlg = page.locator(".dialog").last();
  if (cat) {
    await dlg.locator(`.conn-picker-cat:has-text("${cat}")`).first().click().catch(() => {});
    await sleep(200);
  }
  await dlg.locator(`.conn-kind-card:has-text("${kind}")`).first().click();
  await sleep(150);
  await dlg.locator('button:has-text("下一步"), .btn:has-text("下一步")').click();
  await sleep(400);
  const d2 = page.locator(".dialog").last();
  await d2.locator('.field:has-text("连接名") input').fill(name);
  for (const [label, val] of Object.entries(fields)) {
    await d2.locator(`.field:has-text("${label}") input`).fill(val);
  }
  await d2.locator('button:has-text("确定"), .btn:has-text("确定")').click();
  await sleep(600);
  return page.locator(`.tree-label:has-text("${name}")`).count();
}

{
  const ok = await addConn("PostgreSQL", "PG-5433", { 主机: "localhost", 端口: "5433", 用户名: "postgres", 密码: "" }, "关系型数据库");
  rec("CONN-PG", ok ? "PASS" : "FAIL", `PG-5433 保存=${ok}`);
  const ok2 = await addConn("SQLite", "SQLite-accept", { 文件: "D:\\github\\tauri-navicat\\testdata\\accept.db" }, "轻量数据库");
  rec("CONN-SQLITE", ok2 ? "PASS" : "FAIL", `SQLite-accept 保存=${ok2}`);
  await shot(page, "TC-TREE-three_conns");
}

// ------------------------------------------------ TREE-05: 单击只选中不打开
{
  await cleanDialogs();
  const mysql = page.locator('.tree-label:has-text("MySQL-3306")').first();
  await mysql.click();
  await sleep(300);
  const selected = await page.locator(".tree-node.selected, .tree-node.active").count();
  const dbs = await page.locator('.tree-label:has-text("navkit_accept")').count();
  rec("TREE-05a", selected > 0 && dbs === 0 ? "PASS" : "FAIL", `单击选中节点=${selected} 未展开库=${dbs === 0}`);
}

// ------------------------------------------------ TREE-01/02: 双击打开 → 自动展开到库层（懒加载）
{
  const mysql = page.locator('.tree-label:has-text("MySQL-3306")').first();
  await mysql.dblclick();
  await sleep(2500);
  const dbs = await page.locator(".tree-label").allTextContents();
  const hasDbs = dbs.some((t) => t.includes("navkit_accept"));
  rec("TREE-02a", hasDbs ? "PASS" : "FAIL", `双击后库层=${dbs.slice(0, 10).join(",")}`);
  await shot(page, "TC-TREE-02_open_mysql");
  // 展开到表层（懒加载）
  await page.locator('.tree-label:has-text("navkit_accept")').first().dblclick();
  await sleep(2000);
  const objs = await page.locator(".tree-label").allTextContents();
  const hasTables = objs.some((t) => t.includes("t_pk_auto") || t.includes("t_big"));
  rec("TREE-01a", hasTables ? "PASS" : "FAIL", `navkit_accept 展开含表=${hasTables} 节点=${objs.length}`);
  await shot(page, "TC-TREE-01_expand_tables");
}

// ------------------------------------------------ TREE-03: 库节点图标颜色（打开=彩色/绿，关闭=灰）
{
  // 找一个未打开库（如 springbatch）与已打开库（navkit_accept）的图标 fill 颜色
  const iconColor = async (label) => {
    const node = page.locator(`.tree-node:has(.tree-label:text-is("${label}"))`).first();
    const svg = node.locator("svg").first();
    return svg.evaluate((el) => {
      const fills = [...el.querySelectorAll("[fill]")].map((x) => x.getAttribute("fill")).filter((f) => f && f !== "none");
      return fills.join(",");
    });
  };
  const openColor = await iconColor("navkit_accept").catch(() => "?");
  // 关闭 navkit_accept（折叠）→ 图标应变灰
  await page.locator('.tree-label:text-is("navkit_accept")').first().click({ clickCount: 2 }).catch(() => {});
  await sleep(600);
  // 重新记录
  const closedColor = await iconColor("navkit_accept").catch(() => "?");
  rec("TREE-03a", "INFO", `navkit_accept 打开态图标色=[${openColor}] 折叠态=[${closedColor}]`);
  await shot(page, "TC-TREE-03_db_icon_colors");
}

// ------------------------------------------------ TREE-04: 搜索过滤
{
  await cleanDialogs();
  const search = page.locator(".tree-search input, .tree-search-box input").first();
  if (!(await search.count())) {
    // 搜索框可能是 contenteditable 或自定义
    console.log("搜索框选择器未命中:", await page.locator(".tree-search").innerHTML().catch(() => "无.tree-search"));
  }
  await search.fill("accept");
  await sleep(600);
  const filtered = await page.locator(".tree-label").allTextContents();
  rec("TREE-04a", filtered.every((t) => /accept/i.test(t)) && filtered.length > 0 ? "PASS" : "FAIL", `过滤后=${filtered.join(",")}`);
  await shot(page, "TC-TREE-04_search");
  await search.fill("");
  await sleep(400);
  const restored = await page.locator(".tree-label").count();
  rec("TREE-04b", restored >= 3 ? "PASS" : "FAIL", `清空还原节点数=${restored}`);
}

// ------------------------------------------------ TREE-06: F2 重命名
{
  await cleanDialogs();
  const mysql = page.locator('.tree-label:has-text("MySQL-3306")').first();
  await mysql.click();
  await page.keyboard.press("F2");
  await sleep(500);
  const editing = await page.locator(".tree-node input, .tree-rename-input").count();
  rec("TREE-06a", editing > 0 ? "PASS" : "FAIL(缺口)", `F2 出现内联编辑=${editing > 0}（静态审计: 全前端无 rename 实现，预期为缺口）`);
  await page.keyboard.press("Escape");
}

// ------------------------------------------------ 右键菜单 6 类节点差异化
{
  await cleanDialogs();
  const menus = {};
  const targets = [
    ["connection", '.tree-node:has(.tree-label:has-text("MySQL-3306"))'],
    ["database", '.tree-node:has(.tree-label:has-text("navkit_accept"))'],
    ["table", '.tree-node:has(.tree-label:has-text("t_pk_auto"))'],
    ["view", '.tree-node:has(.tree-label:has-text("v_types"))'],
    ["function", '.tree-node:has(.tree-label:has-text("fn_add10"))'],
    ["procedure", '.tree-node:has(.tree-label:has-text("sp_reset"))'],
  ];
  for (const [type, sel] of targets) {
    const node = page.locator(sel).first();
    if (!(await node.count())) {
      menus[type] = "(节点不可见——需先展开)";
      continue;
    }
    await node.click({ button: "right" });
    await sleep(250);
    menus[type] = await page.locator(".menu-popup .menu-entry-label").allTextContents();
    await page.keyboard.press("Escape");
    await sleep(150);
  }
  rec("TREE-MENUS", "INFO", JSON.stringify(menus));
  fs.writeFileSync(`${EVID}/tree_context_menus.json`, JSON.stringify(menus, null, 2));
}

// ------------------------------------------------ TREE-09: 分割条拖拽
{
  await cleanDialogs();
  const sp = page.locator(".splitter").first();
  const before = await page.locator(".sidebar").first().evaluate((el) => el.getBoundingClientRect().width);
  const box = await sp.boundingBox();
  if (box) {
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 80, box.y + box.height / 2, { steps: 8 });
    await page.mouse.up();
    await sleep(300);
  }
  const after = await page.locator(".sidebar").first().evaluate((el) => el.getBoundingClientRect().width);
  rec("TREE-09a", Math.abs(after - before) > 40 ? "PASS" : "FAIL", `树宽 ${before}→${after}`);
  // 拖回
  const box2 = await sp.boundingBox();
  await page.mouse.move(box2.x + box2.width / 2, box2.y + box2.height / 2);
  await page.mouse.down();
  await page.mouse.move(box2.x + box2.width / 2 - 80, box2.y + box2.height / 2, { steps: 8 });
  await page.mouse.up();
}

fs.writeFileSync(`${EVID}/tree_results.json`, JSON.stringify(results, null, 2));
console.log(JSON.stringify(results, null, 2));
process.exit(0);
