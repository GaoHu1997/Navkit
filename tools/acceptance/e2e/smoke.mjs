/**
 * 冒烟 + DOM 结构探针。
 * 前置: navkit.exe 已以 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 启动。
 * 用法: node tools/acceptance/e2e/smoke.mjs
 * 产出: docs/验收证据/smoke_*.png + smoke.json（各主区存在性与文本快照）
 */
import fs from "node:fs";
import { connect, shot, EVID, log, sleep } from "./harness.mjs";

const out = {};
const { browser, ctx, page } = await connect();

await page.waitForSelector(".app-shell", { timeout: 15000 });
await sleep(800);

out.url = page.url();
out.title = await page.title();
out.titlebar = await page.locator(".titlebar").count();
out.menubarItems = await page.locator(".menubar-item").allTextContents();
out.ribbonBtns = await page.locator(".ribbon-btn .ribbon-label").allTextContents();
out.ribbonDisabled = await page.locator(".ribbon-btn.disabled .ribbon-label").allTextContents();
out.tabs = await page.locator(".tab .tab-title").allTextContents();
out.treeText = (await page.locator(".tree-panel").textContent().catch(() => "")).slice(0, 400);
out.statusbar = (await page.locator(".statusbar").textContent().catch(() => "")).slice(0, 300);
out.emptyState = await page.locator(".grid-empty").textContent().catch(() => "(无)");

await shot(page, "smoke_startup");
fs.writeFileSync(`${EVID}/smoke.json`, JSON.stringify(out, null, 2));
log(`冒烟 OK url=${out.url} 菜单=${out.menubarItems.length} Ribbon=${out.ribbonBtns.length}`);
console.log(JSON.stringify(out, null, 2));
process.exit(0);
