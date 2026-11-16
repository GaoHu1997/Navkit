/**
 * 关系图正交边避障路由 —— 纯逻辑，无框架依赖。
 *
 * 移植自 参考实现 `lib/diagram/edge-obstacle-router.ts`，移除了 Vue Flow 的 `Position` 枚举，
 * 改为本项目的 `ErSide` 字符串联合。解决的问题：连线直接穿过中间的表卡片，图一复杂就不可读。
 *
 * 算法：从源端出口点出发，枚举 12 条正交候选走廊（H 型 / V 型 / 绕行 / 上绕 / 下绕 / 左绕 / 右绕），
 * 过滤掉穿过障碍物或贴着端点表卡片的候选，取最短的一条。
 */

/** 卡片边缘方向。 */
export type ErSide = "left" | "right" | "top" | "bottom";

export interface Point {
  x: number;
  y: number;
}

export interface ObstacleRect {
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface RouteInput {
  source: Point;
  target: Point;
  /** 源端引出方向；省略时按相对位置推断。 */
  sourceSide?: ErSide;
  /** 目标端引入方向；省略时按相对位置推断。 */
  targetSide?: ErSide;
  obstacles: readonly ObstacleRect[];
  /** 两端表卡片 id，路由时忽略它们。 */
  endpointIds: readonly [string, string];
  offset?: number;
}

/** 拐弯处离卡片边缘的留白。 */
export const EDGE_ROUTE_OFFSET = 36;
const AXIS_EPS = 0.5;

function nearlyEqual(a: number, b: number): boolean {
  return Math.abs(a - b) <= AXIS_EPS;
}

function inflate(rect: ObstacleRect, pad: number): ObstacleRect {
  return { ...rect, x: rect.x - pad, y: rect.y - pad, width: rect.width + pad * 2, height: rect.height + pad * 2 };
}

function segmentIntersectsRect(a: Point, b: Point, rect: ObstacleRect): boolean {
  const minX = Math.min(a.x, b.x);
  const maxX = Math.max(a.x, b.x);
  const minY = Math.min(a.y, b.y);
  const maxY = Math.max(a.y, b.y);
  const rx2 = rect.x + rect.width;
  const ry2 = rect.y + rect.height;
  if (maxX < rect.x || minX > rx2 || maxY < rect.y || minY > ry2) return false;
  if (nearlyEqual(a.x, b.x)) return a.x >= rect.x && a.x <= rx2 && maxY >= rect.y && minY <= ry2;
  if (nearlyEqual(a.y, b.y)) return a.y >= rect.y && a.y <= ry2 && maxX >= rect.x && minX <= rx2;
  return true;
}

export function pathHitsObstacles(points: readonly Point[], obstacles: readonly ObstacleRect[]): boolean {
  for (let i = 0; i < points.length - 1; i++) {
    for (const rect of obstacles) {
      if (segmentIntersectsRect(points[i], points[i + 1], rect)) return true;
    }
  }
  return false;
}

function relevantObstacles(input: RouteInput, pad: number): ObstacleRect[] {
  const [srcId, tgtId] = input.endpointIds;
  return input.obstacles
    .filter((rect) => rect.id !== srcId && rect.id !== tgtId)
    .map((rect) => inflate(rect, pad));
}

function endpointRects(obstacles: readonly ObstacleRect[], endpointIds: readonly [string, string], pad = 0): ObstacleRect[] {
  const [srcId, tgtId] = endpointIds;
  return obstacles
    .filter((rect) => rect.id === srcId || rect.id === tgtId)
    .map((rect) => (pad > 0 ? inflate(rect, pad) : { ...rect }));
}

function stubOut(point: Point, side: ErSide, offset: number): Point {
  if (side === "left") return { x: point.x - offset, y: point.y };
  if (side === "top") return { x: point.x, y: point.y - offset };
  if (side === "bottom") return { x: point.x, y: point.y + offset };
  return { x: point.x + offset, y: point.y };
}

function stubIn(point: Point, side: ErSide, offset: number): Point {
  if (side === "right") return { x: point.x + offset, y: point.y };
  if (side === "top") return { x: point.x, y: point.y - offset };
  if (side === "bottom") return { x: point.x, y: point.y + offset };
  return { x: point.x - offset, y: point.y };
}

function pointOnRectBorder(p: Point, rect: ObstacleRect): boolean {
  const rx2 = rect.x + rect.width;
  const ry2 = rect.y + rect.height;
  const onVertical = (nearlyEqual(p.x, rect.x) || nearlyEqual(p.x, rx2)) && p.y >= rect.y - AXIS_EPS && p.y <= ry2 + AXIS_EPS;
  const onHorizontal = (nearlyEqual(p.y, rect.y) || nearlyEqual(p.y, ry2)) && p.x >= rect.x - AXIS_EPS && p.x <= rx2 + AXIS_EPS;
  return onVertical || onHorizontal;
}

/**
 * 线段是否沿/穿过端点卡片。
 *
 * 只禁止**穿过卡片内部**的段：与卡片矩形有真实重叠（非零面积）才判定为违规。
 * 仅擦到边界（例如从卡片上边竖直引出的线恰好经过矩形边界）不算，否则几乎所有
 * 合法的引出/引入段都会被误判，导致路由全盘返回 null。
 * 例外：短引出段（≤ stubLen）允许贴边，保持与 参考实现 一致。
 */
export function pathSkimsEndpoints(points: readonly Point[], endpointRectsList: readonly ObstacleRect[], stubLen = EDGE_ROUTE_OFFSET): boolean {
  if (endpointRectsList.length === 0 || points.length < 2) return false;
  for (let i = 0; i < points.length - 1; i++) {
    const a = points[i];
    const b = points[i + 1];
    const segLen = Math.hypot(b.x - a.x, b.y - a.y);
    for (const rect of endpointRectsList) {
      if (!segmentIntersectsRect(a, b, rect)) continue;
      // 短引出段贴边放行。
      if (segLen <= stubLen + AXIS_EPS && (pointOnRectBorder(a, rect) || pointOnRectBorder(b, rect))) continue;
      // 只有真正进入卡片内部（收缩 1px 后仍相交）才算违规。
      const inner = { ...rect, x: rect.x + 1, y: rect.y + 1, width: Math.max(0, rect.width - 2), height: Math.max(0, rect.height - 2) };
      if (segmentIntersectsRect(a, b, inner)) return true;
    }
  }
  return false;
}

export function dedupePoints(points: readonly Point[]): Point[] {
  const out: Point[] = [];
  for (const p of points) {
    const last = out[out.length - 1];
    if (!last || !nearlyEqual(last.x, p.x) || !nearlyEqual(last.y, p.y)) out.push({ x: p.x, y: p.y });
  }
  return out;
}

/** 合并同轴相邻点。 */
export function collapseColinearPoints(points: readonly Point[]): Point[] {
  if (points.length <= 2) return points.map((p) => ({ ...p }));
  const out: Point[] = [{ ...points[0] }];
  for (let i = 1; i < points.length - 1; i++) {
    const prev = out[out.length - 1];
    const cur = points[i];
    const next = points[i + 1];
    if ((nearlyEqual(prev.y, cur.y) && nearlyEqual(cur.y, next.y)) || (nearlyEqual(prev.x, cur.x) && nearlyEqual(cur.x, next.x))) continue;
    out.push({ ...cur });
  }
  out.push({ ...points[points.length - 1] });
  return dedupePoints(out);
}

export function polylineLength(points: readonly Point[]): number {
  let total = 0;
  for (let i = 1; i < points.length; i++) total += Math.hypot(points[i].x - points[i - 1].x, points[i].y - points[i - 1].y);
  return total;
}

function isOrthogonalPolyline(points: readonly Point[]): boolean {
  for (let i = 0; i < points.length - 1; i++) {
    if (!nearlyEqual(points[i].x, points[i + 1].x) && !nearlyEqual(points[i].y, points[i + 1].y)) return false;
  }
  return true;
}

/** 两点的正交折线（两个拐角取短者）。 */
function orthogonalConnect(a: Point, b: Point): Point[] {
  if (nearlyEqual(a.x, b.x) || nearlyEqual(a.y, b.y)) return [a, b];
  const viaH: Point[] = [a, { x: b.x, y: a.y }, b];
  const viaV: Point[] = [a, { x: a.x, y: b.y }, b];
  return polylineLength(viaH) <= polylineLength(viaV) ? viaH : viaV;
}

/** 按相对位置推断出口方向（源 → 目标）。 */
export function inferSide(from: Point, to: Point): ErSide {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  if (Math.abs(dx) >= Math.abs(dy)) return dx >= 0 ? "right" : "left";
  return dy >= 0 ? "bottom" : "top";
}

/** 目标端的引入方向（源在目标的哪一侧进入）。 */
export function inferTargetSide(source: Point, target: Point): ErSide {
  const side = inferSide(source, target);
  if (side === "right") return "left";
  if (side === "left") return "right";
  if (side === "bottom") return "top";
  return "bottom";
}

/**
 * 生成绕开障碍的正交折线；找不到合法走廊时返回 null（调用方回退到直线/普通折线）。
 *
 * 相对 参考实现 原实现的增强（都是实测会画出穿卡片连线的情形）：
 *  1. 除固定 offset 通道外，再按**障碍物自身边界**生成通道。参考实现 只用固定 offset，
 *     遇到比 offset 更高的卡片时全部候选都判碰撞，直接返回 null。
 *  2. 每个通道同时尝试「带引出段(stub)」与「不带引出段」两种端点处理。卡片间距小于
 *     offset 时，带 stub 的路径会扎进相邻卡片的膨胀区，此时不带 stub 的直上直下才是
 *     唯一可行解。裸变体按 s.x / t.x（或 s.y / t.y）对齐，保证全程正交。
 */
export function routeOrthogonalAroundObstacles(input: RouteInput): Point[] | null {
  const offset = input.offset ?? EDGE_ROUTE_OFFSET;
  const obstacles = relevantObstacles(input, 6);
  const endpoints = endpointRects(input.obstacles, input.endpointIds, 0);
  const { source: s, target: t } = input;
  const sourceSide = input.sourceSide ?? inferSide(s, t);
  const targetSide = input.targetSide ?? inferTargetSide(s, t);
  const so = stubOut(s, sourceSide, offset);
  const si = stubIn(t, targetSide, offset);

  // 通道候选：y（水平通道）与 x（竖直通道）。
  const ys = new Set<number>([s.y, t.y, (s.y + t.y) / 2, s.y + offset, s.y - offset, t.y + offset, t.y - offset]);
  const xs = new Set<number>([s.x, t.x, (s.x + t.x) / 2, s.x + offset, s.x - offset, t.x + offset, t.x - offset]);
  const gap = 10;
  for (const rect of obstacles) {
    ys.add(rect.y - gap);
    ys.add(rect.y + rect.height + gap);
    xs.add(rect.x - gap);
    xs.add(rect.x + rect.width + gap);
  }

  let best: Point[] | null = null;
  let bestLen = Infinity;
  const consider = (points: Point[]): void => {
    const cleaned = collapseColinearPoints(dedupePoints(points));
    if (cleaned.length < 2) return;
    if (!isOrthogonalPolyline(cleaned)) return;
    if (pathHitsObstacles(cleaned, obstacles)) return;
    if (pathSkimsEndpoints(cleaned, endpoints, offset)) return;
    const len = polylineLength(cleaned);
    if (len < bestLen) {
      bestLen = len;
      best = cleaned;
    }
  };

  for (const y of ys) {
    // 带引出段：s → so → (so.x,y) → (si.x,y) → si → t
    consider([s, so, { x: so.x, y }, { x: si.x, y }, si, t]);
    // 裸端点：s → (s.x,y) → (t.x,y) → t
    consider([s, { x: s.x, y }, { x: t.x, y }, t]);
  }
  for (const x of xs) {
    consider([s, so, { x, y: so.y }, { x, y: si.y }, si, t]);
    consider([s, { x, y: s.y }, { x, y: t.y }, t]);
  }

  return best;
}

/**
 * 路由入口：优先避障；失败则退回「三段式正交折线」，保证永远有线可画。
 * @param fallbackStub 回退折线的中段偏移
 */
export function routeEdge(input: RouteInput, fallbackStub = 24): Point[] {
  const routed = routeOrthogonalAroundObstacles(input);
  if (routed) return routed;
  const sourceSide = input.sourceSide ?? inferSide(input.source, input.target);
  const { x: x1, y: y1 } = input.source;
  const { x: x2, y: y2 } = input.target;
  if (sourceSide === "left" || sourceSide === "right") {
    const dir = sourceSide === "left" ? -1 : 1;
    const mx1 = x1 + dir * fallbackStub;
    const mx2 = x2 + (x2 > x1 ? -fallbackStub : fallbackStub);
    if (nearlyEqual(y1, y2)) return [{ x: x1, y: y1 }, { x: x2, y: y2 }];
    if ((dir === 1 && mx1 <= mx2) || (dir === -1 && mx1 >= mx2)) {
      return [{ x: x1, y: y1 }, { x: mx1, y: y1 }, { x: mx1, y: y2 }, { x: mx2, y: y2 }, { x: x2, y: y2 }];
    }
    const midX = (x1 + x2) / 2;
    return [{ x: x1, y: y1 }, { x: midX, y: y1 }, { x: midX, y: y2 }, { x: x2, y: y2 }];
  }
  const midY = (y1 + y2) / 2;
  return [{ x: x1, y: y1 }, { x: x1, y: midY }, { x: x2, y: midY }, { x: x2, y: y2 }];
}

/** 折线 → SVG path d。 */
export function pointsToSvgPath(points: readonly Point[]): string {
  if (points.length === 0) return "";
  return points.map((p, i) => `${i === 0 ? "M" : "L"} ${p.x} ${p.y}`).join(" ");
}

/** 沿折线按弧长取点（t ∈ [0,1]）。 */
export function pointAlongPolyline(points: readonly Point[], t: number): Point {
  if (points.length === 0) return { x: 0, y: 0 };
  if (points.length === 1) return { ...points[0] };
  const clamped = Math.min(1, Math.max(0, t));
  const segs: number[] = [];
  let total = 0;
  for (let i = 0; i < points.length - 1; i++) {
    const d = Math.hypot(points[i + 1].x - points[i].x, points[i + 1].y - points[i].y);
    segs.push(d);
    total += d;
  }
  if (total === 0) return { ...points[0] };
  let remain = total * clamped;
  for (let i = 0; i < segs.length; i++) {
    if (remain <= segs[i]) {
      const ratio = segs[i] === 0 ? 0 : remain / segs[i];
      return { x: points[i].x + (points[i + 1].x - points[i].x) * ratio, y: points[i].y + (points[i + 1].y - points[i].y) * ratio };
    }
    remain -= segs[i];
  }
  return { ...points[points.length - 1] };
}

export function midpointAlongPolyline(points: readonly Point[]): Point {
  return pointAlongPolyline(points, 0.5);
}

export { isOrthogonalPolyline, orthogonalConnect };
