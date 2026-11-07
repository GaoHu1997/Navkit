/** 检查设计屏的布局问题：横向溢出、被裁切、文字重叠 */
import { chromium } from "playwright-core";
import { existsSync } from "node:fs";

const CHROME = [
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
].find((p) => existsSync(p));

const browser = await chromium.launch({ executablePath: CHROME, headless: true });
const page = await browser.newPage({ viewport: { width: 1680, height: 1000 } });
await page.goto(process.env.GALLERY_URL || "http://127.0.0.1:5199/gallery.html", { waitUntil: "networkidle" });
await page.evaluate(() => {
  document.body.classList.add("export");
  document.documentElement.style.setProperty("--frame-zoom", "1");
});
await page.waitForTimeout(400);

const report = await page.evaluate(() => {
  const out = [];
  const screens = [...document.querySelectorAll(".screen")].slice(36);
  for (const s of screens) {
    const frame = s.querySelector(".screen-frame");
    const fr = frame.getBoundingClientRect();
    const issues = [];
    if (frame.scrollWidth > frame.clientWidth + 1) issues.push(`frame 横向溢出 ${frame.scrollWidth - frame.clientWidth}px`);
    if (frame.scrollHeight > frame.clientHeight + 1) issues.push(`frame 纵向溢出 ${frame.scrollHeight - frame.clientHeight}px`);
    // 检查每个后代是否超出 frame 边界
    for (const el of frame.querySelectorAll("*")) {
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) continue;
      if (r.right > fr.right + 1 || r.bottom > fr.bottom + 1 || r.left < fr.left - 1) {
        const tag = el.tagName.toLowerCase() + (el.className && typeof el.className === "string" ? "." + el.className.split(" ").filter(Boolean).join(".") : "");
        issues.push(`越界 ${tag} [${Math.round(r.left - fr.left)},${Math.round(r.top - fr.top)} ${Math.round(r.width)}x${Math.round(r.height)}] "${(el.textContent || "").trim().slice(0, 24)}"`);
      }
    }
    out.push({ title: s.dataset.title, issues: [...new Set(issues)].slice(0, 8) });
  }
  return out;
});

for (const r of report) {
  console.log(`\n## ${r.title}`);
  if (!r.issues.length) console.log("  OK");
  else r.issues.forEach((i) => console.log("  - " + i));
}
await browser.close();
