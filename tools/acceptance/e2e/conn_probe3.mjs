/** 探针3: 选择器内容转储 → MySQL 连接对话框 DOM 转储。 */
import { connect, shot, sleep } from "./harness.mjs";

const { page } = await connect();
// 若有遗留对话框先关闭
if (await page.locator(".dialog").count()) {
  await page.locator(".dialog .dialog-title-close").last().click().catch(() => {});
  await sleep(200);
}
await page.click('.menubar-item:text-is("文件")');
await sleep(150);
await page.click('.menu-popup .menu-entry:has-text("新建连接")');
await sleep(400);
const dlg = page.locator(".dialog").last();
console.log("== 选择器 ==");
console.log("分类:", JSON.stringify(await dlg.locator(".conn-picker-cat").allTextContents()));
console.log("卡片:", JSON.stringify(await dlg.locator(".conn-kind-name").allTextContents()));
console.log("视图切换:", JSON.stringify(await dlg.locator(".conn-picker-viewbtn").allTextContents()));
console.log("搜索框:", await dlg.locator(".conn-picker-search input").count());
await shot(page, "conn_picker_probe");

await dlg.locator('.conn-kind-card:has-text("MySQL")').first().click();
await sleep(200);
await dlg.locator('button:has-text("下一步"), .btn:has-text("下一步")').click().catch(() => {});
await sleep(500);

const dlg2 = page.locator(".dialog").last();
console.log("== 连接对话框 ==");
console.log("标题:", await dlg2.locator(".dialog-title-text").textContent().catch(() => "?"));
const fields = await dlg2.locator(".field").all();
for (const f of fields) {
  const label = await f.locator(".field-label").textContent().catch(() => "");
  const info = await f.locator(".field-control").evaluate((el) => {
    const c = el.querySelector("input,select,textarea,button") ?? el;
    return `${c.tagName.toLowerCase()}[${c.type ?? ""}] value=${(c.value ?? c.textContent ?? "").slice(0, 40)}`;
  }).catch(() => "?");
  console.log(`field "${label}" -> ${info}`);
}
console.log("页签:", JSON.stringify(await dlg2.locator(".tabs-tab").allTextContents()));
console.log("checkbox 文案:", JSON.stringify(await dlg2.locator(".checkbox").allTextContents()));
console.log("按钮:", JSON.stringify(await dlg2.locator(".dialog-footer button, .dialog-footer .btn").allTextContents()));
await shot(page, "conn_dialog_mysql_probe");
process.exit(0);
