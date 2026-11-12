/** 探针: 打开新建连接对话框并转储其 DOM 结构（供编写断言用）。 */
import { connect, shot, sleep } from "./harness.mjs";

const { page } = await connect();
await page.click('.menubar-item:text-is("文件")');
await sleep(150);
const fileMenu = await page.locator(".menu-popup .menu-entry-label").allTextContents();
console.log("文件菜单项:", JSON.stringify(fileMenu));
await page.click('.menu-popup .menu-entry:has-text("新建连接")');
await sleep(400);
console.log("dialog数:", await page.locator(".dialog").count());
const dlg = page.locator(".dialog").last();
console.log("标题:", await dlg.locator(".dialog-title-text").textContent().catch(() => "?"));
// 转储字段
const fields = await dlg.locator(".field").all();
for (const f of fields) {
  const label = await f.locator(".field-label").textContent().catch(() => "");
  const ctrlTag = await f.locator(".field-control").evaluate((el) => {
    const c = el.querySelector("input,select,textarea,button") ?? el;
    return `${c.tagName.toLowerCase()}[${c.type ?? ""}] value=${(c.value ?? c.textContent ?? "").slice(0, 40)}`;
  }).catch(() => "?");
  console.log(`field "${label}" -> ${ctrlTag}`);
}
console.log("页签:", JSON.stringify(await dlg.locator(".tabs-tab, .dialog-tabs .tab").allTextContents()));
console.log("按钮:", JSON.stringify(await dlg.locator(".dialog-footer button, .dialog-footer .btn").allTextContents()));
await shot(page, "conn_dialog_probe");
process.exit(0);
