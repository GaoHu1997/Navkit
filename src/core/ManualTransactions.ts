/** 手动事务界面控制器。状态缓存仅服务界面，事务连接/串行门禁由原生后端持有。 */
import { manualTransactionUnavailableReason, type ManualTransactionStatus } from "./manualTransaction";
import type { ConnectionConfig } from "./types";
import { confirmTransactionAction } from "../dialogs/TransactionDialog";

export interface TransactionContext {
  id: string;
  connectionId: string;
  schema: string;
  kind: string;
  running: boolean;
  pendingEdits: boolean;
  prepare: () => Promise<void>;
}
export interface TransactionHost {
  context: (id?: string) => TransactionContext | undefined;
  connection: (id: string) => ConnectionConfig | undefined;
  createQuery: (id: string, schema: string) => Promise<TransactionContext>;
  removeTab: (id: string) => void;
  focus: (id: string) => void;
  changed: () => void;
  toast: (kind: "info" | "error" | "success", message: string) => void;
}
export interface TransactionApi {
  startManualTransaction: (id: string, schema: string, owner: string) => Promise<ManualTransactionStatus>;
  manualTransactionStatus: (id: string) => Promise<ManualTransactionStatus>;
  finishManualTransaction: (id: string, owner: string, commit: boolean) => Promise<ManualTransactionStatus>;
  closeConnection: (id: string) => Promise<unknown>;
}
type Confirmation = (title: string, message: string, action: string) => Promise<boolean>;

export class ManualTransactions {
  private states = new Map<string, ManualTransactionStatus>();
  private pending = new Set<string>();
  private generations = new Map<string, number>();
  constructor(private source: TransactionApi, private host: TransactionHost, private confirm: Confirmation = confirmTransactionAction) {}

  status(id: string): ManualTransactionStatus | undefined { return this.states.get(id); }
  busy(id: string): boolean { return this.pending.has(id); }
  hasActive(): boolean { return this.states.size > 0 || this.pending.size > 0; }
  private changed(): void { this.host.changed(); }
  private bump(id: string): number { const next = (this.generations.get(id) || 0) + 1; this.generations.set(id, next); return next; }
  private publish(id: string, state: ManualTransactionStatus): void {
    this.bump(id);
    if (state.active) this.states.set(id, { ...state }); else this.states.delete(id);
    this.changed();
  }
  private lock(id: string): void {
    if (this.busy(id)) throw new Error("事务操作正在进行，请等待完成");
    this.pending.add(id); this.changed();
  }
  private unlock(id: string): void { this.pending.delete(id); this.changed(); }
  async refresh(id: string): Promise<ManualTransactionStatus | undefined> {
    if (!id) return undefined;
    const generation = this.bump(id);
    const result = await this.source.manualTransactionStatus(id);
    // 显式 begin/finish/close 或后来的状态请求优先，迟到的响应不能重新开启已结束事务。
    if (generation === this.generations.get(id)) this.publish(id, result);
    return this.status(id);
  }
  private assertQuery(context: TransactionContext, allowBusy = false): void {
    // 事务所有者：查询页（SQL 执行）或表数据页（表格可视化编辑）。
    if (!["query", "table"].includes(context.kind)) throw new Error("事务只能由查询或表数据页面提交或回滚");
    if (context.running) throw new Error("SQL 正在执行，请等待完成后操作事务");
    if (!allowBusy && this.busy(context.connectionId)) throw new Error("事务操作正在进行");
  }
  private assertOwner(context: TransactionContext, commit = false): ManualTransactionStatus {
    this.assertQuery(context, true);
    const state = this.status(context.connectionId);
    if (!state?.active) throw new Error("此连接没有正在进行的手动事务");
    if (state.owner !== context.id || state.schema !== context.schema) throw new Error("事务由另一个查询页或模式持有，请返回事务所有者页面");
    if (commit && state.failed) throw new Error("事务执行失败，必须回滚，不能提交");
    return state;
  }
  private checkedContext(id: string): TransactionContext {
    const context = this.host.context(id);
    if (!context) throw new Error("原查询页面已关闭，操作已取消");
    return context;
  }
  private assertFinished(result: ManualTransactionStatus): void {
    if (result.active) throw new Error("后端仍报告事务进行中，不能认为提交或回滚已完成");
  }

  async begin(): Promise<boolean> {
    const original = this.host.context();
    if (!original) { this.host.toast("error", "请先打开表数据或查询页面"); return false; }
    const id = original.connectionId;
    let locked = false;
    try {
      const unavailable = manualTransactionUnavailableReason(this.host.connection(id));
      if (unavailable) throw new Error(unavailable);
      if (!["query", "table"].includes(original.kind)) throw new Error("请在表数据或查询页面开始事务");
      if (original.running) throw new Error("SQL 正在执行，请等待完成");
      if (original.pendingEdits) throw new Error("表数据有待提交改动，请先保存或取消，再开始事务");
      this.lock(id); locked = true;
      await this.refresh(id);
      if (this.status(id)) throw new Error("此连接已有手动事务，请先在所有者页面提交或回滚");
      const fromTable = original.kind === "table";
      const accepted = await this.confirm(
        "开始手动事务",
        fromTable
          ? "事务将绑定当前表数据页：此后表格中的编辑、插入、删除都会在该事务连接上执行，提交前仅本事务可见；其它自动提交写入入口暂时停用。请用工具栏的「提交事务 / 回滚事务」结束事务。"
          : "此事务绑定当前连接和查询页。SQL 更改在提交前仅本事务可见；其它自动提交写入入口暂时停用。请明确提交或回滚，关闭事务页面将请求回滚。",
        "开始事务",
      );
      if (!accepted) return false;
      const current = this.host.context();
      if (!current || current.id !== original.id || current.connectionId !== id || current.schema !== original.schema) throw new Error("当前页面或数据库已变化，事务操作已取消");
      // 表数据页事务直接绑定本页面（表格可视化操作）；查询页事务绑定查询页本身。
      const owner = fromTable ? current : current;
      this.assertQuery(owner, true);
      await owner.prepare();
      const checked = this.checkedContext(owner.id);
      if (this.host.context()?.id !== checked.id || checked.connectionId !== id || (original.schema && checked.schema !== original.schema)) throw new Error("当前页面或原数据库/模式已变化，事务操作已取消");
      this.assertQuery(checked, true);
      if (!checked.schema) throw new Error("没有可用的真实数据库/模式，请先选择数据库");
      const result = await this.source.startManualTransaction(id, checked.schema, checked.id);
      this.publish(id, result);
      if (!result.active || result.owner !== checked.id || result.schema !== checked.schema) throw new Error("后端事务所有者或模式不匹配，未确认开始成功");
      this.host.toast("success", fromTable ? "手动事务已开始：表格中的编辑将在提交前仅本事务可见" : "手动事务已开始，更改尚未提交");
      this.host.focus(checked.id);
      return true;
    } catch (error) {
      this.host.toast("error", String(error));
      return false;
    } finally { if (locked) this.unlock(id); }
  }

  async finish(commit: boolean): Promise<boolean> {
    const context = this.host.context();
    if (!context) { this.host.toast("error", "请返回事务所属查询页面"); return false; }
    const id = context.connectionId;
    let locked = false;
    try {
      this.assertOwner(context, commit);
      this.lock(id); locked = true;
      const accepted = await this.confirm(commit ? "提交事务" : "回滚事务", commit ? "提交会将本事务的全部更改写入数据库，之后无法通过回滚撤销。确定提交吗？" : "回滚会放弃本事务中尚未提交的全部数据库更改。确定回滚吗？", commit ? "提交" : "回滚");
      if (!accepted) return false;
      const current = this.checkedContext(context.id);
      if (this.host.context()?.id !== current.id) throw new Error("当前查询页面已变化，事务操作已取消");
      this.assertOwner(current, commit);
      const result = await this.source.finishManualTransaction(id, current.id, commit);
      this.assertFinished(result);
      this.publish(id, result);
      this.host.toast("success", commit ? "事务已提交" : "事务已回滚");
      this.host.focus(current.id);
      return true;
    } catch (error) {
      this.host.toast("error", String(error));
      // 提交失败可能已结束/丢弃后端连接；失败的状态请求绝不能擅自清除已有状态。
      if (locked) await this.refresh(id).catch(() => undefined);
      return false;
    } finally { if (locked) this.unlock(id); }
  }

  async closeOwner(context: TransactionContext): Promise<boolean> {
    const id = context.connectionId;
    let locked = false;
    try {
      this.assertOwner(context);
      this.lock(id); locked = true;
      if (!await this.confirm("关闭查询并回滚", "此查询持有尚未提交的事务。关闭前必须回滚全部未提交更改；取消可保留查询和事务。", "回滚并关闭")) return false;
      this.assertOwner(this.checkedContext(context.id));
      const result = await this.source.finishManualTransaction(id, context.id, false);
      this.assertFinished(result);
      this.publish(id, result);
      this.host.removeTab(context.id);
      this.host.toast("info", "事务已回滚，查询页面已关闭");
      return true;
    } catch (error) {
      this.host.toast("error", String(error));
      if (locked) await this.refresh(id).catch(() => undefined);
      return false;
    }
    finally { if (locked) this.unlock(id); }
  }

  async closeConnection(id: string): Promise<boolean> {
    let locked = false;
    try {
      this.lock(id); locked = true;
      await this.refresh(id);
      const state = this.status(id);
      if (state && !await this.confirm("关闭连接并回滚", "此连接有尚未提交的手动事务。关闭连接将回滚全部未提交更改，确定关闭吗？", "回滚并关闭连接")) return false;
      await this.source.closeConnection(id);
      this.publish(id, { active: false, schema: "", owner: "", failed: false });
      return true;
    } catch (error) {
      this.host.toast("error", String(error));
      if (locked) await this.refresh(id).catch(() => undefined);
      return false;
    }
    finally { if (locked) this.unlock(id); }
  }
}
