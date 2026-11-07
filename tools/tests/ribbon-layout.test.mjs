import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

const bundle = await build({
  entryPoints: ["src/ui/Ribbon.ts"], bundle: true, write: false,
  platform: "node", format: "esm", logLevel: "silent",
});
const { RIBBON_MAIN, Ribbon } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);

test("顶部查询、备份紧随函数，其它与图表不展示", () => {
  const ids = RIBBON_MAIN.map(item => item.id);
  assert.deepEqual(ids.slice(ids.indexOf("open-function"), ids.indexOf("open-function") + 3), ["open-function", "open-query", "backup"]);
  assert(!ids.includes("open-other"));
  assert(!ids.includes("chart"));
  assert(ids.includes("open-user") && ids.includes("schedule") && ids.includes("model"));
});

test("连接按钮不带展开箭头，点击直接创建连接", () => {
  const makeElement = () => ({
    children: [], className: "", innerHTML: "", attributes: {}, events: {},
    classList: { toggle() {} },
    appendChild(child) { this.children.push(child); },
    setAttribute(name, value) { this.attributes[name] = value; },
    addEventListener(name, handler) { this.events[name] = handler; },
  });
  const previous = globalThis.document;
  globalThis.document = { createElement: makeElement };
  try {
    const commands = [], dropdowns = [];
    const ribbon = new Ribbon(RIBBON_MAIN, {
      onCommand: id => commands.push(id),
      onDropdown: id => dropdowns.push(id),
    });
    const connection = ribbon.el.children[0];
    assert.equal(connection.attributes["aria-label"], "连接");
    assert(!connection.children.some(child => child.className === "ribbon-drop"));
    connection.events.click();
    assert.deepEqual(commands, ["new-connection"]);
    assert.deepEqual(dropdowns, []);
  } finally { globalThis.document = previous; }
});
