/** 软件更新对话框（需求 1.2 / STORY-UPD-001）。
 * 设计见 docs/design/DESIGN.md §6.22：紧凑白底、品牌标志、版本与日期、更新偏好及左右分组操作。
 * 打开即检查；下载/校验/重启沿用后端。自动更新仅后台下载，下次启动进入工作区前安装。
 */
import "../styles/update-dialog.css";
import { api } from "../core/api";
import type { UpdateCheckResult, UpdateManifest, UpdateProgressEvent } from "../core/types";
import { downloadUpdate, readUpdatePreferences, saveUpdatePreferences, skipUpdateVersion } from "../core/updatePreferences";
import { Dialog } from "../ui/Dialog";
import { ICON_WIN_MIN, svg } from "../ui/icons";
import { navkitLogo } from "../ui/logo";
import { accessibleDialog } from "./SyncDialog";
import { taskCenter } from "../core/backgroundTasks";

type UpdateState = "checking" | "latest" | "available" | "downloading" | "ready" | "error";

function listenProgress(onEvent: (e: UpdateProgressEvent) => void): () => void {
  let unlisten: (() => void) | null = null;
  let cancelled = false;
  void (async () => {
    try {
      const { listen } = await import("@tauri-apps/api/event");
      if (cancelled) return;
      const stop = await listen<UpdateProgressEvent>("update-progress", e => onEvent(e.payload));
      if (cancelled) stop();
      else unlisten = stop;
    } catch { /* 非 Tauri 环境使用状态轮询。 */ }
  })();
  return () => { cancelled = true; unlisten?.(); };
}

function formatBytes(n: number): string {
  if (!n) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = n, i = 0;
  while (value >= 1024 && i < units.length - 1) { value /= 1024; i++; }
  return `${value.toFixed(value >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

/** 日期只采用清单中的日历日期，不因时区改变发布日期。 */
function publicationDate(value: string): string {
  const match = /^(\d{4})-(\d{2})-(\d{2})(?:$|T|\s)/.exec(value.trim());
  if (!match) return value.trim() || "发布日期未提供";
  const [, year, month, day] = match;
  const date = new Date(`${year}-${month}-${day}T00:00:00Z`);
  if (!Number.isFinite(date.getTime()) || date.getUTCMonth() + 1 !== Number(month) || date.getUTCDate() !== Number(day)) return "发布日期未提供";
  return `${year}年${Number(month)}月${Number(day)}日`;
}

export function openUpdateDialog(): Dialog {
  const previousFocus = document.activeElement as HTMLElement | null;
  const dlg = new Dialog({ title: "软件更新", width: 640, render: () => {}, buttons: [] });
  dlg.el.classList.add("update-dialog");
  const body = dlg.body;

  // 空白可拖动标题栏，最小化收起本弹窗而不是最小化整个应用。
  const minimize = document.createElement("button");
  minimize.type = "button";
  minimize.className = "ud-minimize";
  minimize.setAttribute("aria-label", "最小化");
  minimize.setAttribute("aria-expanded", "true");
  minimize.innerHTML = svg(ICON_WIN_MIN, 12);
  minimize.addEventListener("mousedown", e => e.stopPropagation());
  minimize.addEventListener("click", () => {
    const collapsed = dlg.el.classList.toggle("is-minimized");
    minimize.setAttribute("aria-expanded", String(!collapsed));
    minimize.setAttribute("aria-label", collapsed ? "还原" : "最小化");
  });
  const titlebar = dlg.el.querySelector<HTMLElement>(".dialog-title")!;
  titlebar.insertBefore(minimize, titlebar.querySelector(".dialog-title-close"));

  const hero = document.createElement("div");
  hero.className = "ud-hero";
  const iconBox = document.createElement("div");
  iconBox.className = "ud-icon";
  iconBox.setAttribute("aria-hidden", "true");
  iconBox.innerHTML = navkitLogo(48);
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

  const notesBox = document.createElement("div");
  notesBox.className = "ud-notes";
  notesBox.id = `ud-notes-${crypto.randomUUID()}`;
  notesBox.hidden = true;
  const notesTitle = document.createElement("div");
  notesTitle.className = "ud-notes-title";
  notesTitle.textContent = "更新内容";
  const notesList = document.createElement("ul");
  notesList.className = "ud-notes-list";
  notesBox.append(notesTitle, notesList);
  body.appendChild(notesBox);

  const progress = document.createElement("div");
  progress.className = "ud-progress";
  progress.hidden = true;
  const track = document.createElement("div");
  track.className = "ud-track";
  track.setAttribute("role", "progressbar");
  track.setAttribute("aria-label", "更新下载进度");
  track.setAttribute("aria-valuemin", "0");
  track.setAttribute("aria-valuemax", "100");
  const fill = document.createElement("div");
  fill.className = "ud-fill";
  track.appendChild(fill);
  const metaEl = document.createElement("div");
  metaEl.className = "ud-meta";
  progress.append(track, metaEl);
  body.appendChild(progress);

  const preference = document.createElement("label");
  preference.className = "ud-preference";
  const autoUpdate = document.createElement("input");
  autoUpdate.type = "checkbox";
  autoUpdate.checked = readUpdatePreferences().autoUpdate;
  autoUpdate.setAttribute("aria-label", "以后自动下载并安装更新");
  const preferenceText = document.createElement("span");
  preferenceText.textContent = "以后自动下载并安装更新";
  const preferenceHint = document.createElement("span");
  preferenceHint.id = `ud-auto-hint-${crypto.randomUUID()}`;
  preferenceHint.hidden = true;
  preferenceHint.textContent = "运行期间自动下载，下次启动进入工作界面之前安装；不会立即重启。";
  autoUpdate.setAttribute("aria-describedby", preferenceHint.id);
  preference.title = preferenceHint.textContent;
  preference.append(autoUpdate, preferenceText, preferenceHint);
  body.appendChild(preference);

  const errorLine = document.createElement("div");
  errorLine.className = "ud-error";
  errorLine.setAttribute("role", "alert");
  body.appendChild(errorLine);
  autoUpdate.addEventListener("change", () => {
    if (!saveUpdatePreferences({ autoUpdate: autoUpdate.checked })) {
      autoUpdate.checked = false;
      errorLine.textContent = "无法保存自动更新设置，请检查本地存储后重试。";
    } else errorLine.textContent = "";
  });

  let state: UpdateState = "checking";
  let manifest: UpdateManifest | null = null;
  let currentVersion = "";
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let closed = false;
  let applying = false;
  let actionsInitialized = false;
  let checkGeneration = 0;

  function stopPolling(): void {
    if (pollTimer !== null) { clearInterval(pollTimer); pollTimer = null; }
  }
  function startPolling(): void {
    stopPolling();
    if (typeof globalThis.setInterval !== "function") return;
    pollTimer = setInterval(() => {
      void api.updateStatus().then(s => {
        if (closed) return;
        if (s.phase === "ready") setState("ready");
        else if (s.phase === "downloading") renderProgress(s.done, s.total, s.speed);
      }).catch(() => { /* 忽略单次轮询失败。 */ });
    }, 700);
  }
  function renderProgress(done: number, total: number, speed: number): void {
    progress.hidden = false;
    const pct = total ? Math.min(100, Math.round((done / total) * 100)) : 0;
    fill.style.width = `${pct}%`;
    if (total) track.setAttribute("aria-valuenow", String(pct));
    else track.removeAttribute("aria-valuenow");
    metaEl.textContent = `${formatBytes(done)}${total ? ` / ${formatBytes(total)}` : ""}${speed ? ` · ${formatBytes(speed)}/s` : ""}`;
  }
  const stopProgress = listenProgress(e => {
    if (closed) return;
    if (e.phase === "downloading" || e.phase === "verifying") {
      if (state !== "downloading") setState("downloading");
      if (e.phase === "downloading") renderProgress(e.done, e.total, e.speed);
      else { fill.style.width = "100%"; track.removeAttribute("aria-valuenow"); metaEl.textContent = "正在校验 SHA-256…"; }
    } else if (e.phase === "ready") setState("ready");
    else if (e.phase === "error") setState("error", e.message || "下载更新失败。");
  });

  interface Action { label: string; primary?: boolean; left?: boolean; run: () => void }
  function renderActions(actions: Action[]): void {
    const hadFocus = dlg.footer.querySelectorAll("button");
    const moveFocus = !actionsInitialized || [...hadFocus].some(button => button === document.activeElement);
    const buttons = actions.map(action => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = `btn${action.primary ? " primary" : ""}${action.left ? " ud-action-left" : ""}`;
      btn.textContent = action.label;
      btn.addEventListener("click", () => action.run());
      return btn;
    });
    dlg.footer.replaceChildren(...buttons);
    if (moveFocus) (buttons.find(button => button.classList.contains("primary")) ?? buttons[0])?.focus();
    actionsInitialized = true;
  }
  function setState(next: UpdateState, message?: string): void {
    if (closed) return;
    const focused = document.activeElement;
    const relocateFocus = focused === titleEl.querySelector("button") || (next === "downloading" && focused === autoUpdate);
    if (next !== "checking") checkGeneration++;
    state = next;
    dlg.el.setAttribute("data-state", next);
    notesBox.hidden = true;
    progress.hidden = next !== "downloading" && next !== "ready";
    preference.hidden = next === "downloading";
    errorLine.textContent = next === "error" ? (message ?? "检查更新失败。") : "";
    switch (next) {
      case "checking":
        titleEl.textContent = "正在检查更新…";
        subEl.textContent = currentVersion ? `当前版本 ${currentVersion}` : "正在连接更新服务器";
        renderActions([{ label: "取消", run: () => dlg.close() }]);
        break;
      case "latest":
        stopPolling();
        titleEl.textContent = "已是最新版本";
        subEl.textContent = currentVersion ? `当前版本 ${currentVersion} 已是最新版本` : "当前已是最新版本";
        renderActions([{ label: "重新检查", run: () => void check() }, { label: "完成", primary: true, run: () => dlg.close() }]);
        break;
      case "available": {
        const version = (manifest?.version ?? "").replace(/^v/i, "");
        const versionButton = document.createElement("button");
        versionButton.type = "button";
        versionButton.className = "ud-version-button";
        versionButton.textContent = version ? `发现新版本  v${version}` : "发现新版本";
        versionButton.setAttribute("aria-label", `${versionButton.textContent}，查看更新内容`);
        versionButton.setAttribute("aria-controls", notesBox.id);
        versionButton.setAttribute("aria-expanded", "false");
        versionButton.title = "查看更新内容";
        versionButton.addEventListener("click", () => {
          notesBox.hidden = !notesBox.hidden;
          versionButton.setAttribute("aria-expanded", String(!notesBox.hidden));
        });
        titleEl.replaceChildren(versionButton);
        const calendar = document.createElement("span");
        calendar.className = "ud-calendar";
        calendar.setAttribute("aria-hidden", "true");
        calendar.innerHTML = '<svg width="14" height="14" viewBox="0 0 16 16" fill="none"><rect x="2" y="3" width="12" height="11" rx="1.5" stroke="currentColor"/><path d="M5 1.5v3M11 1.5v3M2 6h12M5 8h1M8 8h1M11 8h1M5 11h1M8 11h1" stroke="currentColor"/></svg>';
        const date = document.createElement("span");
        date.textContent = publicationDate(manifest?.pub_date ?? "");
        subEl.replaceChildren(calendar, date);
        const lines = (manifest?.notes ?? "").split(/\r?\n/).map(line => line.replace(/^\s*(?:[-*•·]|\d+[.)])\s*/, "").trim()).filter(Boolean);
        notesList.replaceChildren(...(lines.length ? lines : ["（本版本无更新说明）"]).map(line => {
          const li = document.createElement("li"); li.textContent = line; return li;
        }));
        renderActions([
          { label: "跳过此版本", left: true, run: () => {
            if (manifest && skipUpdateVersion(manifest.version)) dlg.close();
            else errorLine.textContent = "本次会话已跳过此版本，但无法保存设置；下次启动可能再次提示。";
          } },
          { label: "稍后", run: () => dlg.close() },
          { label: "下载更新", primary: true, run: () => void download() },
        ]);
        break;
      }
      case "downloading":
        titleEl.textContent = manifest?.version ? `正在下载 v${manifest.version.replace(/^v/i, "")}` : "正在下载更新";
        subEl.textContent = manifest?.size ? `更新包 ${formatBytes(manifest.size)}` : "更新包下载中";
        renderActions([{ label: "后台下载", run: () => dlg.close() }]);
        break;
      case "ready":
        stopPolling();
        titleEl.textContent = manifest?.version ? `v${manifest.version.replace(/^v/i, "")} 已就绪` : "更新包已就绪";
        subEl.textContent = readUpdatePreferences().autoUpdate ? "SHA-256 校验通过，下次启动时自动安装" : "SHA-256 校验通过，重启后完成安装";
        fill.style.width = "100%";
        track.setAttribute("aria-valuenow", "100");
        metaEl.textContent = "更新包已就绪（SHA-256 校验通过）";
        renderActions([{ label: "稍后", run: () => dlg.close() }, { label: "重启并安装", primary: true, run: () => void apply() }]);
        break;
      case "error":
        stopPolling();
        titleEl.textContent = "检查更新失败";
        subEl.textContent = "请检查网络或更新源配置后重试";
        renderActions([{ label: "关闭", run: () => dlg.close() }, { label: "重试", primary: true, run: () => void check() }]);
        break;
    }
    if (relocateFocus) {
      (dlg.footer.querySelector<HTMLButtonElement>(".primary") ?? dlg.footer.querySelector<HTMLButtonElement>("button") ?? minimize).focus();
    }
  }
  async function check(): Promise<void> {
    if (closed) return;
    setState("checking");
    const generation = ++checkGeneration;
    try {
      const result: UpdateCheckResult = await api.updateCheck();
      if (closed || generation !== checkGeneration || state !== "checking") return;
      currentVersion = result.current_version;
      manifest = result.update_available ? result.manifest : null;
      setState(manifest ? "available" : "latest");
    } catch (e) {
      if (generation === checkGeneration && state === "checking") setState("error", String(e));
    }
  }
  async function download(): Promise<void> {
    if (closed || !manifest || state !== "available") return;
    setState("downloading");
    renderProgress(0, 0, 0);
    metaEl.textContent = "准备下载…";
    startPolling();
    const task = taskCenter.register("update", `软件更新下载 · v${manifest.version}`);
    try {
      const file = await downloadUpdate(manifest);
      task.finish("更新包已就绪", file);
      setState("ready");
    } catch (e) {
      task.fail(String(e));
      setState("error", String(e));
    } finally { stopPolling(); }
  }
  async function apply(): Promise<void> {
    if (applying || closed) return;
    applying = true;
    dlg.footer.querySelectorAll<HTMLButtonElement>("button").forEach(button => { button.disabled = true; });
    subEl.textContent = "正在换名替换程序并重启…";
    try {
      await api.updateApply(readUpdatePreferences().pendingInstall?.sha256);
      if (!closed) subEl.textContent = "应用即将重启…";
    } catch (e) { applying = false; setState("error", String(e)); }
  }
  dlg.onClose(() => { closed = true; stopProgress(); stopPolling(); });
  accessibleDialog(dlg, "软件更新", previousFocus);
  setState("checking");
  void (async () => {
    try {
      const status = await api.updateStatus();
      if (closed) return;
      if (status.phase === "ready") { setState("ready"); return; }
      if (status.phase === "downloading" || status.phase === "verifying") {
        setState("downloading"); renderProgress(status.done, status.total, status.speed); startPolling(); return;
      }
    } catch { /* 状态不可用时继续正常检查。 */ }
    try {
      const info = await api.aboutInfo();
      if (closed) return;
      currentVersion = info["version"] ?? currentVersion;
      if (state === "checking") subEl.textContent = `当前版本 ${currentVersion}`;
    } catch { /* 版本信息缺失不阻止检查。 */ }
    if (!closed && state === "checking") await check();
  })();
  return dlg;
}
