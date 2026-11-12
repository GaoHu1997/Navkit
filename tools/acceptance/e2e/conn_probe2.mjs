/** 探针2: 选择器 → MySQL 连接对话框 DOM 转储。 */
import { connect, shot, sleep } from "./harness.mjs";

const { page } = await connect();
await page.click('.menubar-item:text-is("文件")');
await sleep(150);
await page.click('.menu-popup .menu-entry:has-text("新建连接")');
await sleep(400);
const dlg = page.locator(".dialog").last();
console.log("== 选择器结构 ==");
console.log("分类:", JSON.stringify(await dlg.locator(".conn-picker-cat").allTextContents()));
console.log("卡片:", JSON.stringify(await dlg.locator(".conn-kind-name").allTextContents()));
console.log("工具栏按钮:", JSON.stringify(await dlg.locator(".conn-picker-toolbar button, .conn-picker-toolbar .btn").allTextContents()));
await shot(page, "conn_picker_probe");
// 选 MySQL 卡片
await dlg.locator(".conn-kind-card:has-text('MySQL')").first().click();
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
console.log("复选框:", JSON.stringify(await dlg2.locator(".checkbox label, .checkbox span, label.checkbox").allTextContents()));
console.log("按钮:", JSON.stringify(await dlg2.locator(".dialog-footer button, .dialog-footer .btn").allTextContents()));
await shot(page, "conn_dialog_mysql_probe");
process.exit(0);
