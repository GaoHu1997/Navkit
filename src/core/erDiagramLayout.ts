/**
 * 关系图 ELK 分层布局 —— 移植自 参考实现 `lib/diagram/elk-layout.ts`。
 *
 * 相比原自研的「最长路径分层 + 同层纵排」，ELK 的 layered 算法（Sugiyama）会做：
 *   · 交叉最小化（LAYER_SWEEP）—— 这是连线交叉严重的根因
 *   · 节点排序平衡（BRANDES_KOEPF）
 *   · 层分配最小化边跨度（NETWORK_SIMPLEX）
 *   · 连通分量自动分离（孤立表不再混进主图）
 *   · 正交边路由 + 返回拐点，可直接拿来画折线
 *
 * ELK 是异步的（Web Worker 可用时跑在 worker 里，不会卡住 UI 线程），
 * 因此所有导出函数都是 Promise；调用方必须处理「布局期间用户仍在拖拽」的竞态。
 */

import type { ErTable } from "./erDiagram";
import { CARD_WIDTH, cardHeight } from "./erDiagram";
import type { ErRelationship } from "./erDiagram";
import type { Point } from "./erDiagramRouting";
import { collapseColinearPoints, dedupePoints } from "./erDiagramRouting";

/** 布局方向：LR 左→右 / TB 上→下 / RL 右→左 / BT 下→上。 */
export type LayoutDirection = "LR" | "TB" | "RL" | "BT";

export interface LayoutOptions {
  direction?: LayoutDirection;
}

export interface LayoutNodeInput {
  table: ErTable;
  width: number;
  height: number;
}

export interface LayoutEdgeInput {
  id: string;
  source: string;
  target: string;
}

export interface LayoutResult {
  /** 表名 → 左上角坐标。 */
  positions: Record<string, Point>;
  /** 关系 id → 折线（含两端），ELK 算出的正交路径。 */
  edgeWaypoints: Record<string, Point[]>;
  /** 布局是否真的执行成功；false 时调用方应保留原坐标。 */
  applied: boolean;
}

interface ElkNode {
  id: string;
  width: number;
  height: number;
  x?: number;
  y?: number;
}

interface ElkSection {
  startPoint: Point;
  endPoint: Point;
  bendPoints?: Point[];
}

interface ElkEdge {
  id: string;
  sources: string[];
  targets: string[];
  sections?: ElkSection[];
}

interface ElkGraph {
  id: string;
  children: ElkNode[];
  edges: ElkEdge[];
  layoutOptions?: Record<string, string>;
}

/**
 * 单例：ELK 内部会创建 worker，重复实例化代价高。
 *
 * 用动态 import 按需加载：elkjs 打包后约 1.6MB（gzip ~430KB），
 * 只有用户真正点「自动布局」时才拉取，不拖慢应用启动和首屏。
 */
type ElkInstance = { layout: (graph: unknown) => Promise<unknown> };
let elkInstance: ElkInstance | null = null;
let elkLoading: Promise<ElkInstance | null> | null = null;

async function loadElk(): Promise<ElkInstance | null> {
  if (elkInstance) return elkInstance;
  if (elkLoading) return elkLoading;
  elkLoading = import("elkjs/lib/elk.bundled.js")
    .then((mod) => {
      const Ctor = (mod as { default: new () => ElkInstance }).default;
      elkInstance = new Ctor();
      return elkInstance;
    })
    .catch(() => null)
    .finally(() => {
      elkLoading = null;
    });
  return elkLoading;
}

/** 布局节点/边超过此数量时跳过 ELK（Sugiyama 在大图上会明显变慢），退回网格。 */
export const ELK_NODE_LIMIT = 400;
export const ELK_EDGE_LIMIT = 1200;

const DIRECTION_MAP: Record<LayoutDirection, string> = {
  LR: "RIGHT",
  TB: "DOWN",
  RL: "LEFT",
  BT: "UP",
};

/** 参考实现 的同一套参数，按本项目卡片尺寸微调间距。 */
export function buildElkOptions(direction: LayoutDirection = "LR"): Record<string, string> {
  return {
    "elk.algorithm": "layered",
    "elk.direction": DIRECTION_MAP[direction],
    "elk.layered.edgeRouting": "ORTHOGONAL",
    "elk.layered.nodePlacement": "BRANDES_KOEPF",
    "elk.layered.crossingMinimization": "LAYER_SWEEP",
    "elk.layered.layering.strategy": "NETWORK_SIMPLEX",
    "elk.layered.separateConnectedComponents": "true",
    "elk.spacing.nodeNode": "48",
    "elk.spacing.layerLayer": "72",
    "elk.padding": "[top=40,left=40,bottom=40,right=40]",
    "elk.layered.edgeSpacing": "18",
    "elk.layered.nodeSpacing": "40",
    "elk.layered.bendPointSpacing": "10",
    "elk.hierarchyHandling": "INCLUDE_CHILDREN",
  };
}

/**
 * 计算分层布局。
 * @param nodes  参与布局的表（已按可见性过滤）
 * @param edges  参与布局的关系（两端都必须在 nodes 内）
 */
export async function computeElkLayout(
  nodes: readonly LayoutNodeInput[],
  edges: readonly LayoutEdgeInput[],
  options: LayoutOptions = {},
): Promise<LayoutResult> {
  if (nodes.length === 0) return { positions: {}, edgeWaypoints: {}, applied: false };

  const graph: ElkGraph = {
    id: "root",
    children: nodes.map((node) => ({
      id: node.table.name,
      width: node.width || CARD_WIDTH,
      height: node.height || cardHeight(node.table),
    })),
    edges: edges.map((edge) => ({ id: edge.id, sources: [edge.source], targets: [edge.target] })),
  };

  let laid: ElkGraph;
  try {
    const instance = await loadElk();
    if (!instance) return { positions: {}, edgeWaypoints: {}, applied: false };
    laid = (await instance.layout({
      ...graph,
      layoutOptions: buildElkOptions(options.direction ?? "LR"),
    })) as ElkGraph;
  } catch {
    // 布局失败（极端图结构 / worker 不可用）不抛给 UI，由调用方回退。
    return { positions: {}, edgeWaypoints: {}, applied: false };
  }

  const positions: Record<string, Point> = {};
  for (const child of laid.children ?? []) {
    if (typeof child.x === "number" && typeof child.y === "number") {
      positions[child.id] = { x: child.x, y: child.y };
    }
  }

  const edgeWaypoints: Record<string, Point[]> = {};
  for (const edge of laid.edges ?? []) {
    const sections = edge.sections;
    if (!sections?.length) continue;
    const flat: Point[] = [];
    sections.forEach((section, index) => {
      const part: Point[] = [section.startPoint, ...(section.bendPoints ?? []), section.endPoint];
      flat.push(...(index === 0 ? part : part.slice(1)));
    });
    const cleaned = collapseColinearPoints(dedupePoints(flat));
    if (cleaned.length >= 2) edgeWaypoints[edge.id] = cleaned;
  }

  return { positions, edgeWaypoints, applied: Object.keys(positions).length > 0 };
}

/**
 * 网格兜底布局：表按连通分量聚簇后 LTR 排列。
 * 用于 ELK 不可用、节点过多、或用户选择「紧凑网格」时。
 */
export function computeGridLayout(nodes: readonly LayoutNodeInput[], relationships: readonly Pick<ErRelationship, "sourceTable" | "targetTable">[] = [], paneWidth = 1200): Record<string, Point> {
  if (nodes.length === 0) return {};
  const gapX = 48;
  const gapY = 36;
  const margin = 40;
  const columns = Math.max(1, Math.min(Math.ceil(Math.sqrt(nodes.length)), Math.floor((paneWidth - margin * 2) / (CARD_WIDTH + gapX)) || 1));

  const ordered = orderByConnectivity(nodes, relationships);
  const positions: Record<string, Point> = {};
  let y = margin;
  for (let start = 0; start < ordered.length; start += columns) {
    const row = ordered.slice(start, start + columns);
    let rowHeight = 0;
    row.forEach((node, col) => {
      const height = node.height || cardHeight(node.table);
      positions[node.table.name] = { x: margin + col * (CARD_WIDTH + gapX), y };
      rowHeight = Math.max(rowHeight, height);
    });
    y += rowHeight + gapY;
  }
  return positions;
}

/** 让有关系的表相邻（DFS 连通分量），孤立表排最后。 */
export function orderByConnectivity<T extends LayoutNodeInput>(nodes: readonly T[], relationships: readonly Pick<ErRelationship, "sourceTable" | "targetTable">[]): T[] {
  if (nodes.length <= 1 || relationships.length === 0) return [...nodes];
  const byName = new Map(nodes.map((node) => [node.table.name, node]));
  const adj = new Map<string, Set<string>>();
  for (const node of nodes) adj.set(node.table.name, new Set());
  for (const rel of relationships) {
    if (!byName.has(rel.sourceTable) || !byName.has(rel.targetTable)) continue;
    if (rel.sourceTable === rel.targetTable) continue;
    adj.get(rel.sourceTable)?.add(rel.targetTable);
    adj.get(rel.targetTable)?.add(rel.sourceTable);
  }

  const visited = new Set<string>();
  const ordered: T[] = [];
  const visit = (start: string): void => {
    const stack = [start];
    const component: string[] = [];
    while (stack.length) {
      const name = stack.pop()!;
      if (visited.has(name)) continue;
      visited.add(name);
      component.push(name);
      for (const next of adj.get(name) ?? []) if (!visited.has(next)) stack.push(next);
    }
    component.sort((a, b) => a.localeCompare(b));
    for (const name of component) {
      const node = byName.get(name);
      if (node) ordered.push(node);
    }
  };

  const withEdges = nodes.filter((node) => (adj.get(node.table.name)?.size ?? 0) > 0).sort((a, b) => a.table.name.localeCompare(b.table.name));
  const isolates = nodes.filter((node) => (adj.get(node.table.name)?.size ?? 0) === 0).sort((a, b) => a.table.name.localeCompare(b.table.name));
  for (const node of withEdges) if (!visited.has(node.table.name)) visit(node.table.name);
  for (const node of isolates) if (!visited.has(node.table.name)) ordered.push(node);
  return ordered;
}
