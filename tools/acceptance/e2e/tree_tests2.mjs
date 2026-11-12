/** TC-TREE 补充: 箭头展开表层、灰/绿图标对比、表/视图/函数右键菜单。 */
import { connect, shot, sleep, EVID, log } from "./harness.mjs";
import fs from "node:fs";

const results = [];
const rec = (id, verdict, note) => {
  results.push({ id, verdict, note });
  log(`[TREE2] ${id} ${verdict} ${note}`);
};
const { page } = await connect();

const iconFills = async (label) => {
  const node = page.locator(`.tree-node:has(.tree-label:text-is("${label}"))`).first();
  return node.locator(".tree-icon svg").evaluate((el) =>
    [...el.querySelectorAll("path,ellipse,rect,circle")]
      .map((x) => x.getAttribute("fill"))
      .filter((f) => f && f !== "none")
      .join(","),
  );
};

// navkit_accept: 已打开（绿）；springbatch: 未打开（应灰）
{
  const openC = await iconFills("navkit_accept").catch(() => "?");
  const closedC = await iconFills("springbatch").catch(() => "?");
  rec("TREE-03a", "INFO", `打开库图标色=[${openC}] 未打开库=[${closedC}]`);
  const green = /#(27A468|4ECB88|1F8F58)/.test(openC);
  const gray = /#(9|A|B|C|D)[0-9A-F]{5}/i.test(closedC) && !/#(27A468|4ECB88)/.test(closedC);
  rec("TREE-03b", green && gray ? "PASS" : "FAIL", `绿=${green} 灰=${gray}`);
  await shot(page, "TC-TREE-03_icon_compare");
}

// 用箭头展开 navkit_accept（避免双击双 toggle）
{
  const node = page.locator('.tree-node:has(.tree-label:text-is("navkit_accept"))').first();
  await node.locator(".tree-arrow").click();
  await sleep(1500);
  const labels = await page.locator(".tree-label").allTextContents();
  const hasTables = labels.some((t) => /t_pk_auto|t_big|t_types/.test(t));
  rec("TREE-01a", hasTables ? "PASS" : "FAIL", `展开后节点=${labels.length} 示例=${labels.slice(10, 22).join(",")}`);
  await shot(page, "TC-TREE-01_tables");
}

// 展开后的右键菜单（表/视图/函数/存储过程）
{
  const targets = [
    ["table", "t_pk_auto"],
    ["view", "v_types"],
    ["function", "fn_add10"],
    ["procedure", "sp_reset"],
  ];
  const menus = {};
  for (const [type, label] of targets) {
    const node = page.locator(`.tree-node:has(.tree-label:has-text("${label}"))`).first();
    if (!(await node.count())) {
      menus[type] = "(不可见)";
      continue;
    }
    await node.click({ button: "right" });
    await sleep(250);
    menus[type] = await page.locator(".menu-popup .menu-entry-label").allTextContents();
    await page.keyboard.press("Escape");
    await sleep(150);
  }
  rec("TREE-MENUS2", "INFO", JSON.stringify(menus));
  fs.writeFileSync(`${EVID}/tree_context_menus2.json`, JSON.stringify(menus, null, 2));
  await shot(page, "TC-TREE-05_table_menu");
}

// 表节点子层（字段/索引/外键/触发器）
{
  const node = page.locator('.tree-node:has(.tree-label:text-is("t_pk_auto"))').first();
  const arrow = node.locator(".tree-arrow");
  if (await arrow.count()) {
    await arrow.click();
    await sleep(1200);
    const labels = await page.locator(".tree-label").allTextContents();
    const i = labels.indexOf("t_pk_auto");
    rec("TREE-01b", "INFO", `t_pk_auto 子层=${labels.slice(i + 1, i + 6).join(",")}`);
  }
}

fs.writeFileSync(`${EVID}/tree_results2.json`, JSON.stringify(results, null, 2));
console.log(JSON.stringify(results, null, 2));
process.exit(0);
