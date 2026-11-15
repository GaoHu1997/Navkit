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
import { loadDiagramMetadataCached, invalidateDiagramMetadata } from "../core/erDiagramMetadata";
import { diagramViewport, intersectsDiagramViewport, edgeBounds } from "./erDiagramRendering";
import { Dialog } from "../ui/Dialog";
import { iconSvgByName } from "../ui/icons";
import { ErTableEditor, type ErTableEditorHost } from "../ui/ErTableEditor";
import { accessibleDialog } from "./SyncDialog";
import {
  CARD_BODY_MAX_H,
  CARD_ROW_H,
  buildRelationships,
  buildJoinSql,
  computeEngineeringLayout,
  computeLayout,
  edgeEndpoints,
  filterTables,
  loadPositions,
  savePositions,
  type Cardinality,
  type ErRelationship,
  type ErTable,
} from "../core/erDiagram";
import {
  describeStrategy,
  filterByStorage,
  inferRelationships,
  mergeRelationships,
  type InferredRelationship,
  type MatchResult,
} from "../core/erDiagramMatch";
import {
  isAutoMatchEnabled,
  loadMatchConfirms,
  loadMatchIgnores,
  saveMatchConfirms,
  saveMatchIgnores,
  setAutoMatchEnabled,
  toggleMatchId,
} from "../core/erDiagramMatchStorage";
import {
  computeElkLayout,
  computeGridLayout,
  ELK_EDGE_LIMIT,
  ELK_NODE_LIMIT,
  type LayoutDirection,
} from "../core/erDiagramLayout";
import {
  inferSide,
  inferTargetSide,
  pointsToSvgPath,
  routeEdge,
  type ObstacleRect,
  type Point as RoutePoint,
} from "../core/erDiagramRouting";
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
  /** 物理外键 + 自定义关系（不含推断）。 */
  let physicalRelationships: ErRelationship[] = [];
  /** 图上实际展示的关系（物理 + 用户启用的推断）。 */
  let relationships: ErRelationship[] = [];
  /** 最近一次推断的全部结果（未过滤）。 */
  let inferredAll: InferredRelationship[] = [];
  /** 过滤后的匹配结果（上图 / 冲突 / 统计）。 */
  let matchResult: MatchResult = { relationships: [], conflicts: [], pending: [], stats: { total: 0, high: 0, medium: 0 } };
  let matchConfirms: string[] = [];
  let matchIgnores: string[] = [];
  let autoMatch = isAutoMatchEnabled();
  let showMatchPanel = false;
  let visibleNames = new Set<string>();
  let nodes: ReturnType<typeof computeLayout> = [];
  /** ELK 布局算出的关系折线（关系 id → 点集），优先于内置三段折线。 */
  let edgePaths = new Map<string, RoutePoint[]>();
  let layoutDirection: LayoutDirection = "LR";
  let layoutRunning = false;
  let mode: "tables" | "engineering" = "tables";
  let viewport: Viewport = { x: 0, y: 0, zoom: 1 };
  let selectedTable: string | null = null;
  let loadGeneration = 0;
  let disposed = false;
  let loadedScope = "";
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
  layoutBtn.title = "用 ELK 分层算法重新排布（最小化连线交叉）";

  // 布局方向：LR / TB / RL / BT
  const dirSelect = document.createElement("select");
  dirSelect.className = "er-select er-dir-select";
  dirSelect.setAttribute("aria-label", "布局方向");
  for (const [value, label] of [["LR", "左 → 右"], ["TB", "上 → 下"], ["RL", "右 → 左"], ["BT", "下 → 上"]] as const) {
    const opt = document.createElement("option");
    opt.value = value;
    opt.textContent = label;
    dirSelect.appendChild(opt);
  }
  dirSelect.value = layoutDirection;

  // 自动匹配：切换推断面板
  const matchBtn = document.createElement("button");
  matchBtn.type = "button";
  matchBtn.className = "er-tool-btn";
  const matchLabel = document.createElement("span");
  matchLabel.textContent = "自动匹配";
  const matchBadge = document.createElement("span");
  matchBadge.className = "er-badge";
  matchBadge.hidden = true;
  matchBtn.append(matchLabel, matchBadge);
  matchBtn.title = "按命名约定与类型签名推断虚拟外键（不写库）";
  matchBtn.addEventListener("click", () => {
    showMatchPanel = !showMatchPanel;
    renderMatchPanel();
  });

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

  toolbar.append(connSelect, dbSelect, search, modeGroup, newTableBtn, layoutBtn, dirSelect, matchBtn, fitBtn, copySqlBtn, syncBtn, refreshBtn, stats, zoomOutBtn, zoomInBtn);
  dlg.body.appendChild(toolbar);

  // 自动匹配面板（复用现有面板的视觉与折叠模式，位于工具栏与画布之间）
  const matchPanel = document.createElement("div");
  matchPanel.className = "er-match-panel";
  matchPanel.hidden = true;
  dlg.body.appendChild(matchPanel);

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

  type Scope = { connection: ConnectionConfig; namespace: string };
  const scopeKey = (scope: Scope): string => JSON.stringify([scope.connection.id, scope.namespace]);
  function currentScope(): Scope | null {
    const connection = connections.find((c) => c.id === connectionId);
    return connection ? { connection: { ...connection }, namespace: database } : null;
  }
  function scopeIsCurrent(scope: Scope): boolean {
    return !disposed && scope.connection.id === connectionId && scope.namespace === database;
  }

  /** DBX 的 generation/context guard：目录和元数据都只提交最新请求。
   * 不用 loading 锁丢弃切换请求，也不让 worker 读取可变的连接/namespace。 */
  async function refresh(preferDb?: string, force = false): Promise<void> {
    const generation = ++loadGeneration;
    const initial = currentScope();
    if (!initial || disposed) return;
    const active = (): boolean => !disposed && generation === loadGeneration && initial.connection.id === connectionId;
    showOverlay("正在加载表结构...");
    try {
      if (preferDb !== undefined || dbSelect.options.length === 0) {
        const dbs = await api.listDatabases(initial.connection.id);
        if (!active()) return;
        const preferred = preferDb ?? initial.namespace;
        const namespace = dbs.some(([name]) => name === preferred) ? preferred : dbs[0]?.[0] ?? "";
        const options = dbs.map(([name]) => {
          const opt = document.createElement("option");
          opt.value = name;
          opt.textContent = name;
          return opt;
        });
        dbSelect.replaceChildren(...options);
        database = namespace;
        dbSelect.value = namespace;
      }
      const scope: Scope = { connection: initial.connection, namespace: database };
      if (!scope.namespace && scope.connection.kind !== "sqlite") throw new Error("请先选择连接与数据库");
      const metadata = await loadDiagramMetadataCached(scope.connection, scope.namespace,
        () => api.getDiagramMetadata(scope.connection.id, scope.namespace), force);
      if (!active() || !scopeIsCurrent(scope)) return;
      // 先完整映射局部结果再提交，失败不会把已有成功图变成空数组。
      const next: ErTable[] = metadata.tables.map((schema) => ({
        name: schema.table,
        schema: schema.database || scope.namespace,
        comment: schema.comment ?? "",
        columns: schema.columns.map((c) => ({ name: c.name, type: c.type, primary_key: c.primary_key,
          auto_increment: c.auto_increment, nullable: c.nullable, comment: c.comment, length: c.length })),
        indexes: schema.indexes.map((index) => ({ columns: index.columns, unique: index.unique, primary: index.primary })),
        foreignKeys: schema.foreign_keys.map((fk) => ({ name: fk.name, columns: fk.columns,
          ref_table: fk.ref_table, ref_schema: fk.ref_schema, ref_columns: fk.ref_columns })),
      })).sort((a, b) => a.name.localeCompare(b.name));
      tables = next;
      physicalRelationships = buildRelationships(tables);
      matchConfirms = loadMatchConfirms(scope.connection.id, scope.namespace);
      matchIgnores = loadMatchIgnores(scope.connection.id, scope.namespace);
      recomputeRelationships();
      loadedScope = scopeKey(scope);
      if (!selectedTable || !tables.some((t) => t.name === selectedTable)) selectedTable = tables[0]?.name ?? null;
      rebuildLayout(false);
      hideOverlay();
      const notices = [...metadata.failures.map((f) => `${f.table}：${f.error}`), ...metadata.limitations];
      inspMessage.textContent = notices.join("；");
      inspMessage.dataset.tone = notices.length ? "error" : "";
      renderAll();
    } catch (e) {
      if (!active()) return;
      // 同一范围刷新失败仍可操作旧图；首次或换库失败使用原有错误覆盖层。
      const scope = currentScope();
      const message = `加载失败：${String(e)}（点击「刷新」重试）`;
      if (scope && loadedScope === scopeKey(scope) && tables.length) {
        hideOverlay();
        inspMessage.textContent = message;
        inspMessage.dataset.tone = "error";
      } else showOverlay(message, true);
    }
  }

  // ---------------------------------------------------------------- 关系与自动匹配

  /**
   * 重算图上关系：物理外键优先，叠加用户启用的推断关系（虚拟外键）。
   * 推断只在「自动匹配开启」且不是工程 ER 图时参与上图。
   */
  function recomputeRelationships(): void {
    inferredAll = tables.length ? inferRelationships(tables, physicalRelationships) : [];
    matchResult = filterByStorage(inferredAll, matchConfirms, matchIgnores);
    if (autoMatch && matchResult.relationships.length) {
      relationships = mergeRelationships(physicalRelationships, matchResult.relationships);
    } else {
      relationships = [...physicalRelationships];
    }
  }

  /** 推断关系数（用于工具栏徽标与统计）。 */
  function inferredCount(): number {
    return relationships.filter((rel) => rel.kind === "inferred").length;
  }

  function renderMatchPanel(): void {
    matchPanel.hidden = !showMatchPanel;
    matchBtn.classList.toggle("active", showMatchPanel);
    const badgeCount = autoMatch ? matchResult.relationships.length + matchResult.conflicts.length : 0;
    matchBadge.hidden = badgeCount === 0;
    matchBadge.textContent = String(badgeCount);
    if (!showMatchPanel) return;

    const head = document.createElement("div");
    head.className = "er-match-head";
    const title = document.createElement("div");
    title.className = "er-match-title";
    title.textContent = "自动匹配（虚拟外键）";
    const summary = document.createElement("div");
    summary.className = "er-match-summary";
    summary.textContent = `${matchResult.stats.total} 条推断 · 高置信 ${matchResult.stats.high} · 中置信 ${matchResult.stats.medium} · 冲突 ${matchResult.conflicts.length}`;

    const toggleWrap = document.createElement("label");
    toggleWrap.className = "er-match-toggle";
    const toggle = document.createElement("input");
    toggle.type = "checkbox";
    toggle.checked = autoMatch;
    toggle.addEventListener("change", () => {
      autoMatch = toggle.checked;
      setAutoMatchEnabled(autoMatch);
      recomputeRelationships();
      rebuildLayout(false);
      renderAll();
    });
    const toggleText = document.createElement("span");
    toggleText.textContent = "在图上显示";
    toggleWrap.append(toggle, toggleText);
    head.append(title, summary, toggleWrap);

    const body = document.createElement("div");
    body.className = "er-match-body";

    if (!inferredAll.length) {
      const empty = document.createElement("p");
      empty.className = "er-match-empty";
      empty.textContent = tables.length
        ? "没有可推断的关系：需要目标表存在主键，且列名形如 user_id / userId / order_uuid。"
        : "尚未加载表结构。";
      body.appendChild(empty);
    } else {
      for (const rel of inferredAll) {
        body.appendChild(renderMatchRow(rel));
      }
    }

    matchPanel.replaceChildren(head, body);
  }

  function renderMatchRow(rel: InferredRelationship): HTMLElement {
    const row = document.createElement("div");
    row.className = "er-match-row";
    if (matchResult.conflicts.some((item) => item.id === rel.id)) row.classList.add("conflict");
    if (matchConfirms.includes(rel.id)) row.classList.add("confirmed");
    if (matchIgnores.includes(rel.id)) row.classList.add("ignored");

    const label = document.createElement("div");
    label.className = "er-match-label";
    const pair = document.createElement("div");
    pair.className = "er-match-pair";
    pair.textContent = `${rel.sourceTable}.${rel.sourceColumn} → ${rel.targetTable}.${rel.targetColumn}`;
    const meta = document.createElement("div");
    meta.className = "er-match-meta";
    const confidence = rel.confidence === "high" ? "高置信" : "中置信";
    const conflictHint = matchResult.conflicts.some((item) => item.id === rel.id) ? " · 多目标冲突，需人工选择" : "";
    meta.textContent = `${confidence} · ${describeStrategy(rel)}${conflictHint}`;
    label.append(pair, meta);

    const actions = document.createElement("div");
    actions.className = "er-match-actions";
    const confirmBtn = document.createElement("button");
    confirmBtn.type = "button";
    confirmBtn.className = "er-match-btn";
    confirmBtn.textContent = matchConfirms.includes(rel.id) ? "已确认" : "确认";
    confirmBtn.disabled = matchConfirms.includes(rel.id);
    confirmBtn.addEventListener("click", () => {
      matchConfirms = toggleMatchId(matchConfirms, rel.id, true);
      matchIgnores = toggleMatchId(matchIgnores, rel.id, false);
      saveMatchConfirms(matchConfirms, connectionId, database);
      saveMatchIgnores(matchIgnores, connectionId, database);
      recomputeRelationships();
      rebuildLayout(false);
      renderAll();
    });
    const ignoreBtn = document.createElement("button");
    ignoreBtn.type = "button";
    ignoreBtn.className = "er-match-btn ghost";
    ignoreBtn.textContent = matchIgnores.includes(rel.id) ? "已忽略" : "忽略";
    ignoreBtn.disabled = matchIgnores.includes(rel.id);
    ignoreBtn.addEventListener("click", () => {
      matchIgnores = toggleMatchId(matchIgnores, rel.id, true);
      matchConfirms = toggleMatchId(matchConfirms, rel.id, false);
      saveMatchIgnores(matchIgnores, connectionId, database);
      saveMatchConfirms(matchConfirms, connectionId, database);
      recomputeRelationships();
      rebuildLayout(false);
      renderAll();
    });
    actions.append(confirmBtn, ignoreBtn);
    row.append(label, actions);
    return row;
  }

  // ---------------------------------------------------------------- 布局与渲染

  function rebuildLayout(resetPositions: boolean): void {
    const filtered = filterTables(tables, search.value);
    visibleNames = new Set(filtered.map((t) => t.name));
    nodes = computeLayout(filtered, relationships);
    nodeByName = new Map(nodes.map((node) => [node.table.name, node]));
    incidentEdges.clear();
    edgesById.clear();
    for (const rel of relationships) {
      if (!nodeByName.has(rel.sourceTable) || !nodeByName.has(rel.targetTable) || edgesById.has(rel.id)) continue;
      edgesById.set(rel.id, rel);
      for (const name of new Set([rel.sourceTable, rel.targetTable])) {
        const ids = incidentEdges.get(name) ?? new Set<string>();
        ids.add(rel.id);
        incidentEdges.set(name, ids);
      }
    }
    engDiagram = null;
    engineeringItems = [];
    dirtyEdges = new Set(edgesById.keys());
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
    // 内置分层布局给出初始坐标；ELK 结果（若可用）在 applyElkLayout 里覆盖。
    edgePaths.clear();
    applyViewport();
  }

  // ---------------------------------------------------------------- ELK 自动布局

  /** 当前可见表卡片的障碍矩形，供边路由绕行。 */
  function obstacleRects(): ObstacleRect[] {
    return nodes.map((node) => ({ id: node.table.name, x: node.x, y: node.y, width: node.width, height: node.height }));
  }

  /**
   * 用 ELK 分层算法重排当前可见表，并把 ELK 算出的正交拐点用作关系折线。
   * 大图跳过 ELK（Sugiyama 在数百节点上会明显变慢），退回网格布局。
   */
  async function applyElkLayout(): Promise<void> {
    if (disposed || layoutRunning || mode !== "tables") return;
    const filtered = filterTables(tables, search.value);
    if (!filtered.length) return;
    layoutRunning = true;
    layoutBtn.classList.add("busy");
    layoutBtn.textContent = "布局中…";
    const generation = loadGeneration;
    try {
      const visibleSet = new Set(filtered.map((t) => t.name));
      const edges = relationships
        .filter((rel) => visibleSet.has(rel.sourceTable) && visibleSet.has(rel.targetTable))
        .map((rel) => ({ id: rel.id, source: rel.sourceTable, target: rel.targetTable }));

      if (filtered.length > ELK_NODE_LIMIT || edges.length > ELK_EDGE_LIMIT) {
        const positions = computeGridLayout(
          filtered.map((table) => ({ table, width: 260, height: cardHeightOf(table) })),
          relationships.filter((rel) => visibleSet.has(rel.sourceTable) && visibleSet.has(rel.targetTable)),
          canvasSize.width || 1200,
        );
        for (const node of nodes) {
          const pos = positions[node.table.name];
          if (pos) { node.x = pos.x; node.y = pos.y; }
        }
        edgePaths.clear();
      } else {
        const result = await computeElkLayout(
          filtered.map((table) => ({ table, width: 260, height: cardHeightOf(table) })),
          edges,
          { direction: layoutDirection },
        );
        // 布局期间用户可能切换了连接/库或关闭了对话框。
        if (disposed || generation !== loadGeneration) return;
        if (result.applied) {
          for (const node of nodes) {
            const pos = result.positions[node.table.name];
            if (pos) { node.x = pos.x; node.y = pos.y; }
          }
          edgePaths.clear();
          for (const [id, points] of Object.entries(result.edgeWaypoints)) {
            edgePaths.set(id, points);
          }
          // 布局结果需要持久化，否则刷新会跳回旧位置。
          persistPositions();
        }
      }
      dirtyEdges = new Set(edgesById.keys());
      fullRenderPending = true;
      applyViewport();
      void Promise.resolve().then(fitView);
    } finally {
      layoutRunning = false;
      layoutBtn.classList.remove("busy");
      layoutBtn.textContent = "自动布局";
    }
  }

  function cardHeightOf(table: ErTable): number {
    // 与渲染共用的卡片高度口径（含表头与裁剪后的列体）。
    return 34 + Math.min(table.columns.length * 22, 240) + 2;
  }

  function persistPositions(): void {
    const positions: Record<string, { x: number; y: number }> = {};
    for (const node of nodes) positions[node.table.name] = { x: node.x, y: node.y };
    savePositions(connectionId, database, positions);
  }

  /**
   * 关系折线：优先用 ELK 拐点，否则用避障路由绕开中间卡片。
   * 端点由节点矩形实时算出，保证拖拽时连线跟随。
   */
  function edgePolyline(rel: ErRelationship, source: DiagramNodeLike, target: DiagramNodeLike): RoutePoint[] {
    const endpoints = edgeEndpoints(source, target);
    const elk = edgePaths.get(rel.id);
    if (elk && elk.length >= 2) return elk;
    return routeEdge({
      source: { x: endpoints.x1, y: endpoints.y1 },
      target: { x: endpoints.x2, y: endpoints.y2 },
      sourceSide: inferSide({ x: endpoints.x1, y: endpoints.y1 }, { x: endpoints.x2, y: endpoints.y2 }),
      targetSide: inferTargetSide({ x: endpoints.x1, y: endpoints.y1 }, { x: endpoints.x2, y: endpoints.y2 }),
      obstacles: obstacleRects(),
      endpointIds: [rel.sourceTable, rel.targetTable],
    });
  }

  type DiagramNodeLike = ReturnType<typeof computeLayout>[number];

  // 平移、缩放、拖拽和 ResizeObserver 共用一个有终止条件的帧；不在 mousemove 重建 DOM。
  let renderFrame: number | null = null;
  let fullRenderPending = true;
  const dirtyCards = new Set<string>();
  let canvasSize = { width: 1, height: 1 };
  let nodeByName = new Map<string, ReturnType<typeof computeLayout>[number]>();
  const incidentEdges = new Map<string, Set<string>>();
  const edgesById = new Map<string, ErRelationship>();
  let dirtyEdges = new Set<string>();
  const mountedCards = new Map<string, { el: HTMLElement; table: ErTable }>();
  const mountedEdges = new Map<string, { path: SVGPathElement; source: SVGElement; target: SVGElement }>();
  const mountedEngineering = new Map<string, SVGElement>();
  let renderedMode: "tables" | "engineering" | null = null;
  let engineeringItems: { key: string; bounds: { left: number; top: number; right: number; bottom: number }; create: () => SVGElement }[] = [];

  function applyViewport(reconcile = true): void {
    if (disposed) return;
    fullRenderPending ||= reconcile;
    if (renderFrame !== null) return;
    renderFrame = window.requestAnimationFrame(flushRenderFrame);
  }
  function flushRenderFrame(): void {
    renderFrame = null;
    if (disposed) return;
    // 尺寸通过 observer 批量读取，下面全为写入/纯模型计算。
    viewportEl.style.transform = `translate(${viewport.x}px, ${viewport.y}px) scale(${viewport.zoom})`;
    if (renderedMode !== mode) {
      cardsLayer.replaceChildren();
      edgeSvg.replaceChildren();
      mountedCards.clear();
      mountedEdges.clear();
      mountedEngineering.clear();
      dirtyEdges = new Set(edgesById.keys());
      renderedMode = mode;
    }
    if (mode === "tables") {
      if (fullRenderPending) renderTablesView();
      else renderDirtyTableObjects();
    } else renderEngineeringView();
    if (fullRenderPending) renderStats();
    fullRenderPending = false;
    dirtyCards.clear();
  }
  function renderDirtyTableObjects(): void {
    let width = svgSize.width, height = svgSize.height;
    for (const name of dirtyCards) {
      const mounted = mountedCards.get(name);
      const node = nodeByName.get(name);
      if (mounted && node) {
        mounted.el.style.transform = `translate(${node.x}px, ${node.y}px)`;
        width = Math.max(width, node.x + node.width + 200);
        height = Math.max(height, node.y + node.height + 200);
      }
    }
    if (width !== svgSize.width || height !== svgSize.height) setSvgSize(width, height);
    const clip = nodes.length > 200;
    const bounds = diagramViewport(viewport, canvasSize.width, canvasSize.height);
    for (const id of dirtyEdges) {
      const rel = edgesById.get(id);
      if (!rel) continue;
      const source = nodeByName.get(rel.sourceTable);
      const target = nodeByName.get(rel.targetTable);
      if (!source || !target) continue;
      const endpoints = edgeEndpoints(source, target);
      let mounted = mountedEdges.get(id);
      if (clip && !intersectsDiagramViewport(edgeBounds(endpoints), bounds)) {
        if (mounted) { mounted.path.remove(); mounted.source.remove(); mounted.target.remove(); mountedEdges.delete(id); }
      } else {
        mounted ??= mountTableEdge(id, rel);
        const points = edgePolyline(rel, source, target);
        mounted.path.setAttribute("d", pointsToSvgPath(points));
        // 端点徽标落在折线首尾，绕行后也不会偏离卡片。
        const head = points[0] ?? { x: endpoints.x1, y: endpoints.y1 };
        const tail = points[points.length - 1] ?? { x: endpoints.x2, y: endpoints.y2 };
        moveCardinalityBadge(mounted.source, head.x, head.y, "source");
        moveCardinalityBadge(mounted.target, tail.x, tail.y, "target");
      }
      dirtyEdges.delete(id);
    }
  }
  const resizeObserver = new ResizeObserver((entries) => {
    const entry = entries[0];
    if (!entry || disposed) return;
    canvasSize = { width: Math.max(1, entry.contentRect.width), height: Math.max(1, entry.contentRect.height) };
    applyViewport();
  });
  resizeObserver.observe(canvas);

  function fitView(): void {
    if (disposed) return;
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
      minX = Infinity; minY = Infinity;
      for (const node of nodes) {
        minX = Math.min(minX, node.x); minY = Math.min(minY, node.y);
        maxX = Math.max(maxX, node.x + node.width); maxY = Math.max(maxY, node.y + node.height);
      }
    } else {
      const d = ensureEngineeringModel();
      minX = 0;
      minY = 0;
      maxX = d.width;
      maxY = d.height;
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
    const inferred = inferredCount();
    stats.textContent = `${visibleNames.size} 张表 · ${relCount} 条关系${inferred ? `（含 ${inferred} 条推断）` : ""} · ${Math.round(viewport.zoom * 100)}%${pending ? ` · ${pending} 项待同步` : ""}`;
  }

  /** 渲染全部（按模式）。 */
  function renderAll(): void {
    modeTables.classList.toggle("active", mode === "tables");
    modeEng.classList.toggle("active", mode === "engineering");
    renderStats();
    renderMatchPanel();
    applyViewport();
    updateInspector();
    renderPending();
  }

  function renderTablesView(): void {
    const clip = nodes.length > 200;
    const bounds = diagramViewport(viewport, canvasSize.width, canvasSize.height);
    const wantedCards = new Set<string>();
    for (const node of nodes) {
      const name = node.table.name;
      if (clip && dragCard?.node !== node && !intersectsDiagramViewport({ left: node.x, top: node.y,
        right: node.x + node.width, bottom: node.y + node.height }, bounds)) continue;
      wantedCards.add(name);
      let mounted = mountedCards.get(name);
      if (mounted && mounted.table !== node.table) {
        mounted.el.remove();
        mountedCards.delete(name);
        mounted = undefined;
      }
      if (!mounted) {
        mounted = { el: renderCard(node), table: node.table };
        mountedCards.set(name, mounted);
        cardsLayer.appendChild(mounted.el);
      }
      const transform = `translate(${node.x}px, ${node.y}px)`;
      if (mounted.el.style.transform !== transform) mounted.el.style.transform = transform;
      mounted.el.classList.toggle("selected", selectedTable === name);
    }
    for (const [name, mounted] of mountedCards) {
      if (!wantedCards.has(name)) { mounted.el.remove(); mountedCards.delete(name); }
    }
    // 连线按自身 bbox 裁剪，不能仅看两端节点是否挂载。
    const wantedEdges = new Set<string>();
    for (const [id, rel] of edgesById) {
      const source = nodeByName.get(rel.sourceTable);
      const target = nodeByName.get(rel.targetTable);
      if (!source || !target) continue;
      const endpoints = edgeEndpoints(source, target);
      if (clip && !intersectsDiagramViewport(edgeBounds(endpoints), bounds)) continue;
      wantedEdges.add(id);
      const mounted = mountedEdges.get(id) ?? mountTableEdge(id, rel);
      if (dirtyEdges.has(id)) {
        const points = edgePolyline(rel, source, target);
        mounted.path.setAttribute("d", pointsToSvgPath(points));
        // 路径与两端基数共用同一份坐标，拖拽时徽标不会留在原地。
        const head = points[0] ?? { x: endpoints.x1, y: endpoints.y1 };
        const tail = points[points.length - 1] ?? { x: endpoints.x2, y: endpoints.y2 };
        moveCardinalityBadge(mounted.source, head.x, head.y, "source");
        moveCardinalityBadge(mounted.target, tail.x, tail.y, "target");
        dirtyEdges.delete(id);
      }
      mounted.path.classList.toggle("highlight", rel.sourceTable === selectedTable || rel.targetTable === selectedTable);
    }
    for (const [id, mounted] of mountedEdges) {
      if (!wantedEdges.has(id)) {
        mounted.path.remove(); mounted.source.remove(); mounted.target.remove(); mountedEdges.delete(id);
      }
    }
    dirtyEdges.clear();
    let width = 600, height = 500;
    for (const node of nodes) { width = Math.max(width, node.x + node.width + 200); height = Math.max(height, node.y + node.height + 200); }
    setSvgSize(width, height);
  }

  function mountTableEdge(id: string, rel: ErRelationship): NonNullable<ReturnType<typeof mountedEdges.get>> {
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.classList.add("er-edge");
    // 虚拟外键用虚线，和物理外键区分。
    if (rel.kind === "inferred") path.classList.add("inferred");
    if (rel.kind === "custom") path.classList.add("custom");
    path.dataset.source = rel.sourceTable;
    path.dataset.target = rel.targetTable;
    path.dataset.kind = rel.kind;
    const mounted = { path, source: cardinalityBadge(0, 0, rel.sourceCardinality, "source"),
      target: cardinalityBadge(0, 0, rel.targetCardinality, "target") };
    path.classList.toggle("highlight", rel.sourceTable === selectedTable || rel.targetTable === selectedTable);
    mountedEdges.set(id, mounted);
    edgeSvg.append(path, mounted.source, mounted.target);
    dirtyEdges.add(id);
    return mounted;
  }

  let svgSize = { width: 600, height: 500 };
  function setSvgSize(width: number, height: number): void {
    svgSize = { width, height };
    for (const [key, value] of [["width", String(width)], ["height", String(height)], ["viewBox", `0 0 ${width} ${height}`]]) {
      if (edgeSvg.getAttribute(key) !== value) edgeSvg.setAttribute(key, value);
    }
  }

  function moveCardinalityBadge(badge: SVGElement, x: number, y: number, side: "source" | "target"): void {
    const text = badge.firstElementChild!;
    text.setAttribute("x", String(x + (side === "source" ? 8 : -14)));
    text.setAttribute("y", String(y - 6));
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
    card.style.left = "0";
    card.style.top = "0";
    card.style.transform = `translate(${node.x}px, ${node.y}px)`;
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
    // 当前 UI 正文固定裁到 240px：只构造可见的 11 行（最后一行仍由既有 CSS 裁剪）。
    const fkColumns = new Set(node.table.foreignKeys.flatMap((fk) => fk.columns));
    for (const col of node.table.columns.slice(0, Math.ceil(CARD_BODY_MAX_H / CARD_ROW_H))) {
      const row = document.createElement("div");
      row.className = "er-card-row";
      const keyIcon = document.createElement("span");
      keyIcon.className = "er-col-icon";
      if (col.primary_key) keyIcon.innerHTML = iconSvgByName("info_pane_pk");
      else if (fkColumns.has(col.name)) keyIcon.innerHTML = iconSvgByName("info_pane_fk");
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
    header.addEventListener("mousedown", (e) => {
      const currentNode = nodeByName.get(node.table.name);
      if (currentNode) startDragCard(currentNode, card, e);
    });
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

  function makeHost(scope: Scope): ErTableEditorHost {
    return {
      connection: () => scope.connection,
      writeReason: () => {
        if (!scopeIsCurrent(scope)) return "连接或数据库已切换，请重新选择表";
        if (scope.connection.read_only) return "此连接只读，不能修改表结构";
        return null;
      },
      changed: () => { if (scopeIsCurrent(scope)) { renderStats(); renderPending(); } },
      toast: (kind, text) => { if (scopeIsCurrent(scope)) { inspMessage.textContent = text; inspMessage.dataset.tone = kind; } },
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
      const scope = currentScope();
      if (!scope) return;
      designer = new ErTableEditor({ connectionId, schema: database, table: selectedTable, ownerId: ownerIdFor(selectedTable) }, makeHost(scope), holder);
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

  async function reloadMetadata(scope: Scope): Promise<void> {
    // DDL 真正改变了旧范围：关闭/切库之后也必须使该范围失效，绝不覆盖新图。
    invalidateDiagramMetadata(scope.connection, scope.namespace);
    if (scopeIsCurrent(scope)) await refresh(undefined, true);
  }

  function cmdNewTable(): void {
    const scope = currentScope();
    if (!scope) { inspMessage.textContent = "请先选择连接与数据库"; return; }
    const cfg = scope.connection;
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
                if (!scopeIsCurrent(scope)) return;
                const ddl = createTableDdl(cfg.kind, scope.namespace, name);
                await api.executeDdl(scope.connection.id, ddl, scope.namespace || undefined);
                if (scopeIsCurrent(scope)) {
                  selectedTable = name;
                  inspMessage.textContent = `表「${name}」已创建`;
                  inspMessage.dataset.tone = "success";
                }
                await reloadMetadata(scope);
              } catch (e) {
                if (scopeIsCurrent(scope)) { inspMessage.textContent = String(e); inspMessage.dataset.tone = "error"; }
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
    const scope = currentScope();
    if (!scope) return;
    const kind = scope.connection.kind;
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
                if (!scopeIsCurrent(scope)) return;
                const full = scope.namespace ? `${quoteIdent(kind, scope.namespace)}.${quoteIdent(kind, table)}` : quoteIdent(kind, table);
                await api.executeDdl(scope.connection.id, `DROP TABLE IF EXISTS ${full}`, scope.namespace || undefined);
                if (scopeIsCurrent(scope)) {
                  const d = designers.get(table);
                  if (d) { d.destroy(); designers.delete(table); }
                  selectedTable = null;
                  inspMessage.textContent = `表「${table}」已删除`;
                  inspMessage.dataset.tone = "success";
                }
                await reloadMetadata(scope);
              } catch (e) {
                if (scopeIsCurrent(scope)) { inspMessage.textContent = String(e); inspMessage.dataset.tone = "error"; }
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
    const scope = currentScope();
    if (!scope) return;
    await designer.save();
    if (scopeIsCurrent(scope)) renderPending();
    await reloadMetadata(scope);
  }

  // ---------------------------------------------------------------- 工程 ER 图

  let engDiagram: ReturnType<typeof computeEngineeringLayout> | null = null;

  function ensureEngineeringModel(): NonNullable<typeof engDiagram> {
    if (engDiagram) return engDiagram;
    const filtered = filterTables(tables, search.value);
    const visibleSet = new Set(filtered.map((t) => t.name));
    const rels = relationships.filter((r) => visibleSet.has(r.sourceTable) && visibleSet.has(r.targetTable));
    engDiagram = computeEngineeringLayout(filtered, rels);
    // 模型仅在数据、模式首次进入、搜索/自动布局时重算。切片只增删有变化的图元。
    for (const element of mountedEngineering.values()) element.remove();
    mountedEngineering.clear();
    engineeringItems = [];
    const ns = "http://www.w3.org/2000/svg";
    const d = engDiagram;
    d.links.forEach((link, i) => engineeringItems.push({ key: `link:${i}`, bounds: edgeBounds(link), create: () => {
      const line = document.createElementNS(ns, "line");
      line.setAttribute("x1", String(link.x1)); line.setAttribute("y1", String(link.y1));
      line.setAttribute("x2", String(link.x2)); line.setAttribute("y2", String(link.y2));
      line.classList.add(link.kind === "attribute" ? "er-eng-attr-line" : "er-eng-rel-line");
      return line;
    } }));
    d.attributes.forEach((attr, i) => engineeringItems.push({ key: `attr:${i}`,
      bounds: { left: attr.x, top: attr.y, right: attr.x + attr.width, bottom: attr.y + attr.height }, create: () => {
        const g = document.createElementNS(ns, "g");
        const ellipse = document.createElementNS(ns, "ellipse");
        ellipse.setAttribute("cx", String(attr.x + attr.width / 2)); ellipse.setAttribute("cy", String(attr.y + attr.height / 2));
        ellipse.setAttribute("rx", String(attr.width / 2)); ellipse.setAttribute("ry", String(attr.height / 2));
        ellipse.classList.add("er-eng-attr");
        if (attr.isPk) ellipse.classList.add("pk");
        const text = document.createElementNS(ns, "text");
        text.setAttribute("x", String(attr.x + attr.width / 2)); text.setAttribute("y", String(attr.y + attr.height / 2 + 4));
        text.setAttribute("text-anchor", "middle"); text.textContent = attr.column;
        g.append(ellipse, text);
        return g;
      } }));
    d.entities.forEach((entity, i) => engineeringItems.push({ key: `entity:${i}`,
      bounds: { left: entity.x, top: entity.y, right: entity.x + entity.width, bottom: entity.y + entity.height }, create: () => {
        const g = document.createElementNS(ns, "g");
        const rect = document.createElementNS(ns, "rect");
        rect.setAttribute("x", String(entity.x)); rect.setAttribute("y", String(entity.y));
        rect.setAttribute("width", String(entity.width)); rect.setAttribute("height", String(entity.height));
        rect.classList.add("er-eng-entity");
        const text = document.createElementNS(ns, "text");
        text.setAttribute("x", String(entity.x + entity.width / 2)); text.setAttribute("y", String(entity.y + entity.height / 2 + 5));
        text.setAttribute("text-anchor", "middle"); text.classList.add("er-eng-entity-text"); text.textContent = entity.table.name;
        g.append(rect, text);
        return g;
      } }));
    d.diamonds.forEach((diamond, i) => engineeringItems.push({ key: `diamond:${i}`,
      bounds: { left: diamond.x - Math.max(13, diamond.label.length * 4), top: diamond.y - 32,
        right: diamond.x + Math.max(13, diamond.label.length * 4), bottom: diamond.y + 13 }, create: () => {
        const g = document.createElementNS(ns, "g");
        const polygon = document.createElementNS(ns, "polygon");
        const s = 26;
        polygon.setAttribute("points", `${diamond.x},${diamond.y - s / 2} ${diamond.x + s / 2},${diamond.y} ${diamond.x},${diamond.y + s / 2} ${diamond.x - s / 2},${diamond.y}`);
        polygon.classList.add("er-eng-diamond");
        const text = document.createElementNS(ns, "text");
        text.setAttribute("x", String(diamond.x)); text.setAttribute("y", String(diamond.y - s / 2 - 6));
        text.setAttribute("text-anchor", "middle"); text.classList.add("er-eng-diamond-text"); text.textContent = diamond.label;
        g.append(polygon, text);
        return g;
      } }));
    return d;
  }

  function renderEngineeringView(): void {
    const d = ensureEngineeringModel();
    setSvgSize(d.width, d.height);
    const clip = d.entities.length + d.attributes.length + d.diamonds.length > 2000;
    const bounds = diagramViewport(viewport, canvasSize.width, canvasSize.height);
    const wanted = new Set<string>();
    // 按原链接/属性/实体/菱形顺序保留 stacking，即使重新挂载也不会把线放到节点上方。
    let previous: SVGElement | null = null;
    for (const item of engineeringItems) {
      if (clip && !intersectsDiagramViewport(item.bounds, bounds)) continue;
      wanted.add(item.key);
      let element = mountedEngineering.get(item.key);
      if (!element) {
        element = item.create();
        mountedEngineering.set(item.key, element);
        edgeSvg.insertBefore(element, previous ? previous.nextSibling : edgeSvg.firstChild);
      }
      previous = element;
    }
    for (const [key, element] of mountedEngineering) {
      if (!wanted.has(key)) { element.remove(); mountedEngineering.delete(key); }
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
  const onMouseMove = (e: MouseEvent): void => {
    if (disposed) return;
    if (panning) {
      viewport.x = panStart.vx + (e.clientX - panStart.x);
      viewport.y = panStart.vy + (e.clientY - panStart.y);
      applyViewport();
      return;
    }
    if (dragCard) {
      const dx = (e.clientX - dragCard.startX) / viewport.zoom;
      const dy = (e.clientY - dragCard.startY) / viewport.zoom;
      dragCard.node.x = Math.round(dragCard.originX + dx);
      dragCard.node.y = Math.round(dragCard.originY + dy);
      dirtyCards.add(dragCard.node.table.name);
      for (const id of incidentEdges.get(dragCard.node.table.name) ?? []) dirtyEdges.add(id);
      applyViewport(false);
    }
  };
  const onMouseUp = (): void => {
    if (disposed) return;
    if (panning) {
      panning = false;
      canvas.classList.remove("panning");
    }
    if (dragCard) {
      const positions = loadPositions(dragCard.scope.connection.id, dragCard.scope.namespace);
      positions[dragCard.node.table.name] = { x: dragCard.node.x, y: dragCard.node.y };
      savePositions(dragCard.scope.connection.id, dragCard.scope.namespace, positions);
      dragCard = null;
      applyViewport();
    }
  };
  window.addEventListener("mousemove", onMouseMove);
  window.addEventListener("mouseup", onMouseUp);
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
  }, { passive: false });

  let dragCard: { node: ReturnType<typeof computeLayout>[number]; el: HTMLElement; startX: number; startY: number; originX: number; originY: number; scope: Scope } | null = null;
  function startDragCard(node: ReturnType<typeof computeLayout>[number], el: HTMLElement, e: MouseEvent): void {
    e.preventDefault();
    e.stopPropagation();
    const scope = currentScope();
    if (scope) dragCard = { node, el, startX: e.clientX, startY: e.clientY, originX: node.x, originY: node.y, scope };
  }

  // ---------------------------------------------------------------- 工具栏事件

  function resetScopeGraph(): void {
    resetDesigners();
    selectedTable = null;
    tables = [];
    relationships = [];
    dragCard = null;
    panning = false;
    canvas.classList.remove("panning");
    inspMessage.textContent = "";
    rebuildLayout(false);
    renderAll();
  }
  connSelect.addEventListener("change", () => {
    const preferred = database;
    connectionId = connSelect.value;
    database = "";
    dbSelect.replaceChildren();
    resetScopeGraph();
    void refresh(preferred);
  });
  dbSelect.addEventListener("change", () => {
    database = dbSelect.value;
    resetScopeGraph();
    void refresh();
  });
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  search.addEventListener("input", () => {
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      searchTimer = null;
      if (disposed) return;
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
    void applyElkLayout();
  });
  dirSelect.addEventListener("change", () => {
    layoutDirection = dirSelect.value as LayoutDirection;
    // 切方向即重排，否则用户要再点一次「自动布局」。
    void applyElkLayout();
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
  refreshBtn.addEventListener("click", () => void refresh(undefined, true));
  let clipboardTimer: ReturnType<typeof setTimeout> | null = null;
  copySqlBtn.addEventListener("click", () => {
    const sql = buildJoinSql(tables.filter((t) => visibleNames.has(t.name)), relationships.filter((r) => visibleNames.has(r.sourceTable) && visibleNames.has(r.targetTable)));
    void navigator.clipboard
      ?.writeText(sql)
      .then(() => {
        if (disposed) return;
        stats.textContent = "JOIN SQL 已复制到剪贴板";
        if (clipboardTimer !== null) clearTimeout(clipboardTimer);
        clipboardTimer = setTimeout(() => { clipboardTimer = null; if (!disposed) renderStats(); }, 1800);
      })
      .catch(() => {
        if (disposed) return;
        stats.textContent = "复制失败，请手动选择";
      });
  });

  // ---------------------------------------------------------------- 组装

  dlg.onClose(() => {
    disposed = true;
    ++loadGeneration;
    window.removeEventListener("mousemove", onMouseMove);
    window.removeEventListener("mouseup", onMouseUp);
    resizeObserver.disconnect();
    if (renderFrame !== null) window.cancelAnimationFrame(renderFrame);
    renderFrame = null;
    if (searchTimer !== null) clearTimeout(searchTimer);
    if (clipboardTimer !== null) clearTimeout(clipboardTimer);
    searchTimer = clipboardTimer = null;
    dragCard = null;
    panning = false;
    dirtyCards.clear();
    dirtyEdges.clear();
    mountedCards.clear();
    mountedEdges.clear();
    mountedEngineering.clear();
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
