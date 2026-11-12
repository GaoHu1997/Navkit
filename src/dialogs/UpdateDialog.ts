/**
 * 软件更新对话框（需求 1.2）：检查更新 → 下载（进度 / 速度）→ 重启并安装。
 *
 * 流程与 magpie 对齐：
 *  · update_check 拉取清单（版本 / 说明 / 大小 / SHA-256）
 *  · update_download 后端流式下载（断点续传 + 哈希校验），进度经 update-progress 事件推送
 *  · update_apply 换名替换 exe → 分离启动新版本 → 退出；旧版本由新版本启动时清理
 */

import { api } from "../core/api";
import type { UpdateCheckResult, UpdateManifest, UpdateProgressEvent } from "../core/types";
import { Dialog, textInput } from "../ui/Dialog";
import { accessibleDialog } from "./SyncDialog";
import { taskCenter } from "../core/backgroundTasks";

/** 监听后端 update-progress 事件（事件 API 缺失时静默，改走轮询）。 */
function listenProgress(onEvent: (e: UpdateProgressEvent) => void): () => void {
  let unlisten: (() => void) | null = null;
  let cancelled = false;
  void (async () => {
    try {
      const { listen } = await import("@tauri-apps/api/event");
      if (cancelled) return;
      unlisten = await listen<UpdateProgressEvent>("update-progress", (e) => onEvent(e.payload));
    } catch {
      /* 非 Tauri 环境（静态预览）无事件，靠轮询兜底 */
    }
  })();
  return () => {
    cancelled = true;
    unlisten?.();
  };
}

function formatBytes(n: number): string {
  if (!n) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = n;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  return `${value.toFixed(value >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

/** 打开「检查更新」对话框。 */
export function openUpdateDialog(): Dialog {
  const dlg = new Dialog({ title: "软件更新", width: 520, render: () => {}, buttons: [{ label: "关闭" }] });
  const body = dlg.body;
  body.style.display = "flex";
  body.style.flexDirection = "column";
  body.style.gap = "10px";
  body.style.minHeight = "220px";

  // 当前版本行
  const currentRow = document.createElement("div");
  currentRow.style.cssText = "display:grid;grid-template-columns:80px 1fr;gap:8px;font-size:12px;align-items:center;";
  const currentLabel = document.createElement("span");
  currentLabel.style.color = "var(--text-tertiary)";
  currentLabel.textContent = "当前版本";
  const currentInput = textInput("", 200);
  currentInput.readOnly = true;
  currentRow.append(currentLabel, currentInput);
  body.appendChild(currentRow);

  // 更新信息区
  const info = document.createElement("div");
  info.style.cssText =
    "border:1px solid var(--border-subtle);border-radius:6px;padding:10px 12px;font-size:12px;line-height:1.7;min-height:64px;color:var(--text-secondary);white-space:pre-wrap;overflow-wrap:anywhere;";
  info.textContent = "点击「检查更新」查询最新版本。";
  body.appendChild(info);

  // 进度条
  const track = document.createElement("div");
  track.style.cssText = "height:6px;border-radius:999px;background:var(--n-150);overflow:hidden;";
  const fill = document.createElement("div");
  fill.style.cssText = "height:100%;width:0;background:var(--accent-500);border-radius:999px;transition:width .25s;";
  track.appendChild(fill);
  track.hidden = true;
  body.appendChild(track);

  const meta = document.createElement("div");
  meta.style.cssText = "font-size:11px;color:var(--text-tertiary);min-height:16px;";
  body.appendChild(meta);

  const errorLine = document.createElement("div");
  errorLine.style.cssText = "font-size:11.5px;color:var(--danger-600);min-height:0;white-space:pre-wrap;";
  body.appendChild(errorLine);

  // 按钮
  const checkBtn = document.createElement("button");
  checkBtn.type = "button";
  checkBtn.className = "btn primary";
  checkBtn.textContent = "检查更新";
  const downloadBtn = document.createElement("button");
  downloadBtn.type = "button";
  downloadBtn.className = "btn primary";
  downloadBtn.textContent = "下载更新";
  downloadBtn.hidden = true;
  const applyBtn = document.createElement("button");
  applyBtn.type = "button";
  applyBtn.className = "btn primary";
  applyBtn.textContent = "重启并安装";
  applyBtn.hidden = true;
  const stop = listenProgress((e) => {
    if (e.phase === "downloading") {
      track.hidden = false;
      const pct = e.total ? Math.round((e.done / e.total) * 100) : 0;
      fill.style.width = `${pct}%`;
      meta.textContent = `${formatBytes(e.done)} / ${formatBytes(e.total)} · ${formatBytes(e.speed)}/s`;
    } else if (e.phase === "verifying") {
      track.hidden = false;
      fill.style.width = "100%";
      meta.textContent = "校验 SHA-256...";
    } else if (e.phase === "ready") {
      onReady();
    }
  });
  dlg.onClose(() => stop());

  let manifest: UpdateManifest | null = null;
  let updateAvailable = false;
  let taskHandle: ReturnType<typeof taskCenter.register> | null = null;

  function onReady(): void {
    track.hidden = false;
    fill.style.width = "100%";
    meta.textContent = "更新包已就绪（SHA-256 校验通过）";
    downloadBtn.hidden = true;
    applyBtn.hidden = false;
    checkBtn.disabled = false;
    info.textContent = `新版本 ${manifest?.version ?? ""} 已下载完成。点击「重启并安装」换名替换程序并自动重启；旧版本将在新版本首次启动时删除。`;
    taskHandle?.finish("更新包已就绪", manifest?.url);
    taskHandle = null;
  }

  checkBtn.addEventListener("click", () => {
    errorLine.textContent = "";
    checkBtn.disabled = true;
    checkBtn.textContent = "正在检查...";
    info.textContent = "正在连接更新服务器...";
    void api
      .updateCheck()
      .then((result: UpdateCheckResult) => {
        currentInput.value = result.current_version;
        checkBtn.disabled = false;
        checkBtn.textContent = "检查更新";
        if (result.update_available && result.manifest) {
          manifest = result.manifest;
          updateAvailable = true;
          const size = manifest.size ? ` · ${formatBytes(manifest.size)}` : "";
          info.textContent = `发现新版本 ${manifest.version}${size}\n发布日期：${manifest.pub_date || "—"}\n\n${manifest.notes || "（无更新说明）"}`;
          downloadBtn.hidden = false;
        } else {
          updateAvailable = false;
          info.textContent = `当前已是最新版本（${result.current_version}）。`;
          downloadBtn.hidden = true;
        }
      })
      .catch((e) => {
        checkBtn.disabled = false;
        checkBtn.textContent = "检查更新";
        info.textContent = "检查更新失败。";
        errorLine.textContent = String(e);
      });
  });

  downloadBtn.addEventListener("click", () => {
    if (!updateAvailable) return;
    errorLine.textContent = "";
    downloadBtn.disabled = true;
    checkBtn.disabled = true;
    track.hidden = false;
    fill.style.width = "0";
    meta.textContent = "准备下载...";
    taskHandle = taskCenter.register("update", `软件更新下载 · v${manifest?.version ?? "?"}`);
    void api
      .updateDownload()
      .then(() => {
        onReady();
      })
      .catch((e) => {
        downloadBtn.disabled = false;
        checkBtn.disabled = false;
        track.hidden = true;
        meta.textContent = "";
        errorLine.textContent = String(e);
        taskHandle?.fail(String(e));
        taskHandle = null;
      });
  });

  applyBtn.addEventListener("click", () => {
    applyBtn.disabled = true;
    info.textContent = "正在换名替换程序并重启...";
    void api
      .updateApply()
      .then(() => {
        info.textContent = "应用即将重启...";
      })
      .catch((e) => {
        applyBtn.disabled = false;
        errorLine.textContent = String(e);
      });
  });

  const btnRow = document.createElement("div");
  btnRow.style.cssText = "display:flex;gap:8px;align-items:center;";
  btnRow.append(checkBtn, downloadBtn, applyBtn);
  body.appendChild(btnRow);

  accessibleDialog(dlg, "软件更新");
  // 预填当前版本（about_info）
  void api
    .aboutInfo()
    .then((info2) => {
      currentInput.value = info2["version"] ?? "";
    })
    .catch(() => {});
  // 已有就绪更新包则直接进入可安装状态
  void api
    .updateStatus()
    .then((s) => {
      if (s.phase === "ready") onReady();
    })
    .catch(() => {});
  return dlg;
}
