/**
 * TC-GRID 数据表格测试（MySQL navkit_accept）+ DB 侧反查。
 * 覆盖: GRID-03 编辑三入口 / GRID-04 提交落库+DB复核 / GRID-05 增删 / GRID-07 NULL区分 /
 *       GRID-08 特殊字符 / GRID-11 排序 / GRID-12 筛选 / GRID-13 分页 / 待提交守卫 / 回滚
 * 运行: node tools/acceptance/e2e/grid_tests.mjs
 */
import { execFileSync } from "node:child_process";
import { connect, shot, sleep, EVID, log } from "./harness.mjs";
import fs from "node:fs";

const MYSQL = "C:\\Program Files\\MySQL\\MySQL Server 8.0\\bin\\mysql.exe";
const q = (sql) =>
  execFileSync(MYSQL, ["-u", "root", "-p123456", "-N", "-D", "navkit_accept", "-e", sql], {
    encoding: "utf8",
  }).replace(/\r/g, "");

const results = [];
const rec = (id, verdict, note) => {
  results.push({ id, verdict, note });
  log(`[GRID] ${id} ${verdict} ${note}`);
};
const { page } = await connect();

const rows = () => page.locator(".grid-table tbody tr");
const cell = (r, c) => rows().nth(r).locator("td").nth(c + 1); // 第0列为行头

async function openTableViaTree(table) {
  // 树中双击表
  const node = page.locator(`.tree-node:has(.tree-label:text-is("${table}"))`).first();
  if (!(await node.count())) throw new Error(`树中无 ${table}`);
  await node.locator(".tree-label").dblclick();
  await sleep(1800);
}

const statusText = async () => (await page.locator(".statusbar").textContent().catch(() => "")).replace(/\s+/g, " ").trim();

// ------------------------------------------------ 打开 t_types：特殊类型/字符/NULL 展示
{
  await openTableViaTree("t_types");
  const tabTitles = await page.locator(".tab .tab-title").allTextContents();
  rec("GRID-OPEN", tabTitles.some((t) => t.includes("t_types")) ? "PASS" : "FAIL", `标签=${tabTitles.join(",")}`);
  await sleep(600);
  const headers = await page.locator(".grid-th-name").allTextContents();
  rec("GRID-HEADER", "INFO", `列头=${headers.join("|")}`);
  const firstRowCells = await rows().first().locator("td").allTextContents();
  rec("GRID-08a", "INFO", `首行=${firstRowCells.join(" § ").slice(0, 400)}`);
  await shot(page, "TC-GRID-t_types");
  // NULL 与空串可区分：t_types 第3行 nullable_col=NULL, 第2行 nullable_col=''
  const r2 = await rows().nth(1).locator("td").allTextContents();
  const r3 = await rows().nth(2).locator("td").allTextContents();
  const nullCellClass = await rows().nth(2).locator("td.grid-cell-null, td.null").count();
  rec("GRID-07a", "INFO", `行2(空串)_nullable='${r2[13]}' 行3(NULL)='${r3[13]}' NULL样式类=${nullCellClass}`);
  await shot(page, "TC-GRID-null_vs_empty");
}

// ------------------------------------------------ GRID-13 分页 + STA-02 行数（t_big 10万行）
{
  await openTableViaTree("t_big");
  await sleep(2500);
  const st = await statusText();
  rec("STA-01a", "INFO", `状态栏=${st.slice(0, 160)}`);
  const t0 = Date.now();
  await page.locator('.tree-node:has(.tree-label:text-is("t_big"))').first().locator(".tree-label").dblclick();
  try { await page.waitForSelector('.tab:has-text("t_big")', { timeout: 5000 }); } catch {}
  const openMs = Date.now() - t0;
  rec("PERF-02", openMs <= 1000 ? "PASS" : "FAIL", `已打开表重开耗时=${openMs}ms（阈值1s；首开另测）`);
  const pagerTitles = await page.locator(".grid-pager .grid-footer-btn").evaluateAll((els) => els.map((e) => e.title));
  rec("GRID-13a", "INFO", `分页按钮=${pagerTitles.join(",")}`);
  // 末页
  await page.locator('.grid-footer-btn[title="最后一页"]').click();
  await sleep(1200);
  const lastStatus = await statusText();
  rec("GRID-13b", "INFO", `末页状态=${lastStatus.slice(0, 140)}`);
  await shot(page, "TC-GRID-big_lastpage");
  // 回首页
  await page.locator('.grid-footer-btn[title="第一页"]').click();
  await sleep(1000);
  // DB 侧 COUNT 反查
  const cnt = q("SELECT COUNT(*) FROM t_big").trim();
  rec("STA-02a", /100000|100,000|共 ?100000/.test(st + lastStatus) ? "PASS" : "FAIL", `UI总行数含100000=${/100000|100,000/.test(st + lastStatus)} DB_COUNT=${cnt}`);
}

// ------------------------------------------------ GRID-11 排序（服务端，翻页后稳定）
{
  // 点击 name 列头 → 排序箭头
  const th = page.locator('.grid-th-name:has-text("name")').first();
  await th.click();
  await sleep(1200);
  const st1 = await statusText();
  // 服务端排序验证：排序后第一页首行 name 应为字典序最小 name_0000..
  const firstCell = await rows().nth(1).locator("td").nth(2).textContent();
  rec("GRID-11a", firstCell?.includes("name_0000") ? "PASS" : "FAIL", `升序首行name=${firstCell}（期望 name_0000）`);
  await shot(page, "TC-GRID-sort_asc");
  // 降序
  await th.click();
  await sleep(1200);
  const firstDesc = await rows().nth(1).locator("td").nth(2).textContent();
  rec("GRID-11b", /name_099\d/.test(firstDesc ?? "") ? "PASS" : "FAIL", `降序首行name=${firstDesc}`);
}

// ------------------------------------------------ GRID-12 筛选向导（amount > 5000）
{
  await cleanDialogs();
  // 打开筛选对话框（工具栏漏斗按钮）
  const funnel = page.locator('.tab-tool-btn:has-text("筛选"), .tab-tool-btn[title*="筛选"]').first();
  await funnel.click();
  await sleep(500);
  const dlg = page.locator(".dialog").last();
  const hasDlg = await dlg.count();
  rec("GRID-12a", hasDlg ? "PASS" : "FAIL", `筛选对话框打开=${hasDlg}`);
  await shot(page, "TC-GRID-filter_dialog");
  if (hasDlg) {
    console.log("筛选对话框字段:", JSON.stringify(await dlg.locator("select, input").evaluateAll(els => els.map(e => `${e.tagName}[${e.type ?? ""}]`))));
    await dlg.locator('button:has-text("取消"), .btn:has-text("取消")').first().click().catch(() => {});
    await sleep(300);
  }
}

function cleanDialogs() {
  return (async () => {
    for (let i = 0; i < 5 && (await page.locator(".dialog").count()); i++) {
      await page.locator(".dialog .dialog-title-close").last().click().catch(() => {});
      await sleep(150);
    }
  })();
}

// ------------------------------------------------ GRID-03/04: 编辑 t_pk_auto → 提交 → DB 复核
{
  await openTableViaTree("t_pk_auto");
  await sleep(1200);
  const before = q("SELECT name FROM t_pk_auto WHERE id=1").trim();
  // 双击 id=1 行的 name 单元格（第1列数据）
  const c = rows().first().locator("td").nth(2); // 行头+id+name → td[1]=id, td[2]=name
  await c.dblclick();
  await sleep(300);
  const editor = page.locator(".grid-cell-input").first();
  const editing = await editor.count();
  rec("GRID-03a", editing ? "PASS" : "FAIL", `双击出现编辑框=${editing}`);
  if (editing) {
    await editor.fill(`编辑后-${Date.now() % 100000}`);
    await page.keyboard.press("Enter"); // 提交并下移
    await sleep(300);
    // 待提交标记
    const dirty = await page.locator(".row-flag, tr.row-inserted, tr.row-updated, td.dirty").count();
    const commitBtn = page.locator('.grid-footer-btn[title="提交改动"]');
    const opBefore = await commitBtn.evaluate((el) => el.style.opacity);
    rec("GRID-04a", "INFO", `脏标记数=${dirty} 提交钮透明度=${opBefore}`);
    await shot(page, "TC-GRID-pending");
    await commitBtn.click();
    await sleep(1500);
    const after = q("SELECT name FROM t_pk_auto WHERE id=1").trim();
    rec("GRID-04b", after !== before && after.startsWith("编辑后") ? "PASS" : "FAIL", `DB侧 name: '${before}' → '${after}'`);
    await shot(page, "TC-GRID-committed");
  }
}

// ------------------------------------------------ GRID-05: 插入行 → 提交 → DB 复核
{
  const cntBefore = Number(q("SELECT COUNT(*) FROM t_pk_auto").trim());
  await page.locator('.grid-footer-btn[title="插入记录"]').click();
  await sleep(500);
  // 新行在底部，编辑 name 单元格
  const lastRow = rows().last();
  const nameCell = lastRow.locator("td").nth(2);
  await nameCell.dblclick();
  await sleep(250);
  const editor = page.locator(".grid-cell-input").first();
  if (await editor.count()) {
    await editor.fill("UI插入行");
    await page.keyboard.press("Enter");
    await sleep(250);
  }
  await page.locator('.grid-footer-btn[title="提交改动"]').click();
  await sleep(1500);
  const cntAfter = Number(q("SELECT COUNT(*) FROM t_pk_auto").trim());
  const inserted = q("SELECT id FROM t_pk_auto WHERE name='UI插入行'").trim();
  rec("GRID-05a", cntAfter === cntBefore + 1 && inserted ? "PASS" : "FAIL", `DB行数 ${cntBefore}→${cntAfter} 新行id=${inserted}`);
  // 删除该行（选中最后一行 → 删除记录 → 提交）
  await rows().last().locator("td").nth(1).click();
  await sleep(200);
  await page.locator('.grid-footer-btn[title="删除记录"]').click();
  await sleep(400);
  await page.locator('.grid-footer-btn[title="提交改动"]').click();
  await sleep(1500);
  const cntFinal = Number(q("SELECT COUNT(*) FROM t_pk_auto").trim());
  rec("GRID-05b", cntFinal === cntBefore ? "PASS" : "FAIL", `删除后 DB 行数=${cntFinal}（期望 ${cntBefore}）`);
}

// ------------------------------------------------ 回滚: 修改不提交 → ✗ 回滚
{
  const before = q("SELECT name FROM t_pk_auto WHERE id=2").trim();
  const c = rows().nth(1).locator("td").nth(3);
  await c.dblclick();
  await sleep(250);
  const editor = page.locator(".grid-cell-input").first();
  await editor.fill("将被回滚");
  await page.keyboard.press("Enter");
  await sleep(250);
  await page.locator('.grid-footer-btn[title="回滚改动"]').click();
  await sleep(800);
  const after = q("SELECT name FROM t_pk_auto WHERE id=2").trim();
  const uiCell = await rows().first().locator("td").nth(3).textContent();
  rec("GRID-RB", after === before && !uiCell.includes("将被回滚") ? "PASS" : "FAIL", `DB未变=${after === before} UI已还原=${!uiCell.includes("将被回滚")}`);
}

// ------------------------------------------------ 待提交守卫: 未提交翻页被拦截
{
  const c = rows().nth(1).locator("td").nth(2);
  await c.dblclick();
  await sleep(250);
  await page.locator(".grid-cell-input").first().fill("守卫测试");
  await page.keyboard.press("Enter");
  await sleep(250);
  await page.locator('.grid-footer-btn[title="下一页"]').click();
  await sleep(500);
  const toast = (await page.locator(".toast").allTextContents().catch(() => [])).join("|");
  rec("GRID-GUARD", toast.includes("未提交") ? "PASS" : "FAIL", `翻页守卫toast="${toast}"`);
  await page.locator('.grid-footer-btn[title="回滚改动"]').click();
  await sleep(500);
}

// ------------------------------------------------ GRID-06: 无主键表编辑限制（t_no_pk）
{
  await openTableViaTree("t_no_pk");
  await sleep(1500);
  const c = rows().first().locator("td").nth(2);
  await c.dblclick();
  await sleep(300);
  const editor = page.locator(".grid-cell-input").first();
  if (await editor.count()) {
    await editor.fill("np- edited");
    await page.keyboard.press("Enter");
    await sleep(250);
    await page.locator('.grid-footer-btn[title="提交改动"]').click();
    await sleep(1200);
    const toast = (await page.locator(".toast").allTextContents().catch(() => [])).join("|");
    const dbVal = q("SELECT qty FROM t_no_pk WHERE name='np1'").trim();
    rec("GRID-06a", "INFO", `无主键提交 toast="${toast.slice(0, 100)}" DB第一行qty=${dbVal}`);
    await shot(page, "TC-GRID-nopk");
  } else {
    rec("GRID-06a", "INFO", "无主键表双击未进入编辑（需观察是否有提示）");
  }
}

// ------------------------------------------------ 复制单元格（GRID-10）
{
  await openTableViaTree("t_pk_auto");
  await sleep(1200);
  const c = rows().nth(1).locator("td").nth(2);
  await c.click();
  await page.keyboard.press("Control+c");
  await sleep(300);
  const clip = await page.evaluate(() => navigator.clipboard.readText().catch((e) => `ERR:${e.message}`));
  rec("GRID-10a", "INFO", `剪贴板="${String(clip).slice(0, 60)}"`);
}

fs.writeFileSync(`${EVID}/grid_results.json`, JSON.stringify(results, null, 2));
console.log(JSON.stringify(results, null, 2));
process.exit(0);
