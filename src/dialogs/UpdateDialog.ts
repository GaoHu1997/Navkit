/**
 * 软件更新对话框（需求 1.2）：打开即自动检查 → 下载（进度 / 速度）→ 重启并安装。
 *
 * 设计要点（2026-11 重做）：
 *  · 打开即自动发起 update_check，用户不必再点「检查更新」；已有就绪安装包时直接进可安装态。
 *  · 全流程收敛为单一状态机 checking / latest / available / downloading / ready / error，
 *    每个状态只保留一个主操作，主按钮随状态变化而不是并列多个 hidden 按钮。
 *  · 版本用 chip 对比（已安装 → 最新版），更新说明按行渲染为列表。
 *
 * 流程与 magpie 对齐：
 *  · update_check 拉取清单（版本 / 说明 / 大小 / SHA-256）
 *  · update_download 后端流式下载（断点续传 + 哈希校验），进度经 update-progress 事件推送
 *  · update_apply 换名替换 exe → 分离启动新版本 → 退出；旧版本由新版本启动时清理
 */

import "../styles/update-dialog.css";
import { api } from "../core/api";
import type { UpdateCheckResult, UpdateManifest, UpdateProgressEvent } from "../core/types";
import { Dialog } from "../ui/Dialog";
import { ICON_CHECK, ICON_CROSS, ICON_DOWNLOAD, ICON_REFRESH, svg } from "../ui/icons";
import { accessibleDialog } from "./SyncDialog";
import { taskCenter } from "../core/backgroundTasks";

/** 对话框状态：一个时刻只有一个主操作。 */
type UpdateState = "checking" | "latest" | "available" | "downloading" | "ready" | "error";

/** 监听后端 update-progress 事件（事件 API 缺失时静默，改走 update_status 轮询）。 */
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

/** 把清单里的更新说明拆成列表行：按行切分，去掉项目符号与空行。 */
function splitNotes(notes: string): string[] {
  return notes
    .split(/\r?\n/)
    .map((line) => line.replace(/^\s*(?:[-*•·]|\d+[.)])\s*/, "").trim())
    .filter(Boolean);
}

/** 打开「软件更新」对话框：打开即自动检查。 */
export function openUpdateDialog(): Dialog {
  const dlg = new Dialog({ title: "软件更新", width: 560, render: () => {}, buttons: [] });
  const body = dlg.body;
  body.classList.add("update-dialog");

  // ---------------------------------------------------------------- 头部
  const hero = document.createElement("div");
  hero.className = "ud-hero";
  const iconBox = document.createElement("div");
  iconBox.className = "ud-icon";
  iconBox.setAttribute("data-state", "checking");
  iconBox.setAttribute("aria-hidden", "true");
  const heroText = document.createElement("div");
  heroText.className = "ud-hero-text";
  const titleEl = document.createElement("div");
  titleEl.className = "ud-title";
  titleEl.setAttribute("role", "status");
  titleEl.setAttribute("aria-live", "polite");
  const subEl = document.createElement("div");
  subEl.className = "ud-sub";
  heroText.append(titleEl, subEl);
  hero.append(iconBox, heroText);
  body.appendChild(hero);

  // ---------------------------------------------------------------- 版本对比条
  const versions = document.createElement("div");
  versions.className = "ud-versions";
  versions.hidden = true;
  const installedLabel = document.createElement("span");
  installedLabel.textContent = "已安装";
  const installedChip = document.createElement("span");
  installedChip.className = "ud-chip";
  const arrow = document.createElement("span");
  arrow.className = "ud-arrow";
  arrow.textContent = "→";
  const latestLabel = document.createElement("span");
  latestLabel.textContent = "最新版";
  const latestChip = document.createElement("span");
  latestChip.className = "ud-chip is-latest";
  const sizeEl = document.createElement("span");
  sizeEl.className = "ud-size";
  versions.append(installedLabel, installedChip, arrow, latestLabel, latestChip, sizeEl);
  body.appendChild(versions);

  // ---------------------------------------------------------------- 更新内容
  const notesBox = document.createElement("div");
  notesBox.className = "ud-notes";
  notesBox.hidden = true;
  const notesTitle = document.createElement("div");
  notesTitle.className = "ud-notes-title";
  notesTitle.textContent = "更新内容";
  const notesList = document.createElement("ul");
  notesList.className = "ud-notes-list";
  notesBox.append(notesTitle, notesList);
  body.appendChild(notesBox);

  // ---------------------------------------------------------------- 下载进度
  const progress = document.createElement("div");
  progress.className = "ud-progress";
  progress.hidden = true;
  const track = document.createElement("div");
  track.className = "ud-track";
  const fill = document.createElement("div");
  fill.className = "ud-fill";
  track.appendChild(fill);
  const metaEl = document.createElement("div");
  metaEl.className = "ud-meta";
  progress.append(track, metaEl);
  body.appendChild(progress);

  // 检查中的不确定进度条
  const indeterminate = document.createElement("div");
  indeterminate.className = "ud-indeterminate";
  indeterminate.hidden = true;
  body.appendChild(indeterminate);

  const errorLine = document.createElement("div");
  errorLine.className = "ud-error";
  errorLine.setAttribute("role", "alert");
  body.appendChild(errorLine);

  // ---------------------------------------------------------------- 状态
  let state: UpdateState = "checking";
  let manifest: UpdateManifest | null = null;
  let currentVersion = "";
  let taskHandle: ReturnType<typeof taskCenter.register> | null = null;
  let pollTimer: number | null = null;
  let closed = false;

  const stopProgress = listenProgress((e) => {
    if (closed) return;
    if (e.phase === "downloading") {
      if (state !== "downloading") setState("downloading");
      renderProgress(e.done, e.total, e.speed);
    } else if (e.phase === "verifying") {
      if (state !== "downloading") setState("downloading");
      track.hidden = false;
      fill.style.width = "100%";
      metaEl.textContent = "正在校验 SHA-256…";
    } else if (e.phase === "ready") {
      setState("ready");
    } else if (e.phase === "error") {
      setState("error", e.message || "下载更新失败。");
    }
  });

  function stopPolling(): void {
    if (pollTimer !== null) {
      (globalThis as unknown as { clearInterval?: (id: number) => void }).clearInterval?.(pollTimer);
      pollTimer = null;
    }
  }

  /** 事件通道缺失（静态预览 / 旧后端）时用 update_status 轮询兜底。 */
  function startPolling(): void {
    stopPolling();
    const setIntervalFn = (globalThis as unknown as {
      setInterval?: (fn: () => void, ms: number) => number;
    }).setInterval;
    if (typeof setIntervalFn !== "function") return;
    pollTimer = setIntervalFn(() => {
      void api
        .updateStatus()
        .then((s) => {
          if (closed) return;
          if (s.phase === "ready") setState("ready");
          else if (s.phase === "downloading") renderProgress(s.done, s.total, s.speed);
        })
        .catch(() => {
          /* 忽略单次轮询失败 */
        });
    }, 700);
  }

  function renderProgress(done: number, total: number, speed: number): void {
    progress.hidden = false;
    indeterminate.hidden = true;
    track.hidden = false;
    const pct = total ? Math.min(100, Math.round((done / total) * 100)) : 0;
    fill.style.width = `${pct}%`;
    metaEl.textContent = total
      ? `${formatBytes(done)} / ${formatBytes(total)}${speed ? ` · ${formatBytes(speed)}/s` : ""}`
      : `${formatBytes(done)}${speed ? ` · ${formatBytes(speed)}/s` : ""}`;
  }

  function setIcon(def: { w: number; h: number; body: string }): void {
    iconBox.innerHTML = svg(def, 22);
  }

  // ---------------------------------------------------------------- 底部按钮
  interface Action {
    label: string;
    primary?: boolean;
    disabled?: boolean;
    run: () => void;
  }

  function renderActions(actions: Action[]): void {
    const buttons = actions.map((action) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = action.primary ? "btn primary" : "btn";
      btn.textContent = action.label;
      if (action.disabled) btn.disabled = true;
      btn.addEventListener("click", () => action.run());
      return btn;
    });
    dlg.footer.replaceChildren(...buttons);
  }

  /** 单一入口：所有界面文字、图标、按钮都由状态推导，避免出现互相矛盾的组合。 */
  function setState(next: UpdateState, message?: string): void {
    if (closed) return;
    state = next;
    iconBox.setAttribute("data-state", next);
    versions.hidden = next === "checking" || next === "error";
    notesBox.hidden = next !== "available";
    indeterminate.hidden = next !== "checking";
    progress.hidden = next !== "downloading" && next !== "ready";
    errorLine.textContent = next === "error" ? (message ?? "检查更新失败。") : "";

    switch (next) {
      case "checking": {
        setIcon(ICON_REFRESH);
        titleEl.textContent = "正在检查更新…";
        subEl.textContent = currentVersion ? `当前版本 ${currentVersion}` : "正在连接更新服务器";
        renderActions([{ label: "取消", run: () => dlg.close() }]);
        break;
      }
      case "latest": {
        setIcon(ICON_CHECK);
        titleEl.textContent = "已是最新版本";
        subEl.textContent = currentVersion
          ? `当前版本 ${currentVersion} 已是最新版本`
          : "当前已是最新版本";
        versions.hidden = false;
        arrow.hidden = true;
        latestLabel.hidden = true;
        latestChip.hidden = true;
        installedLabel.textContent = "当前版本";
        sizeEl.textContent = "";
        renderActions([
          { label: "重新检查", run: () => void check() },
          { label: "完成", primary: true, run: () => dlg.close() },
        ]);
        break;
      }
      case "available": {
        setIcon(ICON_DOWNLOAD);
        const version = manifest?.version ?? "";
        titleEl.textContent = version ? `发现新版本 ${version}` : "发现新版本";
        subEl.textContent = manifest?.pub_date
          ? `发布于 ${manifest.pub_date}，可立即下载安装`
          : "可立即下载安装";
        versions.hidden = false;
        arrow.hidden = false;
        latestLabel.hidden = false;
        latestChip.hidden = false;
        installedLabel.textContent = "已安装";
        latestChip.textContent = version ? `v${version}` : "最新版";
        sizeEl.textContent = manifest?.size ? `更新包 ${formatBytes(manifest.size)}` : "";
        const lines = splitNotes(manifest?.notes ?? "");
        notesList.replaceChildren(
          ...(lines.length ? lines : ["（本版本无更新说明）"]).map((line) => {
            const li = document.createElement("li");
            li.textContent = line;
            return li;
          }),
        );
        renderActions([
          { label: "稍后提醒", run: () => dlg.close() },
          { label: "下载并安装", primary: true, run: () => void download() },
        ]);
        break;
      }
      case "downloading": {
        setIcon(ICON_DOWNLOAD);
        const version = manifest?.version ?? "";
        titleEl.textContent = version ? `正在下载 ${version}` : "正在下载更新";
        subEl.textContent = manifest?.size ? `更新包 ${formatBytes(manifest.size)}` : "更新包下载中";
        renderActions([{ label: "后台下载", run: () => dlg.close() }]);
        break;
      }
      case "ready": {
        setIcon(ICON_CHECK);
        const version = manifest?.version ?? "";
        titleEl.textContent = version ? `${version} 已就绪` : "更新包已就绪";
        subEl.textContent = "SHA-256 校验通过，重启后完成安装";
        progress.hidden = false;
        track.hidden = false;
        fill.style.width = "100%";
        metaEl.textContent = "更新包已就绪（SHA-256 校验通过）";
        renderActions([
          { label: "稍后重启", run: () => dlg.close() },
          { label: "重启并安装", primary: true, run: () => void apply() },
        ]);
        break;
      }
      case "error": {
        setIcon(ICON_CROSS);
        titleEl.textContent = "检查更新失败";
        subEl.textContent = "请检查网络或更新源配置后重试";
        renderActions([
          { label: "关闭", run: () => dlg.close() },
          { label: "重试", primary: true, run: () => void check() },
        ]);
        break;
      }
    }
  }

  // ---------------------------------------------------------------- 动作
  async function check(): Promise<void> {
    if (closed) return;
    setState("checking");
    try {
      const result: UpdateCheckResult = await api.updateCheck();
      if (closed) return;
      currentVersion = result.current_version;
      installedChip.textContent = result.current_version ? `v${result.current_version}` : "—";
      if (result.update_available && result.manifest) {
        manifest = result.manifest;
        setState("available");
      } else {
        manifest = null;
        setState("latest");
      }
    } catch (e) {
      if (closed) return;
      setState("error", String(e));
    }
  }

  async function download(): Promise<void> {
    if (closed || !manifest) return;
    setState("downloading");
    renderProgress(0, 0, 0);
    metaEl.textContent = "准备下载…";
    startPolling();
    taskHandle = taskCenter.register("update", `软件更新下载 · v${manifest.version}`);
    try {
      await api.updateDownload();
      if (closed) return;
      taskHandle?.finish("更新包已就绪", manifest.url);
      taskHandle = null;
      setState("ready");
    } catch (e) {
      if (closed) return;
      taskHandle?.fail(String(e));
      taskHandle = null;
      setState("error", String(e));
    } finally {
      stopPolling();
    }
  }

  async function apply(): Promise<void> {
    subEl.textContent = "正在换名替换程序并重启…";
    try {
      await api.updateApply();
      if (closed) return;
      subEl.textContent = "应用即将重启…";
    } catch (e) {
      setState("error", String(e));
    }
  }

  dlg.onClose(() => {
    closed = true;
    stopProgress();
    stopPolling();
  });

  accessibleDialog(dlg, "软件更新");
  setState("checking");

  // ---------------------------------------------------------------- 打开即检查
  void (async () => {
    // 已有就绪安装包（例如上次下载完成后关闭了弹窗）直接进可安装态，不重复检查。
    try {
      const status = await api.updateStatus();
      if (closed) return;
      if (status.phase === "ready") {
        setState("ready");
        return;
      }
      if (status.phase === "downloading") {
        setState("downloading");
        renderProgress(status.done, status.total, status.speed);
        startPolling();
        return;
      }
    } catch {
      /* 非桌面 / 预览环境没有该状态，继续正常检查 */
    }
    // 当前版本号先拿到，检查过程中也能显示。
    try {
      const info = await api.aboutInfo();
      if (closed) return;
      currentVersion = info["version"] ?? currentVersion;
      if (state === "checking") subEl.textContent = `当前版本 ${currentVersion}`;
      installedChip.textContent = currentVersion ? `v${currentVersion}` : "—";
    } catch {
      /* 读不到版本号不影响检查 */
    }
    if (closed) return;
    await check();
  })();

  return dlg;
}
