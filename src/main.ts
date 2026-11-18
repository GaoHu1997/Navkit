/** 应用入口。 */

import "./styles/base.css";
import "./styles/connection-dialog.css";
import "./ui/searchable-select.css";
import { installSearchableSelects } from "./ui/SearchableSelect";
import { installSearchableInputs } from "./ui/SearchableInput";
import { App } from "./core/App";
import { formatError } from "./core/execution";
import { installPendingUpdateAtStartup } from "./core/updatePreferences";

const root = document.getElementById("app");
if (!root) {
  throw new Error("找不到 #app 根节点");
}

async function bootstrap(): Promise<void> {
  root!.textContent = "正在启动 NavKit…";
  const update = await installPendingUpdateAtStartup();
  if (update.restarting) {
    root!.textContent = "正在安装更新并重启…";
    return;
  }
  installSearchableSelects();
  installSearchableInputs();
  const app = new App(root!);
  await app.init();
  if (update.error) {
    const notice = document.createElement("div");
    notice.className = "toast error";
    notice.setAttribute("role", "alert");
    notice.style.pointerEvents = "auto";
    const text = document.createElement("div");
    text.className = "toast-text";
    text.textContent = update.error;
    const dismiss = document.createElement("button");
    dismiss.type = "button";
    dismiss.className = "dialog-error-close";
    dismiss.setAttribute("aria-label", "关闭更新提示");
    dismiss.textContent = "×";
    dismiss.addEventListener("click", () => notice.remove());
    notice.append(text, dismiss);
    root!.querySelector(".toast-host")?.appendChild(notice);
  }
}

void bootstrap().catch((e) => {
  console.error("应用启动失败", e);
  root.innerHTML = `<div style="padding:24px;font-family:'Microsoft YaHei UI',sans-serif">
    <h2 style="font-size:14px;margin:0 0 8px">应用启动失败</h2>
    <pre style="font-size:12px;white-space:pre-wrap;color:#c62828"></pre>
  </div>`;
  root.querySelector("pre")!.textContent = formatError(e);
});
