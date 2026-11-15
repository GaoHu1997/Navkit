// 用真实组件 + 真实样式渲染表设计器，导出验收截图（整页，含浮层）
import fs from "node:fs";
import { designerPage, launchDesignerBrowser } from "../../tools/tests/table-designer-fixture.mjs";

const dir = "docs/design/shots";
const css = fs.readFileSync("src/styles/base.css", "utf8");
const browser = await launchDesignerBrowser();
const page = await designerPage(browser);
await page.evaluate(text => { const style = document.createElement("style"); style.textContent = text; document.head.prepend(style); }, css);
await page.setViewportSize({ width: 1440, height: 760 });
await page.waitForTimeout(200);

const shot = async name => { await page.screenshot({ path: `${dir}/${name}.png`, fullPage: true }); console.log("shot", name); };

await shot("impl-01-fields");

await page.getByRole("button", { name: "字段 id 扩展", exact: true }).click();
await page.waitForTimeout(150);
await shot("impl-02-extension");
await page.keyboard.press("Escape");

await page.getByRole("tab", { name: "索引", exact: true }).click();
await page.getByRole("button", { name: "索引 1 字段选择", exact: true }).click();
await page.waitForTimeout(150);
await shot("impl-03-indexes");
await page.keyboard.press("Escape");

await page.getByRole("tab", { name: "字段", exact: true }).click();
await page.getByRole("textbox", { name: "字段 id 注释", exact: true }).fill("主键");
await page.getByRole("tab", { name: "SQL 预览", exact: true }).click();
await page.waitForFunction(() => document.querySelector(".designer-sql")?.textContent.includes("backend_actual_plan"));
await page.waitForTimeout(200);
await shot("impl-04-sql");

await browser.close();
console.log("完成");
