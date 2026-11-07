// 独立浏览器验证，不启动应用服务器、不访问 WebDAV 或真实 data。
import assert from "node:assert/strict";
import { readFile, access, mkdir } from "node:fs/promises";
import { build } from "esbuild";
import { chromium } from "playwright-core";
const executablePath = process.env.BROWSER_EXE || "C:/Program Files/Google/Chrome/Application/chrome.exe";
await access(executablePath);
const bundle = await build({ stdin: { contents: `import { DatasourceSync } from "./src/core/datasourceSync"; import { openDatasourceSyncDialog } from "./src/dialogs/DatasourceSyncDialog"; window.openSync = openDatasourceSyncDialog; window.Sync = DatasourceSync;`, resolveDir: process.cwd(), sourcefile: "sync-browser-entry.ts" }, bundle: true, format: "iife", write: false });
const browser = await chromium.launch({ executablePath, headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 960, height: 860 } });
  const errors = []; page.on("pageerror", e => errors.push(e.message));
  await page.route("http://localhost/**", route => route.fulfill({ contentType: "text/html", body: '<!doctype html><html><head><meta charset="utf-8"></head><body><button id="trigger">数据源同步</button></body></html>' }));
  await page.goto("http://localhost/");
  await page.addStyleTag({ content: await readFile("src/styles/base.css", "utf8") });
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  await page.evaluate(async () => {
    window.calls = []; window.applied = [];
    const config = { url: "https://example.test/dav", user: "test", auto_every: -1, remember_password: true, password_set: true, passphrase_set: true, headers_set: true, last_sync: null };
    const preview = () => ({ plan_id: "plan-1", conflicts: [{ key: "id:1", label: '<img src=x onerror="window.injected=true">', local_summary: "本机数据源已修改", remote_summary: "远端修改同一数据源" }], summary: { added: 0, updated: 1, deleted: 0, organization: true }, warnings: ["首次接入已有远端，请手动确认"], remote_exists: true, has_changes: true, requires_confirmation: true });
    const source = {
      getConfig: async () => config,
      setConfig: async input => { window.calls.push(["save", input]); return { ...config, ...input }; },
      test: async () => "WebDAV 测试通过",
      preview: async () => { window.calls.push(["preview"]); if (window.deferPreview) await new Promise(resolve => window.resolvePreview = resolve); return preview(); },
      commit: async (...args) => { window.calls.push(["commit", ...args]); return { connections: [], groups: [], changed_ids: [], deleted_ids: [], warnings: window.partialResult ? ["基线保存失败，请恢复"] : [], last_sync: window.partialResult ? "" : "2026-11-06T10:00:00Z", recovery_required: !!window.partialResult }; },
      cancel: async id => window.calls.push(["cancel", id]),
    };
    window.sync = new window.Sync(source, { ready: () => true, applied: value => window.applied.push(value), status: () => {} });
    await window.sync.start();
    document.getElementById("trigger").focus(); window.dialog = window.openSync(window.sync);
  });
  const dialog = page.getByRole("dialog", { name: "数据源同步 · WebDAV" });
  await page.waitForFunction(() => document.querySelector("[role=dialog]")?.getAttribute("aria-busy") === "false");
  assert.equal(await dialog.getByLabel("WebDAV 密码", { exact: true }).inputValue(), "");
  assert.equal(await dialog.getByLabel("同步加密口令", { exact: true }).inputValue(), "");
  assert.equal(await dialog.getByLabel("自定义请求头", { exact: true }).inputValue(), "");
  assert.equal(await page.locator("#trigger").evaluate(el => el.inert || el.parentElement.inert), true);
  await dialog.getByRole("button", { name: "关闭", exact: true }).focus(); await page.keyboard.press("Tab");
  assert.equal(await dialog.getByLabel("WebDAV 地址", { exact: true }).evaluate(el => document.activeElement === el), true);
  await page.keyboard.press("Shift+Tab"); assert.equal(await dialog.getByRole("button", { name: "关闭", exact: true }).evaluate(el => document.activeElement === el), true);
  await dialog.getByLabel("WebDAV 地址", { exact: true }).fill("");
  await dialog.getByRole("button", { name: "保存配置", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("[role=dialog]")?.getAttribute("aria-busy") === "false");
  assert.match(await dialog.getByRole("alert").innerText(), /请填写 WebDAV/);
  assert.equal(await dialog.getByLabel("WebDAV 地址", { exact: true }).getAttribute("aria-invalid"), "true");
  assert.equal(await dialog.getByLabel("WebDAV 地址", { exact: true }).evaluate(el => el.getAttribute("aria-describedby").split(" ").includes(document.querySelector("[role=alert]").id)), true);
  await dialog.getByLabel("WebDAV 地址", { exact: true }).fill("https://example.test/dav");
  await page.evaluate(() => window.deferPreview = true);
  await dialog.getByRole("button", { name: "预览同步", exact: true }).click();
  assert.equal(await dialog.getByRole("button", { name: "关闭", exact: true }).isDisabled(), true);
  await page.keyboard.press("Escape"); assert.equal(await dialog.count(), 1);
  await page.waitForFunction(() => !!window.resolvePreview); await page.evaluate(() => { window.deferPreview = false; window.resolvePreview(); });
  await page.waitForFunction(() => document.querySelector("[role=dialog]")?.getAttribute("aria-busy") === "false");
  assert.equal(await dialog.getByRole("button", { name: "确认合并", exact: true }).isDisabled(), true);
  assert.equal(await dialog.locator("fieldset img").count(), 0); assert.equal(await page.evaluate(() => !!window.injected), false);
  await dialog.getByLabel("冲突处理：", { exact: true }).selectOption("remote");
  assert.equal(await dialog.getByRole("button", { name: "确认合并", exact: true }).isDisabled(), true);
  await dialog.getByLabel(/我已核对预览和冲突选择/).check();
  assert.equal(await dialog.getByRole("button", { name: "确认合并", exact: true }).isEnabled(), true);
  await mkdir("tools/tests/artifacts", { recursive: true }); await page.screenshot({ path: "tools/tests/artifacts/datasource-sync.png" });
  await page.setViewportSize({ width: 375, height: 700 });
  const narrow = await dialog.boundingBox(); assert(narrow.x >= 0 && narrow.x + narrow.width <= 375);
  assert.equal(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true);
  assert.equal(await dialog.locator(".dialog-body").evaluate(el => el.scrollWidth <= el.clientWidth + 1), true);
  await page.screenshot({ path: "tools/tests/artifacts/datasource-sync-narrow.png" });
  await page.setViewportSize({ width: 960, height: 860 });
  await dialog.getByLabel("用户名", { exact: true }).fill("changed");
  assert.equal(await dialog.locator(".datasource-sync-preview").isVisible(), false);
  assert.equal(await dialog.getByRole("button", { name: "确认合并", exact: true }).isDisabled(), true);
  await dialog.getByRole("button", { name: "预览同步", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("[role=dialog]")?.getAttribute("aria-busy") === "false");
  await dialog.getByLabel("冲突处理：", { exact: true }).selectOption("local"); await dialog.getByLabel(/我已核对预览和冲突选择/).check();
  await dialog.getByRole("button", { name: "确认合并", exact: true }).click();
  await page.waitForFunction(() => window.applied.length === 1);
  assert.deepEqual(await page.evaluate(() => window.calls.find(call => call[0] === "commit")), ["commit", "plan-1", { "id:1": "local" }, false]);
  await page.keyboard.press("Escape"); await page.waitForFunction(() => !document.querySelector("[role=dialog]"));
  assert.equal(await page.locator("#trigger").evaluate(el => document.activeElement === el), true);
  assert.equal(await page.locator("#trigger").evaluate(el => el.inert), false);
  await page.evaluate(() => { window.partialResult = true; window.dialog = window.openSync(window.sync); });
  await page.waitForFunction(() => document.querySelector("[role=dialog]")?.getAttribute("aria-busy") === "false");
  await dialog.getByRole("button", { name: "预览同步", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("[role=dialog]")?.getAttribute("aria-busy") === "false");
  await dialog.getByLabel("冲突处理：", { exact: true }).selectOption("remote");
  await dialog.getByLabel(/我已核对预览和冲突选择/).check();
  await dialog.getByRole("button", { name: "确认合并", exact: true }).click();
  await page.waitForFunction(() => window.applied.length === 2 && document.querySelector("[role=dialog]")?.getAttribute("aria-busy") === "false");
  assert.match(await dialog.innerText(), /同步尚未完成.*恢复/);
  assert(!((await dialog.innerText()).includes("数据源同步完成。")));
  assert.equal(await page.evaluate(() => window.applied[1].recovery_required), true);
  await dialog.getByRole("button", { name: "关闭", exact: true }).click();
  assert.deepEqual(errors, []);
  console.log("PASS: labels, secret non-echo, focus trap/restore, background inert, busy Escape guard, unselected conflicts, explicit confirmation, dirty-plan invalidation, XSS-safe summaries, commit and cancel lifecycle");
} finally { await browser.close(); }
