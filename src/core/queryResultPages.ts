/** 查询结果单页状态：只读取原执行快照，不积累已访问页，也不重跑 SQL。 */
import type { MultiResult, QueryRowsPage, QuerySpoolScope, ResultSet } from "./types";

export interface QueryResultPage {
  offset: number;
  /** 当前结果集的每页行数；切换大小成功后才更新。 */
  limit: number;
  rows: ResultSet["rows"];
  total: number;
  loading: boolean;
  truncated: boolean;
  message: string;
  request: number;
  /** 只保存访问起点，不保存旧页行数据；支持字节截短后的准确上一页。 */
  previous: number[];
}

export class QueryResultPages {
  readonly limit: number;
  result: MultiResult | null = null;
  scope: QuerySpoolScope | null = null;
  private pages: QueryResultPage[] = [];
  private generation = 0;

  constructor(limit = 200) { this.limit = Math.max(1, Math.min(1000, Math.floor(limit) || 200)); }
  get busy(): boolean { return this.pages.some(page => page.loading); }
  page(index: number): QueryResultPage | null { return this.pages[index] ?? null; }

  replace(result: MultiResult | null, context?: Omit<QuerySpoolScope, "spoolId">): QuerySpoolScope | null {
    const old = this.scope;
    ++this.generation;
    this.result = result;
    this.scope = result?.spool_id && context ? { ...context, spoolId: result.spool_id } : null;
    this.pages = result?.sets.map(set => {
      // 快照路径只保留首屏；历史无 spool 路径保留原有已加载行，绝不声称全量。
      if (this.scope) set.rows = set.rows.slice(0, this.limit);
      return { offset: 0, limit: this.limit, rows: set.rows, total: Math.max(set.total, set.rows.length), loading: false,
        truncated: false, message: set.message || "", request: 0, previous: [] };
    }) ?? [];
    return old?.spoolId !== this.scope?.spoolId ? old : null;
  }

  range(index: number): string {
    const page = this.page(index), set = this.result?.sets[index];
    if (!page || !set) return "";
    const start = page.rows.length ? page.offset + 1 : 0;
    const end = page.offset + page.rows.length;
    return this.scope ? `第 ${start}–${page.rows.length ? end : 0} 行 / 共 ${page.total} 行`
      : `已加载 ${set.rows.length} / 共 ${set.total} 行（无完整快照，仅可导出已加载行）`;
  }

  pageLimit(index: number): number { return this.page(index)?.limit ?? this.limit; }
  pageCount(index: number): number { return Math.ceil((this.page(index)?.total ?? 0) / this.pageLimit(index)); }
  pageNumber(index: number): number {
    const page = this.page(index);
    return page?.total ? Math.floor(page.offset / page.limit) + 1 : 0;
  }
  pageOffset(index: number, number: number): number | null {
    return Number.isSafeInteger(number) && number >= 1 && number <= this.pageCount(index) ? (number - 1) * this.pageLimit(index) : null;
  }

  offset(index: number, action: "first" | "previous" | "next" | "last"): number {
    const page = this.page(index);
    if (!page) return 0;
    if (action === "first") return 0;
    if (action === "previous") return page.previous[page.previous.length - 1] ?? Math.max(0, page.offset - page.limit);
    if (action === "next") return page.offset + page.rows.length;
    return this.pageOffset(index, this.pageCount(index)) ?? 0;
  }

  async fetch(index: number, offset: number,
    read: (scope: QuerySpoolScope, index: number, offset: number, limit: number) => Promise<QueryRowsPage>,
    limit = this.pageLimit(index)): Promise<boolean> {
    const page = this.page(index), scope = this.scope, result = this.result, generation = this.generation;
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 1000 || !page || !scope || !result || page.loading || offset < 0 || offset >= page.total && page.total !== 0) return false;
    const request = ++page.request;
    page.loading = true; page.message = "";
    try {
      const data = await read(scope, index, offset, limit);
      if (generation !== this.generation || request !== page.request || result !== this.result) return false;
      if (limit !== page.limit) page.previous = [];
      else if (offset > page.offset) page.previous.push(page.offset);
      else if (offset === 0) page.previous = [];
      else if (offset === page.previous[page.previous.length - 1]) page.previous.pop();
      page.limit = limit; page.offset = offset; page.rows = data.rows; page.total = data.total;
      page.truncated = data.truncated; page.message = data.message;
      // 同步唯一当前页，释放上一页；网格/复制/旧路径导出都不会混入其他结果页。
      result.sets[index].rows = data.rows;
      return true;
    } catch (error) {
      if (generation !== this.generation || request !== page.request) return false;
      page.message = `读取缓存页失败：${String(error)}。保留当前页，可重试；不会重新执行 SQL。`;
      throw error;
    } finally {
      if (generation === this.generation && request === page.request) page.loading = false;
    }
  }
}
