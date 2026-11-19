/**
 * 标题栏「更新」徽标：检测到新版本时以绿色「更新」出现；点击下载时边框加粗并沿边框生长为进度条；
 * 下载校验完成变为橘黄色「重启」，点击换名替换程序并重启。下载/校验/重启均显示内部遮罩与加载环并禁用按钮。
 *
 * 复用「软件更新」对话框的后端通道：update_check / update_download / update_status / update_apply，
 * 下载进度优先取 update-progress 事件，事件不可用时轮询 update_status 兜底。
 */

import "../styles/update-badge.css";
import { api } from "../core/api";
import type { UpdateManifest, UpdateProgressEvent } from "../core/types";
import { downloadUpdate, isUpdateSkipped, readUpdatePreferences, subscribeUpdatePreferences } from "../core/updatePreferences";
import { taskCenter } from "../core/backgroundTasks";

const SVG_NS = "http://www.w3.org/2000/svg";
const BADGE_W = 54;
const BADGE_H = 22;
const PAD = 1.45;
const RADIUS = 5;
/** 圆角矩形边框周长（与 ringPath 一致），用于把进度映射成 stroke-dashoffset。 */
const PERIMETER =
  2 * (BADGE_W - 2 * PAD - 2 * RADIUS) + 2 * (BADGE_H - 2 * PAD - 2 * RADIUS) + 2 * Math.PI * RADIUS;

const FIRST_CHECK_DELAY_MS = 2000;
const CHECK_INTERVAL_MS = 30 * 60 * 1000;
const POLL_INTERVAL_MS = 700;

export type UpdateBadgeNotify = (kind: "success" | "error" | "info", text: string) => void;

type BadgePhase = "hidden" | "available" | "downloading" | "ready" | "applying";

/** 圆角矩形路径：起点在顶部中点、顺时针，进度自顶部向右生长。 */
function ringPath(w: number, h: number, pad: number, r: number): string {
  const x0 = pad;
  const y0 = pad;
  const x1 = w - pad;
  const y1 = h - pad;
  const mid = (x0 + x1) / 2;
  return (
    "M " + mid + " " + y0 +
    " H " + (x1 - r) + " A " + r + " " + r + " 0 0 1 " + x1 + " " + (y0 + r) +
    " V " + (y1 - r) + " A " + r + " " + r + " 0 0 1 " + (x1 - r) + " " + y1 +
    " H " + (x0 + r) + " A " + r + " " + r + " 0 0 1 " + x0 + " " + (y1 - r) +
    " V " + (y0 + r) + " A " + r + " " + r + " 0 0 1 " + (x0 + r) + " " + y0 +
    " H " + mid
  );
}

export class UpdateBadge {
  readonly el: HTMLButtonElement;
  private readonly labelEl: HTMLElement;
  private readonly barEl: SVGPathElement;
  private readonly trackEl: SVGPathElement;
  private phase: BadgePhase = "hidden";
  private manifest: UpdateManifest | null = null;
  private started = false;
  private checkTimer: number | null = null;
  private pollTimer: number | null = null;
  private stopEventListener: (() => void) | null = null;
  private stopPreferences: (() => void) | null = null;
  private checking = false;
  private generation = 0;
  private firstCheckTimer: number | null = null;
  private readonly notify?: UpdateBadgeNotify;

  constructor(notify?: UpdateBadgeNotify) {
    this.notify = notify;
    const el = document.createElement("button");
    el.type = "button";
    el.className = "tb-icon update-badge";
    el.hidden = true;
    el.dataset.phase = "hidden";
    el.title = "检查更新";
    el.setAttribute("aria-label", "更新");
    el.addEventListener("mousedown", (e) => e.stopPropagation());

    const ub = document.createElement("span");
    ub.className = "ub";

    const frame = document.createElementNS(SVG_NS, "svg") as SVGSVGElement;
    frame.setAttribute("class", "ub-frame");
    frame.setAttribute("viewBox", "0 0 " + BADGE_W + " " + BADGE_H);
    frame.setAttribute("aria-hidden", "true");

    const d = ringPath(BADGE_W, BADGE_H, PAD, RADIUS);
    this.trackEl = document.createElementNS(SVG_NS, "path") as SVGPathElement;
    this.trackEl.setAttribute("class", "ub-track");
    this.trackEl.setAttribute("d", d);
    this.barEl = document.createElementNS(SVG_NS, "path") as SVGPathElement;
    this.barEl.setAttribute("class", "ub-bar");
    this.barEl.setAttribute("d", d);
    frame.append(this.trackEl, this.barEl);

    this.labelEl = document.createElement("span");
    this.labelEl.className = "ub-lb";
    this.labelEl.textContent = "更新";

    const loading = document.createElement("span");
    loading.className = "ub-loading";
    loading.setAttribute("aria-hidden", "true");
    const spinner = document.createElement("span");
    spinner.className = "ub-spinner";
    loading.append(spinner);

    ub.append(frame, this.labelEl, loading);
    el.append(ub);
    el.addEventListener("click", () => void this.onClick());

    this.el = el;
    this.setPhase("hidden");
    this.renderProgress(0);
  }

  /** 应用启动后调用：先看是否有已就绪的安装包，再延迟检查新版本并定时复查。 */
  start(): void {
    if (this.started) return;
    this.started = true;
    this.stopPreferences = subscribeUpdatePreferences(() => {
      if (this.phase === "available" && this.manifest && isUpdateSkipped(this.manifest.version)) this.hide();
    });
    void this.restoreStatus();
    this.firstCheckTimer = window.setTimeout(() => void this.check(), FIRST_CHECK_DELAY_MS);
    this.checkTimer = window.setInterval(() => void this.check(), CHECK_INTERVAL_MS);
  }

  stop(): void {
    this.started = false;
    this.generation++;
    this.stopPreferences?.();
    this.stopPreferences = null;
    if (this.firstCheckTimer !== null) {
      window.clearTimeout(this.firstCheckTimer);
      this.firstCheckTimer = null;
    }
    if (this.checkTimer !== null) {
      window.clearInterval(this.checkTimer);
      this.checkTimer = null;
    }
    this.stopPolling();
    this.stopProgressListener();
  }

  private hasActiveUpdate(): boolean {
    return this.phase === "downloading" || this.phase === "ready" || this.phase === "applying";
  }

  /** 静默检查新版本；网络/未配置更新源时保持原状，不打扰用户。 */
  private async check(): Promise<void> {
    if (this.checking || this.hasActiveUpdate()) return;
    this.checking = true;
    const generation = this.generation;
    try {
      const result = await api.updateCheck();
      if (generation !== this.generation || this.hasActiveUpdate()) return;
      if (result.update_available && result.manifest) {
        this.setAvailable(result.manifest);
        if (this.phase === "available" && readUpdatePreferences().autoUpdate) await this.download();
      } else this.hide();
    } catch {
      /* 静默失败 */
    } finally { this.checking = false; }
  }

  private async restoreStatus(): Promise<void> {
    const generation = this.generation;
    try {
      const status = await api.updateStatus();
      if (generation !== this.generation || this.hasActiveUpdate()) return;
      if (status.phase === "ready") this.setReady();
      else if (status.phase === "downloading") this.enterDownloading(status.done, status.total);
    } catch {
      /* 非桌面 / 预览环境没有该状态 */
    }
  }

  private setPhase(phase: BadgePhase): void {
    this.phase = phase;
    this.el.dataset.phase = phase;
    this.el.hidden = phase === "hidden";
    const busy = phase === "downloading" || phase === "applying";
    this.el.disabled = busy;
    this.el.setAttribute("aria-busy", String(busy));
  }

  /** 发现新版本：显示绿色「更新」。 */
  setAvailable(manifest: UpdateManifest | null): void {
    if (this.phase === "applying") return;
    this.manifest = manifest;
    if (manifest && isUpdateSkipped(manifest.version)) { this.hide(); return; }
    this.setPhase("available");
    this.labelEl.textContent = "更新";
    const what = manifest && manifest.version ? "新版本 " + manifest.version : "有可用更新";
    this.el.title = what + "，点击下载";
    this.el.setAttribute("aria-label", what + "，点击下载更新");
  }

  private hide(): void {
    this.setPhase("hidden");
    this.labelEl.textContent = "更新";
    this.el.title = "已是最新版本";
  }

  private enterDownloading(done: number, total: number): void {
    if (this.phase === "applying" || this.phase === "ready") return;
    this.setPhase("downloading");
    this.labelEl.textContent = "更新";
    const pct = total > 0 ? (done / total) * 100 : 0;
    this.renderProgress(pct);
    this.el.title = total > 0 ? "正在下载更新 " + Math.round(pct) + "%" : "正在下载更新…";
    this.el.setAttribute("aria-label", "正在下载更新");
  }

  private setReady(): void {
    if (this.phase === "applying") return;
    this.stopPolling();
    this.stopProgressListener();
    this.setPhase("ready");
    this.renderProgress(100);
    this.labelEl.textContent = "重启";
    this.el.title = "更新已下载，点击重启并安装";
    this.el.setAttribute("aria-label", "重启并安装更新");
  }

  private renderProgress(pct: number): void {
    const clamped = Math.max(0, Math.min(100, pct));
    this.barEl.style.strokeDasharray = String(PERIMETER);
    this.barEl.style.strokeDashoffset = String(PERIMETER * (1 - clamped / 100));
  }

  private async onClick(): Promise<void> {
    // 原生 disabled 阻止用户输入；状态入口同时防止程序派发重复 click。
    if (this.el.disabled) return;
    if (this.phase === "available") {
      await this.download();
      return;
    }
    if (this.phase === "ready") await this.apply();
  }

  private async download(): Promise<void> {
    if (this.phase !== "available") return;
    const manifest = this.manifest;
    if (!manifest || isUpdateSkipped(manifest.version)) return;
    this.enterDownloading(0, 0);
    this.listenProgress();
    this.startPolling();
    const task = taskCenter.register("update", `软件更新下载 · v${manifest.version}`);
    try {
      const file = await downloadUpdate(manifest);
      task.finish("更新包已就绪，下次启动前可安装", file);
      this.setReady();
    } catch (e) {
      task.fail(String(e));
      this.stopPolling();
      this.stopProgressListener();
      this.setAvailable(this.manifest);
      this.notify?.("error", "更新下载失败：" + String(e));
    }
  }

  private async apply(): Promise<void> {
    if (this.phase !== "ready") return;
    this.setPhase("applying");
    this.el.title = "正在重启并安装更新…";
    this.el.setAttribute("aria-label", "正在重启并安装更新");
    try {
      await api.updateApply(readUpdatePreferences().pendingInstall?.sha256);
      // 成功回执不代表应用已退出，保持忙碌，避免重复安装。
    } catch (e) {
      this.setPhase("ready");
      this.setReady();
      this.el.title = "重启安装失败，点击重试：" + String(e);
      this.el.setAttribute("aria-label", "重启安装失败，点击重试");
      this.notify?.("error", "重启安装失败：" + String(e));
    }
  }

  private listenProgress(): void {
    this.stopProgressListener();
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        if (cancelled) return;
        unlisten = await listen<UpdateProgressEvent>("update-progress", (event) => {
          if (cancelled || this.phase === "applying") return;
          const p = event.payload;
          if (p.phase === "downloading") this.enterDownloading(p.done, p.total);
          else if (p.phase === "verifying") {
            this.renderProgress(100);
            this.el.title = "正在校验更新包…";
            this.el.setAttribute("aria-label", "正在校验更新包");
          } else if (p.phase === "ready") {
            this.setReady();
          } else if (p.phase === "error") {
            this.el.title = p.message ? "更新下载出错：" + p.message : "更新下载出错";
          }
        });
      } catch {
        /* 无事件 API 时靠 update_status 轮询兜底 */
      }
    })();
    this.stopEventListener = () => {
      cancelled = true;
      unlisten?.();
    };
  }

  private startPolling(): void {
    this.stopPolling();
    const generation = this.generation;
    this.pollTimer = window.setInterval(() => {
      if (this.phase !== "downloading") return;
      void api
        .updateStatus()
        .then((status) => {
          if (generation !== this.generation || this.phase !== "downloading") return;
          if (status.phase === "ready") this.setReady();
          else if (status.phase === "downloading") this.enterDownloading(status.done, status.total);
        })
        .catch(() => {
          /* 忽略轮询失败，等下一次 */
        });
    }, POLL_INTERVAL_MS);
  }

  private stopPolling(): void {
    if (this.pollTimer !== null) {
      window.clearInterval(this.pollTimer);
      this.pollTimer = null;
    }
  }

  private stopProgressListener(): void {
    if (this.stopEventListener) {
      this.stopEventListener();
      this.stopEventListener = null;
    }
  }
}
