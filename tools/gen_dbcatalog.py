# -*- coding: utf-8 -*-
"""从 src-tauri/src/catalog.rs 生成 src/core/dbcatalog.ts，保证前后端目录一致。"""
import io
import re
import sys

SRC = 'src-tauri/src/catalog.rs'
DST = 'src/core/dbcatalog.ts'

s = io.open(SRC, encoding='utf-8').read()
# 截取 CATALOG 数组体
start = s.index('pub static CATALOG: &[KindMeta] = &[')
end = s.index('\n];', start)
body = s[start:end]

# 逐条解析 k/odbc/http/native/planned(...) 调用
entries = []
i = 0
pattern = re.compile(r'\b(k|odbc|http|native|planned)\(')
while True:
    m = pattern.search(body, i)
    if not m:
        break
    # 平衡括号截取参数
    depth = 1
    j = m.end()
    while depth > 0 and j < len(body):
        if body[j] == '(':
            depth += 1
        elif body[j] == ')':
            depth -= 1
        j += 1
    inner = body[m.end():j - 1]
    # 拆顶层逗号（考虑嵌套括号）
    args, depth, cur = [], 0, ''
    for ch in inner:
        if ch in '(<':
            depth += 1
        elif ch in ')>':
            depth -= 1
        if ch == ',' and depth == 0:
            args.append(cur.strip())
            cur = ''
        else:
            cur += ch
    if cur.strip():
        args.append(cur.strip())
    kind = m.group(1)
    if kind == 'k':
        _id, label, port, _protocol, category = args
        protocol = _protocol.split('::')[-1].strip().lower()
        driver = note = ''
        planned = False
    elif kind == 'odbc':
        _id, label, port, category, driver, note = args
        protocol = 'odbc'
        planned = False
    elif kind == 'http':
        _id, label, port, category, _adapter, note = args
        protocol = 'http'
        driver = ''
        planned = False
    elif kind == 'native':
        _id, label, port, category, note = args
        protocol = 'native'
        driver = ''
        planned = False
    else:  # planned
        _id, label, port, category, note = args
        protocol = 'planned'
        driver = ''
        planned = True
    entries.append({
        'id': _id.strip('"'),
        'label': label.strip('"'),
        'port': port.strip(),
        'category': category.strip('"'),
        'protocol': protocol,
        'driver': driver.strip('"'),
        'note': note.strip('"'),
        'planned': planned,
    })
    i = j

ids = [e['id'] for e in entries]
assert len(ids) == len(set(ids)), 'duplicate ids'

CATEGORIES = [
    ('relational', '关系型数据库'),
    ('analytics', '数仓分析'),
    ('domestic', '国产数据库'),
    ('lightweight', '轻量数据库'),
    ('nosql', '文档/缓存/检索'),
    ('graph', '图谱/向量'),
    ('timeseries', '时序数据库'),
    ('mq', '消息队列'),
    ('registry', '注册与配置'),
]

def ts_str(v):
    return JSONV(v)

import json
def JSONV(v):
    return json.dumps(v, ensure_ascii=False)

lines = []
lines.append('/**')
lines.append(' * 数据源类型目录 —— 由 src-tauri/src/catalog.rs 经 tools/gen_dbcatalog.py 生成，')
lines.append(' * 请勿手工编辑（改动请改 Rust 目录后重新生成）。「新建连接」选择器与')
lines.append(' * 连接属性对话框的数据源列表、分类、默认端口、驱动提示均来源于此。')
lines.append(' */')
lines.append('')
lines.append('export type DbCategory =')
lines.append('  | "relational"')
lines.append('  | "analytics"')
lines.append('  | "domestic"')
lines.append('  | "lightweight"')
lines.append('  | "nosql"')
lines.append('  | "graph"')
lines.append('  | "timeseries"')
lines.append('  | "mq"')
lines.append('  | "registry";')
lines.append('')
lines.append('export interface KindInfo {')
lines.append('  id: string;')
lines.append('  label: string;')
lines.append('  port: number;')
lines.append('  category: DbCategory;')
lines.append('  /** 连接协议族：mysql / postgres / sqlite / mssql / redis / mongo / clickhouse / http / native / odbc / planned。 */')
lines.append('  protocol: string;')
lines.append('  /** ODBC 驱动名提示（仅 ODBC 类数据源）。 */')
lines.append('  driver: string;')
lines.append('  /** 连接说明。 */')
lines.append('  note: string;')
lines.append('  /** 适配器尚未提供（仅可浏览目录）。 */')
lines.append('  planned: boolean;')
lines.append('}')
lines.append('')
lines.append('/** 数据源分类（顺序即选择器展示顺序）。 */')
lines.append('export const DB_CATEGORIES: { id: DbCategory; label: string }[] = [')
for cid, label in CATEGORIES:
    lines.append(f'  {{ id: "{cid}", label: "{label}" }},')
lines.append('];')
lines.append('')
lines.append('export const DB_KINDS: KindInfo[] = [')
for e in entries:
    lines.append(
        '  { id: %s, label: %s, port: %s, category: "%s", protocol: "%s", driver: %s, note: %s, planned: %s },'
        % (JSONV(e['id']), JSONV(e['label']), e['port'], e['category'], e['protocol'],
           JSONV(e['driver']), JSONV(e['note']), 'true' if e['planned'] else 'false'))
lines.append('];')
lines.append('')
lines.append('const KIND_MAP = new Map<string, KindInfo>(DB_KINDS.map((k) => [k.id, k]));')
lines.append('')
lines.append('/** 按分类取数据源列表。 */')
lines.append('export function kindsByCategory(category: DbCategory): KindInfo[] {')
lines.append('  return DB_KINDS.filter((k) => k.category === category);')
lines.append('}')
lines.append('')
lines.append('export function kindMeta(id: string): KindInfo | undefined {')
lines.append('  return KIND_MAP.get(id);')
lines.append('}')
lines.append('')
lines.append('/** 显示名（未知类型原样返回）。 */')
lines.append('export function kindLabel(id: string): string {')
lines.append('  return KIND_MAP.get(id)?.label ?? id;')
lines.append('}')
lines.append('')
lines.append('/** 对象树 / 卡片使用的图标注册名。 */')
lines.append('export function kindIcon(id: string): string {')
lines.append('  return `info_pane_conn_${id}`;')
lines.append('}')
lines.append('')

io.open(DST, 'w', encoding='utf-8', newline='\n').write('\n'.join(lines))
print('generated', DST, 'with', len(entries), 'kinds')
