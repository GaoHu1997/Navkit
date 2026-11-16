/** 参考实现 的可见区域切片原则，适配现有 transform 视口；不改变图元或交互。 */
export interface DiagramBounds { left: number; top: number; right: number; bottom: number }
export function diagramViewport(viewport: { x: number; y: number; zoom: number }, width: number, height: number, overscan = 800): DiagramBounds {
  const zoom = viewport.zoom;
  return { left: -viewport.x / zoom - overscan, top: -viewport.y / zoom - overscan,
    right: (width - viewport.x) / zoom + overscan, bottom: (height - viewport.y) / zoom + overscan };
}
export function intersectsDiagramViewport(bounds: DiagramBounds, viewport: DiagramBounds): boolean {
  return bounds.left <= viewport.right && bounds.right >= viewport.left && bounds.top <= viewport.bottom && bounds.bottom >= viewport.top;
}
/** 当前正交路径中间段和端点徽标都落在端点包围盒加 stub/标签留白内。
 * 连线单独裁剪，保留两端离屏但穿过视口的路径。 */
export function edgeBounds(e: { x1: number; y1: number; x2: number; y2: number }): DiagramBounds {
  return { left: Math.min(e.x1, e.x2) - 32, top: Math.min(e.y1, e.y2) - 24,
    right: Math.max(e.x1, e.x2) + 32, bottom: Math.max(e.y1, e.y2) + 24 };
}
