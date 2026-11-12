/** 应用入口。 */

import "./styles/base.css";
import "./styles/connection-dialog.css";
import { App } from "./core/App";
import { formatError } from "./core/execution";

const root = document.getElementById("app");
if (!root) {
  throw new Error("找不到 #app 根节点");
}

const app = new App(root);
app.init().catch((e) => {
  console.error("应用启动失败", e);
  root.innerHTML = `<div style="padding:24px;font-family:'Microsoft YaHei UI',sans-serif">
    <h2 style="font-size:14px;margin:0 0 8px">应用启动失败</h2>
    <pre style="font-size:12px;white-space:pre-wrap;color:#c62828"></pre>
  </div>`;
  root.querySelector("pre")!.textContent = formatError(e);
});
