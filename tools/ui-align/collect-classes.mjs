// 收集 src/**/*.ts 里出现的 CSS 类名(静态字符串 + 模板串拆分), 输出去重清单。
import fs from "node:fs";
import path from "node:path";

const ROOT = path.resolve(new URL("..", import.meta.url).pathname.replace(/^\/(\w):/, "$1:"), "..");
const SRC = path.join(ROOT, "src");

const files = [];
(function walk(dir) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p);
    else if (e.name.endsWith(".ts")) files.push(p);
  }
})(SRC);

const set = new Set();
for (const f of files) {
  const code = fs.readFileSync(f, "utf8");
  // className = "..." / className = `...` / classList.add("...") / className += " ..."
  const re = /(?:className|class)\s*[+]?=\s*[`"']([^`"']*)[`"']|classList\.(?:add|toggle|remove)\(\s*[`"']([^`"':]+)[`"']/g;
  let m;
  while ((m = re.exec(code))) {
    const raw = (m[1] ?? m[2] ?? "")
      .replace(/\$\{[^}]*\}/g, " ")
      .replace(/[$(){}]/g, " ");
    for (const tok of raw.split(/\s+/)) if (/^[a-z][a-z0-9_-]*$/i.test(tok)) set.add(tok);
  }
}
console.log([...set].sort().join("\n"));
console.error(`total: ${set.size}`);
