// 数据源同步设置页（审定宿主：选项对话框 WebDAV 页签）独立浏览器验证。
// 不启动应用服务器、不访问 WebDAV 或真实 data；控制器行为由 tools/tests/datasource-sync.test.mjs 覆盖。
import assert from "node:assert/strict";
import { readFile, access } from "node:fs/promises";
import { build } from "esbuild";
import { chromium } from "playwright-core";
const executablePath = process.env.BROWSER_EXE || "C:/Program Files/Google/Chrome/Application/chrome.exe";
await access(executablePath);
const bundle = await build({
  stdin: {
    contents: `import { DatasourceSync } from "./src/core/datasourceSync"; import { createDatasourceSyncTab } from "./src/dialogs/DatasourceSyncTab"; import { openSettingsDialog } from "./src/dialogs/SettingsDialog"; window.Sync = DatasourceSync; window.createTab = createDatasourceSyncTab; window.openSettingsDialog = openSettingsDialog;`,
    resolveDir: process.cwd(), sourcefile: "sync-browser-entry.ts",
  },
  bundle: true, format: "iife", write: false, loader: { ".css": "empty" },
});
const browser = await chromium.launch({ executablePath, headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 960, height: 860 } });
  const errors = []; page.on("pageerror", e => errors.push(e.message));
  await page.route("http://localhost/**", route => route.fulfill({ contentType: "text/html", body: '<!doctype html><html><head><meta charset="utf-8"></head><body><div id="host"></div></body></html>' }));
  await page.goto("http://localhost/");
  await page.addStyleTag({ content: await readFile("src/styles/base.css", "utf8") });
  await page.addStyleTag({ content: await readFile("src/styles/settings-dialog.css", "utf8") });
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  await page.evaluate(async () => {
    window.calls = [];
    let config = { url: "https://example.test/dav", user: "test", auto_every: -1, remember_password: true, password_set: true, passphrase_set: true, headers_set: true, headers: "Authorization: Bearer saved-demo\nX-Client: NavKit", last_sync: null };
    const source = {
      getConfig: async () => config,
      setConfig: async input => { window.calls.push(["save", input]); return config = { ...config, ...input }; },
      test: async () => "WebDAV 测试通过",
      preview: async () => { throw new Error("本测试不触发预览"); },
      commit: async () => { throw new Error("本测试不触发合并"); },
      cancel: async () => undefined,
    };
    window.sync = new window.Sync(source, { ready: () => true, applied: () => {}, status: () => {} });
    await window.sync.start();
    window.tab = window.createTab(window.sync);
    document.getElementById("host").appendChild(window.tab.el);
  });

  const tab = page.locator(".datasource-sync-tab");
  await tab.waitFor();
  // 1) 字段集合与 19 字段清单一致（sync_* 六个）
  const keys = await tab.locator("[data-sync-field]").evaluateAll(els => els.map(el => el.dataset.syncField));
  assert.deepEqual(keys.sort(), ["auto_every", "headers", "passphrase", "password", "url", "user"], `同步页字段：${keys}`);
  // 2) 密码与口令不回显，请求头显示已保存内容。
  for (const key of ["password", "passphrase"]) {
    const el = tab.locator(`[data-sync-field="${key}"]`);
    assert.equal(await el.inputValue(), "", `${key} 不得回显`);
  }
  assert.match(await tab.locator('[data-sync-field="password"]').getAttribute("placeholder"), /已设置/);
  assert.match(await tab.locator('[data-sync-field="passphrase"]').getAttribute("placeholder"), /已设置/);
  assert.equal(await tab.locator('[data-sync-field="headers"]').inputValue(), "Authorization: Bearer saved-demo\nX-Client: NavKit");
  assert.equal(await tab.locator('.settings-sync-note').count(), 0);
  // 3) 已保存值正确装配
  assert.equal(await tab.locator('[data-sync-field="url"]').inputValue(), "https://example.test/dav");
  assert.equal(await tab.locator('[data-sync-field="user"]').inputValue(), "test");
  assert.equal(await tab.locator('[data-sync-field="auto_every"]').inputValue(), "-1");
  // 4) 地址为空：saveIfNeeded 拒绝且标记 aria-invalid、给出可关联的帮助
  await tab.locator('[data-sync-field="url"]').fill("");
  const rejected = await page.evaluate(async () => {
    try { await window.tab.saveIfNeeded(); return null; } catch (e) { return String(e); }
  });
  assert(rejected && /WebDAV/.test(rejected), `空地址必须拒绝保存：${rejected}`);
  assert.equal(await tab.locator('[data-sync-field="url"]').getAttribute("aria-invalid"), "true");
  const describedBy = await tab.locator('[data-sync-field="url"]').getAttribute("aria-describedby");
  assert(describedBy && await page.locator(`#${describedBy}`).count() === 1, "地址字段应关联帮助文本");
  // 5) 留空秘密不进入请求：只提交显式填写的值
  await tab.locator('[data-sync-field="url"]').fill("https://example.test/dav");
  await page.evaluate(async () => { await window.tab.saveIfNeeded(); });
  const saved = await page.evaluate(() => window.calls.find(c => c[0] === "save")?.[1]);
  assert(saved, "应提交一次保存");
  assert(!("password" in saved) && !("passphrase" in saved) && !("headers" in saved), `留空秘密不得提交：${JSON.stringify(saved)}`);
  assert.equal(saved.url, "https://example.test/dav");
  // 6) 填写秘密时显式提交
  await tab.locator('[data-sync-field="password"]').fill("secret-pass");
  await tab.locator('[data-sync-field="headers"]').fill("Authorization: Bearer changed-demo\nX-Client: Test");
  await page.evaluate(async () => { await window.tab.saveIfNeeded(); });
  const saved2 = await page.evaluate(() => window.calls.filter(c => c[0] === "save").at(-1)?.[1]);
  assert.equal(saved2.password, "secret-pass");
  assert.equal(saved2.headers, "Authorization: Bearer changed-demo\nX-Client: Test");
  assert.equal(await tab.locator('[data-sync-field="headers"]').inputValue(), saved2.headers);
  // 7) 窄屏无横向溢出
  await page.setViewportSize({ width: 375, height: 700 });
  const narrow = await tab.boundingBox();
  assert(narrow.x >= 0 && narrow.x + narrow.width <= 375, `窄屏应贴合视口：${JSON.stringify(narrow)}`);
  assert.equal(await tab.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true, "窄屏不得横向溢出");
  await page.screenshot({ path: "tools/tests/artifacts/datasource-sync-tab.png", fullPage: true });
  // 8) dispose 清理：秘密输入被清空
  await page.evaluate(() => window.tab.dispose());
  assert.equal(await tab.locator('[data-sync-field="password"]').inputValue(), "", "dispose 应清除秘密输入");
  assert.equal(await tab.locator('[data-sync-field="headers"]').inputValue(), "", "dispose 应清除请求头输入");
  // 9) 实际设置窗口：WebDAV 控件加宽到 320px，常规页保留 240px，窄屏改为单列。
  await page.setViewportSize({ width: 1042, height: 790 });
  await page.evaluate(() => {
    window.dialog = window.openSettingsDialog({
      font_family: "Microsoft YaHei UI", font_size: 12, grid_font_size: 12,
      rows_per_page: 1000, query_timeout_secs: 30, max_recent: 10, auto_complete: true, confirm_on_close: true,
      sql_font_family: "Consolas", sql_font_size: 13, theme: "light", show_system_tray: true,
    }, () => {}, window.sync, "sync");
  });
  const dialog = page.locator('.dialog.settings-dialog'); await dialog.waitFor();
  const headerField = dialog.locator('[data-sync-field="headers"]');
  await headerField.waitFor({ state: "visible" });
  assert.equal(await headerField.inputValue(), saved2.headers, "重新打开应回显已保存请求头");
  for (const key of ["url", "user", "password", "passphrase", "headers", "auto_every"]) {
    const box = await dialog.locator(`[data-sync-field="${key}"]`).boundingBox();
    assert.equal(Math.round(box.width), 320, `${key} 控件应为 320px`);
  }
  assert.equal(await dialog.locator('.settings-sync-note').count(), 0);
  await dialog.screenshot({ path: "tools/tests/artifacts/webdav-settings-wide.png" });
  await dialog.locator('[data-settings-tab="font"]').click();
  assert.equal(Math.round((await dialog.locator('[data-settings-field="font_family"]').boundingBox()).width), 240);
  await dialog.locator('[data-settings-tab="sync"]').click();
  for (const width of [760, 640, 375]) {
    await page.setViewportSize({ width, height: 790 });
    assert.equal(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true, `窗口 ${width}px 无横向溢出`);
    const fields = await dialog.locator('.settings-sync-panel .settings-field').evaluateAll(els => els.map(el => getComputedStyle(el).gridTemplateColumns.split(" ").length));
    assert(fields.every(columns => columns === 1), `${width}px 应使用单列布局`);
  }
  await page.evaluate(() => window.dialog.close());
  assert.deepEqual(errors, []);
  console.log("PASS: 请求头回显及保存后重开、密码与口令不回显、未修改请求头不提交、底部说明移除、WebDAV 320px/其他页 240px、760/640/375px 单列无溢出、关闭清理");
} finally { await browser.close(); }
