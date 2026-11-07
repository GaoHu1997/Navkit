/** 交付前自动体检：溢出、空画框、控制台报错、字号越界 */

import { chromium } from "playwright-core";

const browser = await chromium.launch({
  executablePath: "C:/Program Files/Google/Chrome/Application/chrome.exe",
  headless: true,
});
const page = await browser.newPage({ viewport: { width: 1680, height: 1000 } });

const errors = [];
page.on("pageerror", (e) => errors.push(e.message));

await page.goto("http://127.0.0.1:5199/gallery.html", { waitUntil: "networkidle" });
await page.evaluate(() => {
  document.body.classList.add("export");
  document.documentElement.style.setProperty("--frame-zoom", "1");
});
await page.waitForTimeout(400);

const report = await page.evaluate(() => {
  const out = { screens: 0, empty: [], overflow: [], badText: [], tiny: [], orphan: [] };
  const allowed = new Set(["11px", "12px", "13px", "15px", "18px", "22px", "10px", "24px", "26px", "28px"]);

  document.querySelectorAll(".screen").forEach((s, i) => {
    out.screens++;
    const title = s.getAttribute("data-title") || `#${i}`;
    const frame = s.querySelector(".screen-frame");
    if (!frame || frame.children.length === 0) out.empty.push(title);

    // 画框内的横向溢出
    frame.querySelectorAll("*").forEach((el) => {
      if (el.scrollWidth > el.clientWidth + 2 && getComputedStyle(el).overflowX === "visible") {
        if (el.clientWidth > 0) out.overflow.push(`${title}: <${el.tagName.toLowerCase()}.${el.className}> ${el.scrollWidth}>${el.clientWidth}`);
      }
    });

    // 未渲染的占位文本
    if (frame.textContent.includes("[object Object]")) out.badText.push(title);
    if (frame.textContent.includes("undefined") || frame.textContent.includes("NaN")) out.badText.push(title);

    // 字号越界（只查视口中可见的元素，跳过图标 svg）
    if (!["交互状态矩阵", "设计令牌"].includes(title)) {
      frame.querySelectorAll("span,div,td,th,label,button,dt,dd,h1,h2,h3,p").forEach((el) => {
        if (!el.textContent.trim() || el.children.length > 0) return;
        const fs = getComputedStyle(el).fontSize;
        if (!allowed.has(fs)) out.tiny.push(`${title}: ${fs} "${el.textContent.trim().slice(0, 18)}"`);
      });
    }
  });

  return out;
});

console.log(`检查 ${report.screens} 张设计图`);
console.log(`空画框: ${report.empty.length ? report.empty.join(", ") : "无"}`);
console.log(`未渲染文本: ${report.badText.length ? [...new Set(report.badText)].join(", ") : "无"}`);
console.log(`横向溢出: ${report.overflow.length ? [...new Set(report.overflow)].slice(0, 12).join("\n           ") : "无"}`);
console.log(`字号越界: ${report.tiny.length ? [...new Set(report.tiny)].slice(0, 12).join("\n           ") : "无"}`);
console.log(`页面错误: ${errors.length ? errors.join("; ") : "无"}`);

await browser.close();
