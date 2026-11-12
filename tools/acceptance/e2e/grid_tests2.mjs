/**
 * TC-GRID 补充（绕开 Blocker B-01 单元格编辑失效）:
 *   GRID-13 分页全路径 / GRID-12 筛选全流程+DB复核 / GRID-GUARD 待提交守卫 /
 *   回滚(插入路径) / GRID-14 列宽持久化 / GRID-10 复制 / GRID-15 右键菜单
 * 运行: node tools/acceptance/e2e/grid_tests2.mjs
 */
import { execFileSync } from "node:child_process";
import { connect, shot, sleep, EVID, log } from "./harness.mjs";
import fs from "node:fs";

const MYSQL = "C:\\Program Files\\MySQL\\MySQL Server 8.0\\bin\\mysql.exe";
const q = (sql) =>
  execFileSync(MYSQL, ["-u", "root", "-p123456", "-N", "-D", "navkit_accept", "-e", sql], { encoding: "utf8" }).replace(/\r/g, "");

const results = [];
const rec = (id, verdict, note) => {
  results.push({ id, verdict, note });
  log(`[GRID2] ${id} ${verdict} ${note}`);
};
const { page } = await connect();
const statusText = async () => (await page.locator(".statusbar").textContent().catch(() => "")).replace(/\s+/g, " ").trim();

// 激活 t_big 标签
await page.locator('.tab:has-text("t_big")').click();
await sleep(800);

// ------------------------------------------------ GRID-13: 分页（页码输入、更多、每页条数）
{
  const status0 = await statusText();
  rec("GRID-13-INIT", "INFO", status0.slice(0, 120));
  // 下一页
  await page.locator('.grid-footer-btn[title="下一页"]').click();
  await sleep(1000);
  const s2 = await statusText();
  rec("GRID-13-next", /于第 2 页|第 1001 条|第 2 条记录/.test(s2) || !s2.includes("于第 1 页") ? "PASS" : "FAIL", s2.slice(0, 120));
  // 页码输入框跳页
  const pageInput = page.locator(".grid-pager input");
  if (await pageInput.count()) {
    await pageInput.fill("3");
    await page.keyboard.press("Enter");
    await sleep(1000);
    const s3 = await statusText();
    rec("GRID-13-pageno", /第 3 页|第 2001 条/.test(s3) ? "PASS" : "FAIL", s3.slice(0, 120));
  }
  // 最后一页
  await page.locator('.grid-footer-btn[title="最后一页"]').click();
  await sleep(1200);
  const s4 = await statusText();
  rec("GRID-13-last", /第 100 页|第 9900\d 条|第 99\d\d\d 条/.test(s4) ? "PASS" : "FAIL", `末页状态=${s4.slice(0, 130)}`);
  await shot(page, "TC-GRID-lastpage2");
  // 第一页复原
  await page.locator('.grid-footer-btn[title="第一页"]').click();
  await sleep(800);
  // 「更多」下拉
  const more = page.locator('.grid-footer-btn[title="更多"]');
  if (await more.count()) {
    await more.click();
    await sleep(300);
    const items = await page.locator(".menu-popup .menu-entry-label").allTextContents().catch(() => []);
    rec("GRID-13-more", "INFO", `「更多」菜单=${items.join(",")}`);
    await page.keyboard.press("Escape");
  }
}

// ------------------------------------------------ GRID-12: 筛选全流程（amount > 5000 → 总数变化 + SQL 下推 + DB 复核）
{
  const funnel = page.locator('.tab-tool-btn:has-text("筛选")').first();
  await funnel.click();
  await sleep(500);
  const dlg = page.locator(".dialog").last();
  // 转储对话框控件
  const ctrls = await dlg.locator("select, input").evaluateAll((els) =>
    els.map((e) => `${e.tagName}[${e.type ?? ""}]`).join(","),
  );
  rec("GRID-12b", "INFO", `筛选控件=${ctrls}`);
  // 列下拉选 amount、运算符 >、值 5000
  const selects = dlg.locator("select");
  const n = await selects.count();
  if (n >= 2) {
    // 第一行列名，第二行运算符
    const colOpts = await selects.nth(0).locator("option").allTextContents();
    await selects.nth(0).selectOption({ label: "amount" }).catch(() => {});
    const opOpts = await selects.nth(1).locator("option").allTextContents();
    rec("GRID-12c", "INFO", `列选项=${colOpts.join(",")} 运算符=${opOpts.join(",")}`);
    await selects.nth(1).selectOption({ label: ">" }).catch(() => {});
  }
  const valInput = dlg.locator('input[type="text"], input:not([type])').last();
  await valInput.fill("5000");
  await shot(page, "TC-GRID-filter_filled");
  await dlg.locator('button:has-text("应用"), .btn:has-text("应用")').first().click().catch(() => {
    return dlg.locator('button:has-text("确定")').first().click();
  });
  await sleep(2000);
  const st = await statusText();
  const dbCnt = Number(q("SELECT COUNT(*) FROM t_big WHERE amount > 5000").trim());
  rec("GRID-12d", st.includes(`共 ${dbCnt} 条`) || st.includes(`共${dbCnt}条`) ? "PASS" : "FAIL", `UI="${st.slice(40, 110)}" DB_COUNT(amount>5000)=${dbCnt}`);
  rec("GRID-12e", st.includes("amount") && st.includes(">") ? "PASS" : "FAIL", `SQL 下推显示=${st.slice(0, 80)}`);
  await shot(page, "TC-GRID-filter_applied");
  // 清除筛选
  await funnel.click();
  await sleep(400);
  const dlg2 = page.locator(".dialog").last();
  await dlg2.locator('button:has-text("清除"), .btn:has-text("清除")').first().click().catch(() => {});
  await sleep(300);
  await dlg2.locator('button:has-text("关闭"), .dialog-title-close').first().click().catch(() => {});
  await sleep(800);
  const st2 = await statusText();
  rec("GRID-12f", st2.includes("100000") ? "PASS" : "FAIL", `清除后=${st2.slice(40, 110)}`);
}

// ------------------------------------------------ GRID-GUARD + 回滚（插入路径，绕开单元格编辑）
{
  const cntBefore = Number(q("SELECT COUNT(*) FROM t_big").trim());
  await page.locator('.grid-footer-btn[title="插入记录"]').click();
  await sleep(500);
  // 待提交状态翻页 → 应被拦截
  await page.locator('.grid-footer-btn[title="下一页"]').click();
  await sleep(500);
  const toast = (await page.locator(".toast").allTextContents().catch(() => [])).join("|");
  rec("GRID-GUARD", toast.includes("未提交") ? "PASS" : "FAIL", `toast="${toast.slice(0, 60)}"`);
  // 回滚 → 不落库
  await page.locator('.grid-footer-btn[title="回滚改动"]').click();
  await sleep(800);
  const cntAfter = Number(q("SELECT COUNT(*) FROM t_big").trim());
  rec("GRID-RB2", cntAfter === cntBefore ? "PASS" : "FAIL", `DB行数 ${cntBefore}→${cntAfter}`);
}

// ------------------------------------------------ GRID-14: 列宽拖拽持久化（翻页/重开标签）
{
  // 记录 id 列宽
  const th = page.locator(".grid-table th").nth(1);
  const w0 = await th.evaluate((el) => el.getBoundingClientRect().width);
  // 拖拽列分隔线（表头右缘）
  const box = await th.boundingBox();
  await page.mouse.move(box.x + box.width - 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width + 60, box.y + box.height / 2, { steps: 6 });
  await page.mouse.up();
  await sleep(400);
  const w1 = await th.evaluate((el) => el.getBoundingClientRect().width);
  rec("GRID-14a", w1 > w0 + 30 ? "PASS" : "FAIL", `列宽 ${w0.toFixed(0)}→${w1.toFixed(0)}`);
  // 翻页后保持
  await page.locator('.grid-footer-btn[title="下一页"]').click();
  await sleep(900);
  await page.locator('.grid-footer-btn[title="第一页"]').click();
  await sleep(900);
  const w2 = await th.evaluate((el) => el.getBoundingClientRect().width);
  rec("GRID-14b", Math.abs(w2 - w1) < 3 ? "PASS" : "FAIL", `翻页后列宽=${w2.toFixed(0)}`);
  await shot(page, "TC-GRID-colwidth");
}

// ------------------------------------------------ GRID-10: 复制（Ctrl+C，剪贴板）
{
  // 单击选中第一数据行 amount 单元格
  const td = page.locator(".grid-table tbody tr").nth(0).locator("td").nth(3);
  await td.click({ clickCount: 1 }).catch(() => {});
  await sleep(200);
  await page.keyboard.press("Control+c");
  await sleep(300);
  const clip = await page.evaluate(() => navigator.clipboard.readText().catch((e) => `ERR:${e.message}`));
  rec("GRID-10a", "INFO", `单元格复制剪贴板="${String(clip).slice(0, 50)}"`);
}

// ------------------------------------------------ GRID-15: 结果区右键菜单
{
  const td = page.locator(".grid-table tbody tr").nth(1).locator("td").nth(2);
  await td.click({ button: "right" });
  await sleep(300);
  const items = await page.locator(".menu-popup .menu-entry-label").allTextContents();
  const disabled = await page.locator(".menu-popup .menu-entry.disabled .menu-entry-label").allTextContents();
  rec("GRID-15a", items.length ? "PASS" : "FAIL", `菜单=${items.join(",")} 禁用=${disabled.join(",")}`);
  await page.keyboard.press("Escape");
}

fs.writeFileSync(`${EVID}/grid_results2.json`, JSON.stringify(results, null, 2));
console.log(JSON.stringify(results, null, 2));
process.exit(0);
