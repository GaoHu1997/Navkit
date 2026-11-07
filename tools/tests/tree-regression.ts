import { TreePanel } from '../../src/ui/TreePanel';
import type { ConnectionConfig, ConnectionMovePosition, TreeItem } from '../../src/core/types';
import '../../src/styles/base.css';

const storageKey = 'navkit-isolated-tree-regression';
const config = (id: string, group: string): ConnectionConfig => ({ id, name: id.toUpperCase(), kind: 'sqlite', group, host: '', port: 0, user: '', password: '', database: '', file: '', params: '', read_only: false, keep_alive: false });
let connections: ConnectionConfig[] = JSON.parse(localStorage.getItem(storageKey) || 'null') ?? [config('a', ''), config('b', ''), config('c', 'folder'), config('d', 'folder')];
const moves: { id: string; group: string; targetId?: string; position?: ConnectionMovePosition }[] = [];
let fail = false;
let panel: TreePanel;
function render(): void {
  const nodes = TreePanel.buildGroups(connections, ['folder', 'empty']);
  for (const node of nodes) {
    if (node.id.startsWith('group:')) node.expanded = true;
    const children = node.type === 'connection' ? [node] : node.children;
    for (const child of children) {
      child.expanded = child.id === 'conn:c';
      child.children = child.id === 'conn:c' ? [{ id: 'conn:c/db/main', label: 'main', type: 'database', icon: 'info_pane_database', children: [], expandable: false, expanded: false, has_children: false } as TreeItem] : [];
    }
  }
  panel.setNodes(nodes);
}
panel = new TreePanel({
  onMoveConnection: async (id, group, targetId, position) => {
    moves.push({ id, group, targetId, position });
    if (fail) throw new Error('隔离模拟：磁盘只读');
    // 模拟 IPC 响应；Rust 的实际排序/落盘由 storage::tree_order_tests 验证。
    const source = connections.find(c => c.id === id);
    if (!source) throw new Error('source missing');
    const next = connections.filter(c => c.id !== id);
    const target = targetId ? next.findIndex(c => c.id === targetId) : -1;
    const last = next.map(c => c.group).lastIndexOf(group);
    const index = targetId ? target + (position === 'after' ? 1 : 0) : last >= 0 ? last + 1 : group ? next.length : 0;
    next.splice(index, 0, { ...source, group });
    connections = next;
    localStorage.setItem(storageKey, JSON.stringify(connections));
    render();
  },
  onToggle: node => { node.expanded = !node.expanded; panel.render(); },
});
document.querySelector('#tree-fixture')!.appendChild(panel.el);
render();
(window as unknown as { treeRegression: unknown }).treeRegression = { panel, moves, get connections() { return connections; }, set fail(value: boolean) { fail = value; } };
