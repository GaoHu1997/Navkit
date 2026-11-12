import type { App } from "../core/App";
import { api } from "../core/api";
import type { TabState } from "../core/types";
import type { DraftSnapshot } from "../core/editing-state";
import { RoutineEditorSession } from "../core/routine-editor";
import { RoutineCreationSession, type RoutineCreationBaseline } from "../core/routine-create";
import type { RoutineSnapshot } from "../core/object-catalog";
import { SqlEditor } from "./SqlEditor";

/** 精确例程的页面适配；既有对象和不存在基线分别保留，不借用普通DDL保存。 */
export class RoutineDocumentView {
  readonly el = document.createElement("div");
  private readonly editor: SqlEditor;
  private readonly error = document.createElement("div");
  private readonly stop = document.createElement("button");
  private readonly recheck = document.createElement("button");
  private session: RoutineEditorSession | null = null;
  private creation: RoutineCreationSession | null = null;
  private baseline: string;
  private loading = false;
  private disposed = false;
  private generation = 0;
  constructor(private readonly app: App, private readonly tab: TabState) {
    this.baseline = tab.sql ?? "";
    this.el.className = "tab-view routine-document-view";
    this.el.style.cssText = "display:flex;flex:1;flex-direction:column;min-height:0;gap:4px";
    this.error.className = "design-sql-error"; this.error.hidden = true;
    this.error.id = `routine-error-${tab.id}`; this.error.setAttribute("role", "alert"); this.error.style.padding = "6px 8px";
    this.editor = new SqlEditor({ value: this.baseline,
      onChange: sql => { tab.sql = sql; this.changed(); },
      onExecute: () => { void app.runCommand("save"); },
      onFormat: () => { void app.runCommand("beautify-sql"); },
    });
    this.editor.el.setAttribute("aria-describedby", this.error.id);
    const bar = document.createElement("div"); bar.style.cssText = "display:flex;gap:8px;align-items:center;padding:4px 8px;border-bottom:1px solid var(--border)";
    const note = document.createElement("span"); note.textContent = "保存使用打开时的定义或不存在基线；停止请求不是已取消 / 已回滚。";
    note.style.cssText = "font-size:12px;color:var(--text-secondary);flex:1";
    this.stop.type = "button"; this.stop.className = "btn"; this.stop.textContent = "停止执行"; this.stop.disabled = true;
    this.stop.addEventListener("click", () => { void (this.creation ?? this.session)?.stop().catch(error => this.showError(String(error))); });
    this.recheck.type = "button"; this.recheck.className = "btn"; this.recheck.textContent = "重新核对目标";
    this.recheck.title = "读取真实对象或重新证明不存在；不会执行CREATE，也不会自动重放上次执行";
    this.recheck.addEventListener("click", () => { void this.reload().catch(error => this.showError(String(error))); });
    bar.append(note, this.recheck, this.stop); this.el.append(bar, this.error, this.editor.el);
    try {
      if (tab.designCreating && tab.routineCreation) this.createCreationSession(tab.routineCreation);
      else if (tab.routineSnapshot) this.createSession(tab.routineSnapshot);
      else this.showError("没有打开时的完整对象 / 端点基线，仅保留SQL供复制。不能在点击保存时重读基线冒充CAS。");
    } catch (error) { this.showError(String(error)); }
    this.changed();
  }
  private async grant(request: Parameters<typeof api.issueGrant>[0]): ReturnType<typeof api.issueGrant> {
    const granted = await api.issueGrant(request);
    if (this.disposed || this.tab.datasourceStale || request.connection_id !== this.tab.connectionId || request.namespace !== this.tab.schema) throw new Error("例程页面目标在授权期间改变，没有发送SQL");
    return granted;
  }
  private createSession(snapshot: RoutineSnapshot): void {
    this.session = new RoutineEditorSession(snapshot, this.tab.id, {
      preview: draft => api.previewRoutineReplacement(draft), execute: (draft, request) => api.executeRoutineReplacement(draft, request),
      issueGrant: request => this.grant(request),
      cancelExecution: context => api.cancelExecution(context), executionStatus: context => api.executionStatus(context),
    }, () => { if (this.session?.message) this.showError(this.session.message); this.changed(); });
  }
  private createCreationSession(baseline: RoutineCreationBaseline): void {
    if (baseline.object.connection_id !== this.tab.connectionId || baseline.object.schema !== this.tab.schema || baseline.object.name !== this.tab.object || baseline.object.kind !== this.tab.objectType || baseline.object.identity_arguments !== "") throw new Error("创建基线与当前页面对象不匹配，没有发送SQL");
    this.creation = new RoutineCreationSession(baseline, this.tab.id, {
      preview: draft => api.previewRoutineCreation(draft, this.tab.id), execute: (draft, request) => api.executeRoutineCreation(draft, request),
      issueGrant: request => this.grant(request),
      cancelExecution: context => api.cancelExecution(context), executionStatus: context => api.executionStatus(context),
    }, () => { if (this.creation?.message) this.showError(this.creation.message); this.changed(); });
  }
  private changed(): void {
    if (this.disposed) return;
    this.tab.dirty = this.draftDirty(); this.el.setAttribute("aria-busy", String(this.draftBusy()));
    this.stop.disabled = !(this.creation ?? this.session)?.busy; this.recheck.disabled = this.draftBusy();
    this.app.notifyToolbar();
  }
  getSql(): string { return this.editor.getSql(); }
  /** 供标签页工具栏 / F6 共用同一编辑器实例。 */
  copyEditor(): SqlEditor { return this.editor; }
  markClean(sql: string): void { this.baseline = sql; this.tab.sql = this.getSql(); this.changed(); }
  showError(message: string): void { this.error.textContent = message; this.error.hidden = !message; }
  activate(): void { this.editor.focus(); }
  draftDirty(): boolean { return !!this.tab.designCreating || this.getSql() !== this.baseline || !!(this.creation ?? this.session)?.effectsUnknown; }
  draftBusy(): boolean { return this.loading || !!(this.creation ?? this.session)?.busy; }
  captureDraft(): Pick<DraftSnapshot, "kind" | "baseline" | "payload"> {
    return { kind: "routine", baseline: this.creation?.loadedBaseline ?? this.session?.loadedSnapshot ?? this.baseline,
      payload: { sql: this.getSql(), requires_validation: true, design_creating: !!this.tab.designCreating, ...(this.creation?.capture() ?? this.session?.capture() ?? {}) } };
  }
  discardDraft(): void { this.editor.setSql(this.baseline); this.changed(); }
  private acceptCreated(snapshot: RoutineSnapshot): void {
    const expected = this.creation?.loadedBaseline;
    if (!expected || snapshot.source_fingerprint !== expected.source_fingerprint || snapshot.object.connection_id !== expected.object.connection_id
      || snapshot.object.schema !== expected.object.schema || snapshot.object.kind !== expected.object.kind || snapshot.object.name !== expected.object.name
      || snapshot.object.identity_arguments !== expected.object.identity_arguments) throw new Error("创建后快照对象不一致，不能替换当前页面");
    this.createSession(snapshot); this.creation?.dispose(); this.creation = null;
    this.tab.routineObject = { ...snapshot.object, catalog: null, parent: null };
    this.tab.routineSnapshot = snapshot; this.tab.routineCreation = undefined; this.tab.designCreating = false;
  }
  async saveRoutine(): Promise<boolean> {
    if (this.disposed || this.draftBusy()) throw new Error("例程尚在执行 / 排空或页面已关闭");
    if (this.tab.datasourceStale) throw new Error("连接上下文已改变，请明确重读精确对象；不补造旧基线");
    if (!this.session && !this.creation) throw new Error("缺少打开时的端点 / 定义基线，当前文本不会发送");
    const submitted = this.getSql();
    if (this.creation) {
      const saved = await this.creation.save(submitted);
      const snapshot = this.creation.createdSnapshot;
      if (saved && snapshot) { this.acceptCreated(snapshot); this.markClean(submitted); this.showError(""); }
      return saved;
    }
    const saved = await this.session!.save(submitted);
    if (saved) { this.tab.routineSnapshot = this.session!.loadedSnapshot; this.markClean(submitted); this.showError(""); }
    return saved;
  }
  async reload(): Promise<void> {
    if (this.disposed || this.draftBusy()) throw new Error("尚在执行 / 排空，不能重读或清除未知证据");
    if (this.tab.datasourceStale) throw new Error("连接上下文已改变，请在新页面明确选择目标");
    if (!this.tab.routineObject && !this.creation) throw new Error("缺少精确对象身份，不能按名字或空签名重读");
    const before = this.getSql(), generation = ++this.generation;
    this.loading = true; this.changed();
    try {
      if (this.creation) {
        const original = this.creation.loadedBaseline;
        try {
          const baseline = await api.readRoutineCreationBaseline(original.object, this.tab.id);
          if (this.disposed || generation !== this.generation || this.getSql() !== before) throw new Error("核对期间编辑已变化，当前SQL和基线保留");
          this.creation.replaceBaseline(baseline); this.tab.routineCreation = baseline;
          this.showError("已明确重新核对真实目标不存在；创建文本保留，请检查后保存。");
        } catch (error) {
          const code = (error as { code?: string })?.code;
          if (code !== "ROUTINE_CREATION_ALREADY_EXISTS" && !String(error).includes("ROUTINE_CREATION_ALREADY_EXISTS")) throw error;
          const snapshot = await api.readRoutineDesignObject({ ...original.object, catalog: null, parent: null });
          if (this.disposed || generation !== this.generation || this.getSql() !== before) throw new Error("读取期间编辑已变化，当前SQL和基线保留");
          this.acceptCreated(snapshot);
          // 上一次可能已提交；明确核对只更新真实基线，编辑文本保持原样供人工合并。
          this.baseline = snapshot.definition; this.tab.sql = this.getSql();
          this.showError("目标已存在，已读取精确对象基线并保留当前文本；请核对后再决定是否替换。");
        }
      } else {
        const snapshot = await api.readRoutineDesignObject(this.tab.routineObject!);
        if (this.disposed || generation !== this.generation || this.getSql() !== before) throw new Error("读取期间编辑已变化，当前SQL和基线保留");
        if (this.session) this.session.replaceLoaded(snapshot); else this.createSession(snapshot);
        this.tab.routineSnapshot = snapshot; this.editor.setSql(snapshot.definition); this.baseline = snapshot.definition; this.showError("");
      }
    } catch (error) { this.showError(String(error)); throw error; }
    finally { this.loading = false; this.changed(); }
  }
  dispose(): void { this.disposed = true; ++this.generation; this.session?.dispose(); this.creation?.dispose(); this.editor.destroy(); }
}
