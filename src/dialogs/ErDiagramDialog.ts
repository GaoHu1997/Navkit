/**
 * 关系图对话框（需求 1.4，参考 dbx）：数据库可视化建模。
 *
 *  · 两种视图：表结构图（表卡片 + FK 折线边 + 基数标记，如 N / 1）/ 工程 ER 图（表-列-菱形列级图）
 *  · 交互：拖拽卡片、平移、滚轮缩放（以光标为锚）、适应画布、自动布局、搜索过滤
 *  · 右侧表设计器：选中表即编辑字段 / 索引 / 外键 / 检查 / 触发器 / 选项 / 注释，并预览后端 DDL 计划
 *  · 新建表 / 删除表：执行真实 DDL 后重载元数据
 *  · 同步到数据库：把当前表的结构改动交给后端生成实际计划、确认后受控执行
 *  · 复制 SQL：按当前关系生成 LEFT JOIN 语句
 *  · 卡片位置按 连接+库 粒度持久化（localStorage）
 */

import { api } from "../core/api";
import type { ConnectionConfig } from "../core/types";
import { Dialog } from "../ui/Dialog";
import { iconSvgByName } from "../ui/icons";
import { ErTableEditor, type ErTableEditorHost } from "../ui/ErTableEditor";
import { accessibleDialog } from "./SyncDialog";
import {
  buildRelationships,
  buildJoinSql,
  computeEngineeringLayout,
  computeLayout,
  edgeEndpoints,
  orthogonalPath,
  filterTables,
  loadPositions,
  savePositions,
  type Cardinality,
  type ErRelationship,
  type ErTable,
} from "../core/erDiagram";
import "./../styles/er-diagram.css";

interface Viewport {
  x: number;
  y: number;
  zoom: number;
}

export function openErDiagramDialog(
  connections: ConnectionConfig[],
  current: ConnectionConfig | undefined,
  currentSchema: string,
): Dialog {
  const dlg = new Dialog({ title: "关系图", width: 1280, render: () => {}, buttons: [{ label: "关闭" }] });
  dlg.el.classList.add("er-dialog");
  dlg.body.style.padding = "0";
  dlg.body.style.display = "flex";
  dlg.body.style.flexDirection = "column";
  dlg.body.style.overflow = "hidden";

  // ---------------------------------------------------------------- 状态
  let connectionId = current?.id ?? connections[0]?.id ?? "";
  let database = currentSchema;
  let tables: ErTable[] = [];
  let relationships: ErRelationship[] = [];
  let visibleNames = new Set<string>();
  let nodes: ReturnType<typeof computeLayout> = [];
  let mode: "tables" | "engineering" = "tables";
  let viewport: Viewport = { x: 0, y: 0, zoom: 1 };
  let selectedTable: string | null = null;
  let loading = false;
  const instanceId = Math.random().toString(36).slice(2, 8);
  /** 每张表一个独立表设计器（owner 唯一）；切换表不丢失未保存草稿。 */
  const designers = new Map<string, ErTableEditor>();

  // ---------------------------------------------------------------- 工具栏
  const toolbar = document.createElement("div");
  toolbar.className = "er-toolbar";

  const connSelect = document.createElement("select");
  connSelect.className = "er-select";
  connSelect.setAttribute("aria-label", "连接");
  for (const c of connections) {
    const opt = document.createElement("option");
    opt.value = c.id;
    opt.textContent = c.name;
    connSelect.appendChild(opt);
  }
  if (current) connSelect.value = current.id;

  const dbSelect = document.createElement("select");
  dbSelect.className = "er-select";
  dbSelect.setAttribute("aria-label", "数据库");

  const search = document.createElement("input");
  search.type = "search";
  search.className = "er-search";
  search.placeholder = "搜索表 / 字段 / 外键";
  search.setAttribute("aria-label", "搜索表");

  const modeGroup = document.createElement("div");
  modeGroup.className = "er-mode-group";
  modeGroup.setAttribute("role", "radiogroup");
  modeGroup.setAttribute("aria-label", "视图模式");
  const modeTables = document.createElement("button");
  modeTables.type = "button";
  modeTables.className = "er-mode-btn";
  modeTables.textContent = "表结构图";
  const modeEng = document.createElement("button");
  modeEng.type = "button";
  modeEng.className = "er-mode-btn";
  modeEng.textContent = "工程 ER 图";
  modeGroup.append(modeTables, modeEng);

  const newTableBtn = document.createElement("button");
  newTableBtn.type = "button";
  newTableBtn.className = "er-tool-btn";
  newTableBtn.textContent = "＋ 新建表";
  newTableBtn.addEventListener("click", () => cmdNewTable());

  const syncBtn = document.createElement("button");
  syncBtn.type = "button";
  syncBtn.className = "er-tool-btn primary";
  syncBtn.title = "把当前选中表的结构改动生成后端计划并执行";
  const syncLabel = document.createElement("span");
  syncLabel.textContent = "同步到数据库";
  const syncBadge = document.createElement("span");
  syncBadge.className = "er-badge";
  syncBadge.hidden = true;
  syncBtn.append(syncLabel, syncBadge);
  syncBtn.addEventListener("click", () => void syncCurrent());

  const layoutBtn = document.createElement("button");
  layoutBtn.type = "button";
  layoutBtn.className = "er-tool-btn";
  layoutBtn.textContent = "自动布局";
  const fitBtn = document.createElement("button");
  fitBtn.type = "button";
  fitBtn.className = "er-tool-btn";
  fitBtn.textContent = "适应画布";
  const copySqlBtn = document.createElement("button");
  copySqlBtn.type = "button";
  copySqlBtn.className = "er-tool-btn";
  copySqlBtn.textContent = "复制 SQL";
  const refreshBtn = document.createElement("button");
  refreshBtn.type = "button";
  refreshBtn.className = "er-tool-btn icon";
  refreshBtn.setAttribute("aria-label", "刷新");
  refreshBtn.innerHTML = iconSvgByName("dbn_refresh");
  const zoomOutBtn = document.createElement("button");
  zoomOutBtn.type = "button";
  zoomOutBtn.className = "er-tool-btn icon";
  zoomOutBtn.setAttribute("aria-label", "缩小");
  zoomOutBtn.textContent = "−";
  const zoomInBtn = document.createElement("button");
  zoomInBtn.type = "button";
  zoomInBtn.className = "er-tool-btn icon";
  zoomInBtn.setAttribute("aria-label", "放大");
  zoomInBtn.textContent = "+";

  const stats = document.createElement("div");
  stats.className = "er-stats";
  stats.textContent = "未加载数据";

  toolbar.append(connSelect, dbSelect, search, modeGroup, newTableBtn, layoutBtn, fitBtn, copySqlBtn, syncBtn, refreshBtn, stats, zoomOutBtn, zoomInBtn);
  dlg.body.appendChild(toolbar);

  // 画布与右侧表设计器左右分栏。
  const workspace = document.createElement("div");
  workspace.className = "er-workspace";
  dlg.body.appendChild(workspace);

  // ---------------------------------------------------------------- 画布
  const canvas = document.createElement("div");
  canvas.className = "er-canvas";
  const viewportEl = document.createElement("div");
  viewportEl.className = "er-viewport";
  const edgeSvg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  edgeSvg.classList.add("er-edges");
  const cardsLayer = document.createElement("div");
  cardsLayer.className = "er-cards";
  viewportEl.append(edgeSvg, cardsLayer);
  canvas.appendChild(viewportEl);
  workspace.appendChild(canvas);

  const hint = document.createElement("div");
  hint.className = "er-hint";
  canvas.appendChild(hint);

  // 右侧表设计器（字段 / 索引 / 外键 / 检查 / 触发器 / 选项 / 注释 / SQL 预览）
  const inspector = document.createElement("aside");
  inspector.className = "er-inspector";
  const inspHead = document.createElement("div");
  inspHead.className = "er-insp-head";
  const inspTitle = document.createElement("div");
  inspTitle.className = "er-insp-title";
  inspTitle.textContent = "表设计";
  const inspMessage = document.createElement("div");
  inspMessage.className = "er-insp-message";
  const deleteTableBtn = document.createElement("button");
  deleteTableBtn.type = "button";
  deleteTableBtn.className = "er-insp-delete";
  deleteTableBtn.textContent = "删除表";
  deleteTableBtn.addEventListener("click", () => cmdDropTable());
  inspHead.append(inspTitle, inspMessage, deleteTableBtn);
  const inspBody = document.createElement("div");
  inspBody.className = "er-insp-body";
  inspector.append(inspHead, inspBody);
  workspace.appendChild(inspector);

  // 状态覆盖层（加载 / 错误）
  const overlay = document.createElement("div");
  overlay.className = "er-overlay";
  dlg.body.appendChild(overlay);

  function showOverlay(text: string, isError = false): void {
    overlay.hidden = false;
    overlay.classList.toggle("error", isError);
    overlay.textContent = text;
  }
  function hideOverlay(): void {
    overlay.hidden = true;
  }

  // ---------------------------------------------------------------- 数据加载

  async function loadDatabases(prefer: string): Promise<void> {
    dbSelect.replaceChildren();
    if (!connectionId) return;
    try {
      const dbs = await api.listDatabases(connectionId);
      for (const [name] of dbs) {
        const opt = document.createElement("option");
        opt.value = name;
        opt.textContent = name;
        dbSelect.appendChild(opt);
      }
      if (prefer && dbs.some(([name]) => name === prefer)) dbSelect.value = prefer;
      else if (dbs.length) database = dbs[0][0];
    } catch {
      /* 数据库列表读取失败：保持现状 */
    }
  }

  /** 并发受限地拉取全部表结构。 */
  async function loadTables(): Promise<void> {
    const conn = connections.find((c) => c.id === connectionId);
    if (!conn || !database) throw new Error("请先选择连接与数据库");
    const list = await api.listTables(connectionId, database);
    const tableNames = list.filter(([, ty]) => ty === "table").map(([name]) => name);
    tables = [];
    const queue = [...tableNames];
    const concurrency = 6;
    const workers = Array.from({ length: Math.min(concurrency, queue.length) }, async () => {
      while (queue.length) {
        const name = queue.shift()!;
        try {
          const schema = await api.getTableSchema(connectionId, database, name);
          tables.push({
            name,
            comment: schema.comment ?? "",
            columns: schema.columns.map((c) => ({
              name: c.name,
              type: c.type,
              primary_key: c.primary_key,
              auto_increment: c.auto_increment,
              nullable: c.nullable,
              comment: c.comment,
              length: c.length,
            })),
            foreignKeys: schema.foreign_keys.map((fk) => ({
              name: fk.name,
              columns: fk.columns,
              ref_table: fk.ref_table,
              ref_columns: fk.ref_columns,
            })),
          });
        } catch {
          // 单表失败不影响整体（跳过）
        }
      }
    });
    await Promise.all(workers);
    tables.sort((a, b) => a.name.localeCompare(b.name));
    relationships = buildRelationships(tables);
    visibleNames = new Set(tables.map((t) => t.name));
    if (!selectedTable || !tables.some((t) => t.name === selectedTable)) selectedTable = tables[0]?.name ?? null;
  }

  async function refresh(preferDb?: string): Promise<void> {
    if (loading) return;
    loading = true;
    showOverlay("正在加载表结构...");
    try {
      if (preferDb !== undefined || dbSelect.options.length === 0) await loadDatabases(preferDb ?? database);
      await loadTables();
      rebuildLayout(true);
      hideOverlay();
      renderAll();
    } catch (e) {
        showOverlay(`加载失败：${String(e)}（点击「刷新」重试）`, true);
    } finally {
      loading = false;
    }
  }

  // ---------------------------------------------------------------- 布局与渲染

  function rebuildLayout(resetPositions: boolean): void {
    const filtered = filterTables(tables, search.value);
    visibleNames = new Set(filtered.map((t) => t.name));
    nodes = computeLayout(filtered, relationships);
    // 恢复用户拖拽过的位置
    const positions = resetPositions ? {} : loadPositions(connectionId, database);
    if (!resetPositions) {
      for (const node of nodes) {
        const saved = positions[node.table.name];
        if (saved) {
          node.x = saved.x;
          node.y = saved.y;
        }
      }
    }
    applyViewport();
  }

  function applyViewport(): void {
    viewportEl.style.transform = `translate(${viewport.x}px, ${viewport.y}px) scale(${viewport.zoom})`;
  }

  function fitView(): void {
    let minX = 0;
    let minY = 0;
    let maxX = 0;
    let maxY = 0;
    if (mode === "tables") {
      if (!nodes.length) {
        viewport = { x: 0, y: 0, zoom: 1 };
        applyViewport();
        return;
      }
      minX = Math.min(...nodes.map((n) => n.x));
      minY = Math.min(...nodes.map((n) => n.y));
      maxX = Math.max(...nodes.map((n) => n.x + n.width));
      maxY = Math.max(...nodes.map((n) => n.y + n.height));
    } else if (engDiagram) {
      const d = engDiagram;
      minX = 0;
      minY = 0;
      maxX = d.width;
      maxY = d.height;
    } else {
      viewport = { x: 0, y: 0, zoom: 1 };
      applyViewport();
      return;
    }
    const boxW = maxX - minX + 160;
    const boxH = maxY - minY + 160;
    const rect = canvas.getBoundingClientRect();
    const zoom = Math.max(0.15, Math.min(1.4, Math.min(rect.width / boxW, rect.height / boxH)));
    viewport = {
      x: (rect.width - boxW * zoom) / 2 - minX * zoom + 80 * zoom,
      y: (rect.height - boxH * zoom) / 2 - minY * zoom + 80 * zoom,
      zoom,
    };
    applyViewport();
  }

  function renderStats(): void {
    const relCount = relationships.filter((r) => visibleNames.has(r.sourceTable) && visibleNames.has(r.targetTable)).length;
    const pending = pendingCount();
    stats.textContent = `${visibleNames.size} 张表 · ${relCount} 条关系 · ${Math.round(viewport.zoom * 100)}%${pending ? ` · ${pending} 项待同步` : ""}`;
  }

  /** 渲染全部（按模式）。 */
  function renderAll(): void {
    modeTables.classList.toggle("active", mode === "tables");
    modeEng.classList.toggle("active", mode === "engineering");
    renderStats();
    if (mode === "tables") renderTablesView();
    else renderEngineeringView();
    updateInspector();
    renderPending();
  }

  function renderTablesView(): void {
    cardsLayer.replaceChildren();
    edgeSvg.replaceChildren();
    const byName = new Map(nodes.map((n) => [n.table.name, n]));
    // 边（先画，垫底）
    const rendered = new Set<string>();
    for (const rel of relationships) {
      if (!visibleNames.has(rel.sourceTable) || !visibleNames.has(rel.targetTable)) continue;
      const key = rel.id;
      if (rendered.has(key)) continue;
      rendered.add(key);
      const s = byName.get(rel.sourceTable);
      const t = byName.get(rel.targetTable);
      if (!s || !t) continue;
      const e = edgeEndpoints(s, t);
      const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
      path.setAttribute("d", orthogonalPath(e));
      path.classList.add("er-edge");
      path.dataset.source = rel.sourceTable;
      path.dataset.target = rel.targetTable;
      edgeSvg.appendChild(path);
      // 基数徽标：源端 N / 目标端 1
      edgeSvg.appendChild(cardinalityBadge(e.x1, e.y1, rel.sourceCardinality, "source"));
      edgeSvg.appendChild(cardinalityBadge(e.x2, e.y2, rel.targetCardinality, "target"));
    }
    // 卡片
    for (const node of nodes) {
      cardsLayer.appendChild(renderCard(node));
    }
    // SVG 尺寸
    const width = Math.max(...nodes.map((n) => n.x + n.width), 400) + 200;
    const height = Math.max(...nodes.map((n) => n.y + n.height), 300) + 200;
    edgeSvg.setAttribute("width", String(width));
    edgeSvg.setAttribute("height", String(height));
    edgeSvg.setAttribute("viewBox", `0 0 ${width} ${height}`);
  }

  function cardinalityBadge(x: number, y: number, value: Cardinality, side: "source" | "target"): SVGElement {
    const g = document.createElementNS("http://www.w3.org/2000/svg", "g");
    g.classList.add("er-card-badge");
    g.classList.add(side);
    const text = document.createElementNS("http://www.w3.org/2000/svg", "text");
    text.setAttribute("x", String(x + (side === "source" ? 8 : -14)));
    text.setAttribute("y", String(y - 6));
    text.textContent = value;
    g.appendChild(text);
    return g;
  }

  function renderCard(node: ReturnType<typeof computeLayout>[number]): HTMLElement {
    const card = document.createElement("div");
    card.className = "er-card";
    card.style.left = `${node.x}px`;
    card.style.top = `${node.y}px`;
    card.style.width = `${node.width}px`;
    card.dataset.table = node.table.name;
    if (selectedTable === node.table.name) card.classList.add("selected");

    const header = document.createElement("div");
    header.className = "er-card-header";
    const icon = document.createElement("span");
    icon.className = "er-card-icon";
    icon.innerHTML = iconSvgByName("info_pane_table");
    const name = document.createElement("span");
    name.className = "er-card-name";
    name.textContent = node.table.name;
    name.title = node.table.comment ? `${node.table.name} · ${node.table.comment}` : node.table.name;
    const count = document.createElement("span");
    count.className = "er-card-count";
    count.textContent = String(node.table.columns.length);
    header.append(icon, name, count);

    const body = document.createElement("div");
    body.className = "er-card-body";
    for (const col of node.table.columns) {
      const row = document.createElement("div");
      row.className = "er-card-row";
      const keyIcon = document.createElement("span");
      keyIcon.className = "er-col-icon";
      if (col.primary_key) keyIcon.innerHTML = iconSvgByName("info_pane_pk");
      else if (node.table.foreignKeys.some((fk) => fk.columns.includes(col.name))) keyIcon.innerHTML = iconSvgByName("info_pane_fk");
      const colName = document.createElement("span");
      colName.className = "er-col-name";
      colName.textContent = col.name;
      colName.title = col.comment ?? col.name;
      const colType = document.createElement("span");
      colType.className = "er-col-type";
      colType.textContent = col.type + (col.length ? `(${col.length})` : "");
      row.append(keyIcon, colName, colType);
      body.appendChild(row);
    }
    card.append(header, body);

    // 拖拽
    header.addEventListener("mousedown", (e) => startDragCard(node, card, e));
    card.addEventListener("click", () => {
      selectedTable = node.table.name;
      highlightSelection();
      updateInspector();
    });
    return card;
  }

  function highlightSelection(): void {
    for (const card of cardsLayer.children) {
      card.classList.toggle("selected", (card as HTMLElement).dataset.table === selectedTable);
    }
    for (const path of edgeSvg.querySelectorAll("path.er-edge")) {
      const hit = !selectedTable
        ? false
        : (path as SVGPathElement).dataset.source === selectedTable || (path as SVGPathElement).dataset.target === selectedTable;
      path.classList.toggle("highlight", hit);
    }
  }

  // ---------------------------------------------------------------- 右侧表设计器

  /** 每张表一个稳定的短 owner 标识（<=128 且不含 NUL，满足后端 validate_identity）。 */
  const ownerTokens = new Map<string, string>();
  function ownerIdFor(table: string): string {
    let token = ownerTokens.get(table);
    if (!token) { token = `er-${instanceId}-${ownerTokens.size + 1}`; ownerTokens.set(table, token); }
    return token;
  }

  function makeHost(): ErTableEditorHost {
    return {
      connection: () => connections.find((c) => c.id === connectionId),
      writeReason: () => {
        const cfg = connections.find((c) => c.id === connectionId);
        if (!cfg) return "连接不存在";
        if (cfg.read_only) return "此连接只读，不能修改表结构";
        return null;
      },
      changed: () => { renderStats(); renderPending(); },
      toast: (kind, text) => { inspMessage.textContent = text; inspMessage.dataset.tone = kind; },
    };
  }

  function pendingCount(): number {
    let n = 0;
    for (const d of designers.values()) { try { if (d.draftDirty()) n++; } catch { /* 读取失败的草稿不计数 */ } }
    return n;
  }

  function renderPending(): void {
    const badge = syncBtn.querySelector<HTMLElement>(".er-badge");
    const n = pendingCount();
    if (badge) { badge.hidden = n === 0; badge.textContent = String(n); }
    syncBtn.classList.toggle("has-pending", n > 0);
  }

  function resetDesigners(): void {
    for (const d of designers.values()) d.destroy();
    designers.clear();
    ownerTokens.clear();
    inspBody.replaceChildren();
  }

  function updateInspector(): void {
    if (!selectedTable) {
      inspTitle.textContent = "表设计";
      const empty = document.createElement("p");
      empty.className = "er-insp-empty";
      empty.textContent = "在左侧选择一张表，编辑字段、索引与外键，然后「同步到数据库」把改动应用到真实库。";
      inspBody.replaceChildren(empty);
      return;
    }
    if (!database) {
      inspTitle.textContent = `表 · ${selectedTable}`;
      const msg = document.createElement("p");
      msg.className = "er-insp-empty";
      msg.textContent = "请先在工具栏选择数据库 / 模式，才能读取并编辑表结构。";
      inspBody.replaceChildren(msg);
      return;
    }
    inspTitle.textContent = `表 · ${selectedTable}`;
    let designer = designers.get(selectedTable);
    if (!designer) {
      const holder = document.createElement("div");
      designer = new ErTableEditor({ connectionId, schema: database, table: selectedTable, ownerId: ownerIdFor(selectedTable) }, makeHost(), holder);
      designers.set(selectedTable, designer);
      designer.activate();
    }
    if (designer.el.parentElement !== inspBody) inspBody.replaceChildren(designer.el);
    renderPending();
  }

  function quoteIdent(kind: string, ident: string): string {
    const isTick = kind === "mysql" || kind === "clickhouse";
    return isTick ? "`" + ident.replace(/`/g, "``") + "`" : '"' + ident.replace(/"/g, '""') + '"';
  }

  function createTableDdl(kind: string, schema: string, name: string): string {
    const full = schema ? `${quoteIdent(kind, schema)}.${quoteIdent(kind, name)}` : quoteIdent(kind, name);
    if (kind === "sqlite") return `CREATE TABLE ${full} (id INTEGER PRIMARY KEY AUTOINCREMENT)`;
    if (kind === "postgresql") return `CREATE TABLE ${full} (id BIGSERIAL PRIMARY KEY)`;
    if (kind === "clickhouse") return `CREATE TABLE ${full} (id UInt64) ENGINE = MergeTree ORDER BY id`;
    if (kind === "redis" || kind === "mongodb") throw new Error("该数据源类型不支持关系表");
    return `CREATE TABLE ${full} (id BIGINT PRIMARY KEY AUTO_INCREMENT)`;
  }

  async function reloadMetadata(): Promise<void> {
    await loadTables();
    rebuildLayout(false);
    renderAll();
  }

  function cmdNewTable(): void {
    const cfg = connections.find((c) => c.id === connectionId);
    if (!cfg) { inspMessage.textContent = "请先选择连接与数据库"; return; }
    const nameInput = document.createElement("input");
    nameInput.type = "text";
    nameInput.style.width = "220px";
    new Dialog({
      title: "新建表",
      width: 380,
      render: (body) => {
        const row = document.createElement("div");
        row.className = "field";
        const lab = document.createElement("div");
        lab.className = "field-label";
        lab.textContent = "表名:";
        row.append(lab, nameInput);
        body.appendChild(row);
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "确定",
          primary: true,
          onClick: () => {
            const name = nameInput.value.trim();
            if (!name) return false;
            void (async () => {
              try {
                const ddl = createTableDdl(cfg.kind, database, name);
                await api.executeDdl(connectionId, ddl, database || undefined);
                selectedTable = name;
                inspMessage.textContent = `表「${name}」已创建`;
                inspMessage.dataset.tone = "success";
                await reloadMetadata();
              } catch (e) {
                inspMessage.textContent = String(e);
                inspMessage.dataset.tone = "error";
              }
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  function cmdDropTable(): void {
    if (!selectedTable) { inspMessage.textContent = "请先选择表"; return; }
    const table = selectedTable;
    const cfg = connections.find((c) => c.id === connectionId);
    const kind = cfg?.kind ?? "";
    new Dialog({
      title: "删除表",
      width: 380,
      render: (body) => {
        body.textContent = `确定要删除表「${table}」吗？该操作不可恢复。`;
        body.style.padding = "18px 14px";
      },
      buttons: [
        {
          label: "删除",
          primary: true,
          onClick: () => {
            void (async () => {
              try {
                const full = database ? `${quoteIdent(kind, database)}.${quoteIdent(kind, table)}` : quoteIdent(kind, table);
                await api.executeDdl(connectionId, `DROP TABLE IF EXISTS ${full}`, database || undefined);
                const d = designers.get(table);
                if (d) { d.destroy(); designers.delete(table); }
                selectedTable = null;
                inspMessage.textContent = `表「${table}」已删除`;
                inspMessage.dataset.tone = "success";
                await reloadMetadata();
              } catch (e) {
                inspMessage.textContent = String(e);
                inspMessage.dataset.tone = "error";
              }
            })();
          },
        },
        { label: "取消" },
      ],
    });
  }

  async function syncCurrent(): Promise<void> {
    if (!selectedTable) { inspMessage.textContent = "请先选择要同步的表"; return; }
    const designer = designers.get(selectedTable);
    if (!designer) { inspMessage.textContent = "表结构尚未载入，请稍候"; return; }
    await designer.save();
    renderPending();
    try {
      await loadTables();
      rebuildLayout(false);
      renderAll();
    } catch (e) {
      inspMessage.textContent = String(e);
      inspMessage.dataset.tone = "error";
    }
  }

  // ---------------------------------------------------------------- 工程 ER 图

  let engDiagram: ReturnType<typeof computeEngineeringLayout> | null = null;

  function renderEngineeringView(): void {
    cardsLayer.replaceChildren();
    edgeSvg.replaceChildren();
    const filtered = filterTables(tables, search.value);
    const visibleSet = new Set(filtered.map((t) => t.name));
    const rels = relationships.filter((r) => visibleSet.has(r.sourceTable) && visibleSet.has(r.targetTable));
    engDiagram = computeEngineeringLayout(filtered, rels);
    const d = engDiagram;
    edgeSvg.setAttribute("width", String(d.width));
    edgeSvg.setAttribute("height", String(d.height));
    edgeSvg.setAttribute("viewBox", `0 0 ${d.width} ${d.height}`);
    const ns = "http://www.w3.org/2000/svg";
    for (const link of d.links) {
      const line = document.createElementNS(ns, "line");
      line.setAttribute("x1", String(link.x1));
      line.setAttribute("y1", String(link.y1));
      line.setAttribute("x2", String(link.x2));
      line.setAttribute("y2", String(link.y2));
      line.classList.add(link.kind === "attribute" ? "er-eng-attr-line" : "er-eng-rel-line");
      edgeSvg.appendChild(line);
    }
    for (const attr of d.attributes) {
      const g = document.createElementNS(ns, "g");
      const ellipse = document.createElementNS(ns, "ellipse");
      ellipse.setAttribute("cx", String(attr.x + attr.width / 2));
      ellipse.setAttribute("cy", String(attr.y + attr.height / 2));
      ellipse.setAttribute("rx", String(attr.width / 2));
      ellipse.setAttribute("ry", String(attr.height / 2));
      ellipse.classList.add("er-eng-attr");
      if (attr.isPk) ellipse.classList.add("pk");
      const text = document.createElementNS(ns, "text");
      text.setAttribute("x", String(attr.x + attr.width / 2));
      text.setAttribute("y", String(attr.y + attr.height / 2 + 4));
      text.setAttribute("text-anchor", "middle");
      text.textContent = attr.column;
      g.append(ellipse, text);
      edgeSvg.appendChild(g);
    }
    for (const entity of d.entities) {
      const rect = document.createElementNS(ns, "rect");
      rect.setAttribute("x", String(entity.x));
      rect.setAttribute("y", String(entity.y));
      rect.setAttribute("width", String(entity.width));
      rect.setAttribute("height", String(entity.height));
      rect.classList.add("er-eng-entity");
      edgeSvg.appendChild(rect);
      const text = document.createElementNS(ns, "text");
      text.setAttribute("x", String(entity.x + entity.width / 2));
      text.setAttribute("y", String(entity.y + entity.height / 2 + 5));
      text.setAttribute("text-anchor", "middle");
      text.classList.add("er-eng-entity-text");
      text.textContent = entity.table.name;
      edgeSvg.appendChild(text);
    }
    for (const diamond of d.diamonds) {
      const polygon = document.createElementNS(ns, "polygon");
      const s = 26;
      polygon.setAttribute(
        "points",
        `${diamond.x},${diamond.y - s / 2} ${diamond.x + s / 2},${diamond.y} ${diamond.x},${diamond.y + s / 2} ${diamond.x - s / 2},${diamond.y}`,
      );
      polygon.classList.add("er-eng-diamond");
      edgeSvg.appendChild(polygon);
      const text = document.createElementNS(ns, "text");
      text.setAttribute("x", String(diamond.x));
      text.setAttribute("y", String(diamond.y - s / 2 - 6));
      text.setAttribute("text-anchor", "middle");
      text.classList.add("er-eng-diamond-text");
      text.textContent = diamond.label;
      edgeSvg.appendChild(text);
    }
  }

  // 建模关系面板已移除：关系基数直接显示在连线上，字段/索引/外键编辑改到右侧表设计器。

  // ---------------------------------------------------------------- 交互：平移 / 缩放 / 拖拽

  let panning = false;
  let panStart = { x: 0, y: 0, vx: 0, vy: 0 };
  canvas.addEventListener("mousedown", (e) => {
    if ((e.target as HTMLElement).closest(".er-card")) return;
    panning = true;
    panStart = { x: e.clientX, y: e.clientY, vx: viewport.x, vy: viewport.y };
    canvas.classList.add("panning");
  });
  window.addEventListener("mousemove", (e) => {
    if (panning) {
      viewport.x = panStart.vx + (e.clientX - panStart.x);
      viewport.y = panStart.vy + (e.clientY - panStart.y);
      applyViewport();
      renderStats();
      return;
    }
    if (dragCard) {
      const dx = (e.clientX - dragCard.startX) / viewport.zoom;
      const dy = (e.clientY - dragCard.startY) / viewport.zoom;
      dragCard.node.x = Math.round(dragCard.originX + dx);
      dragCard.node.y = Math.round(dragCard.originY + dy);
      dragCard.el.style.left = `${dragCard.node.x}px`;
      dragCard.el.style.top = `${dragCard.node.y}px`;
      // 实时重画边（轻量：仅本卡片相关）
      renderTablesViewEdges();
    }
  });
  window.addEventListener("mouseup", () => {
    if (panning) {
      panning = false;
      canvas.classList.remove("panning");
    }
    if (dragCard) {
      const positions = loadPositions(connectionId, database);
      positions[dragCard.node.table.name] = { x: dragCard.node.x, y: dragCard.node.y };
      savePositions(connectionId, database, positions);
      dragCard = null;
    }
  });
  canvas.addEventListener("wheel", (e) => {
    e.preventDefault();
    const rect = canvas.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const factor = e.deltaY < 0 ? 1.12 : 0.9;
    const zoom = Math.max(0.15, Math.min(2.5, viewport.zoom * factor));
    // 以光标为锚点缩放
    viewport.x = mx - ((mx - viewport.x) / viewport.zoom) * zoom;
    viewport.y = my - ((my - viewport.y) / viewport.zoom) * zoom;
    viewport.zoom = zoom;
    applyViewport();
    renderStats();
  }, { passive: false });

  let dragCard: { node: ReturnType<typeof computeLayout>[number]; el: HTMLElement; startX: number; startY: number; originX: number; originY: number } | null = null;
  function startDragCard(node: ReturnType<typeof computeLayout>[number], el: HTMLElement, e: MouseEvent): void {
    e.preventDefault();
    e.stopPropagation();
    dragCard = { node, el, startX: e.clientX, startY: e.clientY, originX: node.x, originY: node.y };
  }

  /** 仅重画边（拖拽时保持轻量）。 */
  function renderTablesViewEdges(): void {
    if (mode !== "tables") return;
    const byName = new Map(nodes.map((n) => [n.table.name, n]));
    for (const path of edgeSvg.querySelectorAll("path.er-edge")) {
      const s = byName.get((path as SVGPathElement).dataset.source ?? "");
      const t = byName.get((path as SVGPathElement).dataset.target ?? "");
      if (!s || !t) continue;
      path.setAttribute("d", orthPath(edgeEndpoints(s, t)));
    }
  }

  // ---------------------------------------------------------------- 工具栏事件

  connSelect.addEventListener("change", () => {
    connectionId = connSelect.value;
    resetDesigners();
    void refresh();
  });
  dbSelect.addEventListener("change", () => {
    database = dbSelect.value;
    resetDesigners();
    void refresh();
  });
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  search.addEventListener("input", () => {
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      rebuildLayout(false);
      renderAll();
    }, 180);
  });
  modeTables.addEventListener("click", () => {
    mode = "tables";
    renderAll();
  });
  modeEng.addEventListener("click", () => {
    mode = "engineering";
    renderAll();
    void Promise.resolve().then(fitView);
  });
  layoutBtn.addEventListener("click", () => {
    rebuildLayout(true);
    renderAll();
    void Promise.resolve().then(fitView);
  });
  fitBtn.addEventListener("click", () => fitView());
  zoomInBtn.addEventListener("click", () => {
    viewport.zoom = Math.min(2.5, viewport.zoom * 1.2);
    applyViewport();
    renderStats();
  });
  zoomOutBtn.addEventListener("click", () => {
    viewport.zoom = Math.max(0.15, viewport.zoom / 1.2);
    applyViewport();
    renderStats();
  });
  refreshBtn.addEventListener("click", () => void refresh());
  copySqlBtn.addEventListener("click", () => {
    const sql = buildJoinSql(tables.filter((t) => visibleNames.has(t.name)), relationships.filter((r) => visibleNames.has(r.sourceTable) && visibleNames.has(r.targetTable)));
    void navigator.clipboard
      ?.writeText(sql)
      .then(() => {
        stats.textContent = "JOIN SQL 已复制到剪贴板";
        setTimeout(() => renderStats(), 1800);
      })
      .catch(() => {
        stats.textContent = "复制失败，请手动选择";
      });
  });

  // orthPath 本地别名（独立于 erDiagram 的正交路由）
  function orthPath(e: ReturnType<typeof edgeEndpoints>): string {
    const { x1, y1, x2, y2, side1 } = e;
    const dir = side1 === "left" ? -1 : side1 === "right" ? 1 : 0;
    const stub = 24;
    if (dir !== 0) {
      const mx1 = x1 + dir * stub;
      const mx2 = x2 + (x2 > x1 ? -stub : stub);
      if (Math.abs(y1 - y2) < 1) return `M ${x1} ${y1} L ${x2} ${y2}`;
      if ((dir === 1 && mx1 <= mx2) || (dir === -1 && mx1 >= mx2)) {
        return `M ${x1} ${y1} L ${mx1} ${y1} L ${mx1} ${y2} L ${mx2} ${y2} L ${x2} ${y2}`;
      }
      const midX = (x1 + x2) / 2;
      return `M ${x1} ${y1} L ${midX} ${y1} L ${midX} ${y2} L ${x2} ${y2}`;
    }
    const midY = (y1 + y2) / 2;
    return `M ${x1} ${y1} L ${x1} ${midY} L ${x2} ${midY} L ${x2} ${y2}`;
  }

  // ---------------------------------------------------------------- 组装

  dlg.onClose(() => {
    for (const d of designers.values()) d.destroy();
    designers.clear();
  });
  accessibleDialog(dlg, "关系图");
  dlg.el.style.width = "min(1560px, 94vw)";
  dlg.el.style.height = "min(900px, 90vh)";
  dlg.el.style.maxWidth = "94vw";
  dlg.el.style.maxHeight = "90vh";
  // 打开即加载
  void refresh(currentSchema);
  renderAll();
  return dlg;
}
