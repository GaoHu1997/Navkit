/** 异步对象详情。仅展示可验证的元数据；调用方负责选中版本/取消，防止旧响应串选。 */
import { api } from "./api";
import { DB_KINDS } from "./dbcatalog";
import { namespaceName } from "./namespace-label";
import type { ConnectionConfig, ObjectType } from "./types";
import type { InfoField, InfoPanelState } from "../ui/InfoPanel";
import { formatByteCount } from "./format-size";

export interface ObjectDetailsOptions {
  /** 默认对象详情；备份项不是数据库对象，通过此模式明确指定。 */
  category?: "object" | "backup";
  /** 文件名按 listBackups 返回的 name 精确匹配；未提供时展示备份分组汇总。 */
  backupName?: string;
  /** 列/索引详情必须由选择上下文提供父表，不从有歧义的对象名猜测。 */
  parentTable?: string;
}
export type ObjectDetailsApi = Pick<typeof api, "tableStatus" | "getTableSchema" | "getViewDefinition" |
  "getRoutineDefinition" | "listObjects" | "listRoutines" | "serverInfo" | "listBackups">;
const labels: Partial<Record<ObjectType, string>> = {
  connection: "连接", database: "数据库", schema: "模式", table: "表", view: "视图",
  procedure: "存储过程", function: "函数", trigger: "触发器", event: "事件", sequence: "序列",
  query: "查询", folder: "对象目录", column: "列", index: "索引", field: "字段",
};
// 字段名只用名称，不带括号备注；单位、来源等说明放在「统计说明」备注行里。
const statusLabels: Record<string, string> = {
  engine: "引擎", rows: "行数", avg_row_length: "平均行长度",
  data_length: "数据大小", max_data_length: "最大数据长度", index_length: "索引大小",
  data_free: "空闲空间", auto_increment: "下一个自增值", create_time: "创建时间",
  update_time: "修改时间", check_time: "检查时间", collation: "排序规则",
  checksum: "校验和", create_options: "创建选项", comment: "注释", total_bytes: "总大小",
};
const errorText = (e: unknown) => e instanceof Error ? e.message : String(e);
/** 服务器表状态里以字节为单位的统计键。 */
const BYTE_STATUS_KEYS = new Set(["avg_row_length", "data_length", "max_data_length", "index_length", "data_free", "total_bytes"]);

/** App 可直接 await loadObjectDetails(connection, schema, object, type)，无副作用/无缓存。 */
export async function loadObjectDetails(
  connection: ConnectionConfig, schema: string, object: string, type: ObjectType,
  options: ObjectDetailsOptions = {}, source: ObjectDetailsApi = api,
): Promise<InfoPanelState> {
  const protocol = DB_KINDS.find(k => k.id === connection.kind)?.protocol;
  const state: InfoPanelState = {
    icon: type === "connection" ? `info_pane_conn_${connection.kind}` : `info_pane_${type === "database" || type === "schema" ? "database" : type}`,
    title: object || schema || connection.name, subtitle: type === "schema" ? "模式" : type === "database" ? namespaceName(connection) : labels[type] ?? type,
    refs: type === "connection" ? [] : [{ icon: `info_pane_conn_${connection.kind}`, text: connection.name },
      ...(schema && type !== "database" && type !== "schema" ? [{ icon: "info_pane_database", text: schema }] : [])],
    fields: [],
  };
  const add = (label: string, value: unknown) => {
    if (value !== null && value !== undefined && String(value).trim() !== "") state.fields.push({ label, value: String(value) });
  };
  const read = async <T>(label: string, task: () => Promise<T>): Promise<T | undefined> => {
    try { return await task(); } catch (e) { add(`${label}读取失败`, errorText(e)); return undefined; }
  };
  const addConnectionContext = () => {
    add("连接", connection.name);
    add("连接类型", DB_KINDS.find(k => k.id === connection.kind)?.label ?? connection.kind);
    if (connection.file) add("数据库文件", connection.file);
    add("主机", connection.host); add("端口", connection.port || undefined);
    add("用户名", connection.user); add("默认数据库", connection.database);
    add("只读连接", connection.read_only ? "是" : "否");
  };
  const addServerInfo = (info: Awaited<ReturnType<ObjectDetailsApi["serverInfo"]>> | undefined) => {
    if (!info) return;
    add("服务器产品", info.product); add("服务器版本", info.version);
    // SQLite serverInfo 的 charset 现为固定 UTF-8，不将它当真实数据库编码。
    if (protocol !== "sqlite") add("服务器字符集", info.charset);
    add("服务器排序规则", info.collation); add("服务器运行时间", info.uptime);
    for (const [key, value] of info.extra) add(key, value);
    if (!info.version) add("状态", "服务器未提供版本；连接状态请以实际查询为准");
  };
  const addSum = (label: string, values: number[], bytes = false) => {
    const valid = values.filter(value => Number.isSafeInteger(value) && value >= 0);
    if (!values.length) { add(label, 0); return; }
    if (valid.length !== values.length) { add(`${label}状态`, "部分统计缺失/无效，未生成总计"); return; }
    const total = valid.reduce((sum, value) => sum + BigInt(value), 0n);
    // 字节总量统一进位显示（KB/MB/GB），原始字节数保留在括号内供核对。
    add(label, bytes && total <= BigInt(Number.MAX_SAFE_INTEGER) ? formatByteCount(Number(total)) : total.toString());
  };
  if (options.category === "backup") {
    const singleFile = options.backupName !== undefined;
    state.icon = "info_pane_backup"; state.subtitle = singleFile ? "备份文件" : "备份分组";
    add("所属连接", connection.name); add(`所属${namespaceName(connection)}`, schema);
    const files = await read("备份", () => source.listBackups(connection.id, schema));
    if (files && singleFile) {
      const f = files.find(f => f.name === options.backupName);
      // 大小进位显示，括号内保留原始字节数（可核对）。
      if (f) { state.title = f.name; add("修改时间", f.modified); add("文件大小", formatByteCount(f.size)); add("注释", f.comment); }
      else add("状态", "备份文件不存在或已删除");
    } else if (files) {
      add("备份文件数量", files.length);
      addSum("备份总大小", files.map(file => file.size), true);
      // 后端mtime为本地ISO格式，取可解析时间中的最新者，不用名称排序冒充mtime排序。
      const dated = files.map(file => ({ file, time: Date.parse(file.modified.replace(" ", "T")) }))
        .filter(item => item.file.modified.trim() && Number.isFinite(item.time));
      const latest = dated.reduce<typeof dated[number] | undefined>((current, item) => !current || item.time > current.time ? item : current, undefined);
      if (latest) { add("最新修改时间", latest.file.modified); add("最新修改的备份", latest.file.name); }
      if (dated.length < files.length) add("修改时间状态", "部分文件未提供可解析的修改时间，最新时间仅比较已知值");
      if (!files.length) add("状态", "后端未返回备份文件");
    }
    return state;
  }
  if (type === "connection") {
    state.title = connection.name;
    add("类型", DB_KINDS.find(k => k.id === connection.kind)?.label ?? connection.kind);
    if (connection.file) add("数据库文件", connection.file);
    else { add("主机", connection.host); add("端口", connection.port || undefined); }
    add("用户名", connection.user); add("默认数据库", connection.database); add("只读连接", connection.read_only ? "是" : "否");
    // 连接自身的属性：名称只用属性名，来源说明由分组标题承担，不写进字段名括号里。
    const info = await read("服务器", () => source.serverInfo(connection.id));
    addServerInfo(info);
  } else if (type === "database" || type === "schema" || type === "folder") {
    addConnectionContext();
    add(`${type === "schema" ? "模式" : namespaceName(connection)}名称`, schema || object);
    const [objects, routines, server] = await Promise.all([
      read("对象列表", () => source.listObjects(connection.id, schema || object)),
      read("例程列表", () => source.listRoutines(connection.id, schema || object)),
      read("服务器", () => source.serverInfo(connection.id)),
    ]);
    addServerInfo(server);
    if (objects) {
      const tables = objects.filter(o => o.type === "table");
      add("表数量", tables.length);
      add("视图数量", objects.filter(o => o.type === "view").length);
      // 仅聚合真实有统计来源的协议；其它适配器的0为占位，不能成为总行数/总大小。
      if (protocol === "mysql" || protocol === "postgres" || protocol === "sqlite") {
        addSum("表总行数", tables.map(table => table.rows));
        // 大小进位显示（值内保留原始字节数）：MySQL 为数据大小，Postgres 为含索引/TOAST 的存储大小。
        if (protocol !== "sqlite") addSum(protocol === "postgres" ? "表总存储大小" : "表总数据大小", tables.map(table => table.dataLength), true);
        add("统计说明", protocol === "sqlite"
          ? "表行数汇总自对象列表；SQLite对象大小为占位值，未计入总大小。视图未计入汇总。"
          : protocol === "postgres"
            ? "汇总自服务器对象元数据，行数为估计，可能因统计刷新而变化；不是 COUNT(*)。存储大小已含索引与 TOAST。视图未计入汇总。"
            : "汇总自服务器对象元数据，行数为估计，可能因统计刷新而变化；不是 COUNT(*)。视图未计入汇总。");
      } else add("统计说明", "当前协议对象列表未保证提供真实行数/大小，未聚合占位统计值。");
    }
    if (routines) {
      for (const kind of ["procedure", "function", "trigger", "event", "sequence"] as const) add(`${labels[kind]}数量`, routines.filter(([t]) => t === kind).length);
    }
  } else if (type === "table" || type === "view") {
    const [structure, status] = await Promise.all([
      read("结构", () => source.getTableSchema(connection.id, schema, object)),
      type === "table" ? read("表状态", () => source.tableStatus(connection.id, schema, object)) : Promise.resolve(undefined),
    ]);
    if (structure) {
      add("列数量", structure.columns.length);
      add("主键列", structure.columns.filter(c => c.primary_key).map(c => c.name).join(", ") || "无");
      add("索引数量", structure.indexes.length); add("外键数量", structure.foreign_keys.length);
      add("引擎", structure.engine); add("排序规则", structure.collation); add("注释", structure.comment);
      if (structure.row_count !== null) add("行数", structure.row_count);
    }
    // 名称之外的说明（单位、来源、是否整库）统一写进「统计说明」，不再塞进字段名的括号里。
    const notes: string[] = [];
    if (status) for (const [key, value] of status) {
      const lower = key.toLowerCase();
      // 现有 SQLite tableStatus 的 Data_length 实为整库页数×页大小，Collation 被硬填 UTF-8，不能当表属性。
      if (protocol === "sqlite" && lower === "collation") continue;
      // SQLite 的 Data_length 是整库文件大小（字节），不是单表大小：保留原始字节数不进位。
      const wholeDbSize = protocol === "sqlite" && lower === "data_length";
      const label = wholeDbSize ? "数据库文件大小" : statusLabels[lower] ?? key;
      // 体积类统计统一进位显示（值内保留原始字节数）。
      const shown = BYTE_STATUS_KEYS.has(lower) && !wholeDbSize && /^\d+$/.test(value.trim()) ? formatByteCount(Number(value)) : value;
      if (label === "行数") {
        // 结构已给出精确行数时不再重复展示服务器估计值。
        if (state.fields.some(f => f.label === "行数")) continue;
        notes.push("行数取自服务器统计信息，是估计值，不是精确 COUNT(*)。");
      }
      if (!state.fields.some(f => f.label === label && f.value === shown)) add(label, shown);
    }
    if (protocol === "sqlite") notes.push("数据库文件大小是整个数据库文件的大小（单位：字节），不是单表大小。");
    if (notes.length) add("统计说明", notes.join(" "));
    if (type === "view") {
      const definition = await read("视图定义", () => source.getViewDefinition(connection.id, schema, object));
      add("定义", definition);
    }
  } else if (type === "column" || type === "field" || type === "index") {
    if (!options.parentTable) add("状态", "选择上下文未提供父表，无法读取列/索引详情");
    else {
      add("所属表", options.parentTable);
      const structure = await read("父表结构", () => source.getTableSchema(connection.id, schema, options.parentTable!));
      if (structure && type === "index") {
        const index = structure.indexes.find(i => i.name === object);
        if (!index) add("状态", "索引不存在或当前驱动未提供索引元数据");
        else { add("列", index.columns.join(", ")); add("唯一索引", index.unique ? "是" : "否"); add("主键索引", index.primary ? "是" : "否"); add("索引方法", index.method); }
      } else if (structure) {
        const column = structure.columns.find(c => c.name === object);
        if (!column) add("状态", "字段不存在或已删除");
        else {
          add("类型", column.type); add("可为空", column.nullable ? "是" : "否"); add("主键", column.primary_key ? "是" : "否");
          add("自增/标识", column.auto_increment ? "是" : "否"); add("默认值", column.default ?? "未设置");
          add("长度", column.length); add("精度", column.precision); add("小数位数", column.scale); add("注释", column.comment);
          for (const fk of structure.foreign_keys.filter(fk => fk.columns.includes(object))) add(`外键 ${fk.name}`, `${fk.ref_table}(${fk.ref_columns.join(", ")})`);
        }
      }
    }
  } else if (type === "procedure" || type === "function") {
    add("所属连接", connection.name); add(`所属${namespaceName(connection)}`, schema);
    add("对象类型", labels[type]); add("连接类型", DB_KINDS.find(k => k.id === connection.kind)?.label ?? connection.kind);
    const definition = await read("定义", () => source.getRoutineDefinition(connection.id, schema, object, type));
    add("定义", definition);
    if (definition !== undefined) {
      add("源码行数", definition.length ? definition.split(/\r\n|\n|\r/).length : 0);
      add("源码字符数", Array.from(definition).length);
      if (!definition.trim()) add("状态", "数据源未返回对象定义");
    }
  } else if (type === "trigger" || type === "event" || type === "sequence") {
    // 当前 getRoutineDefinition 将非procedure一律当FUNCTION；不能把同名函数误当触发器/事件定义。
    const routines = await read("对象元数据", () => source.listRoutines(connection.id, schema));
    if (routines) add("元数据状态", routines.some(([t, name]) => t === type && name === object) ? "对象在元数据列表中" : "对象不在当前驱动提供的元数据列表中");
    add("定义", "当前详细定义接口不支持此对象类型，未读取定义");
  } else {
    add("状态", "此类对象暂无可用的详细元数据接口");
  }
  return state;
}

/** 选中后即可展示的加载态，实际详情仍须 await；不写入虚构属性。 */
export function objectDetailsLoading(connection: ConnectionConfig, schema: string, object: string, type: ObjectType): InfoPanelState {
  const fields: InfoField[] = [{ label: "状态", value: "正在读取详细信息…" }];
  return { icon: type === "connection" ? `info_pane_conn_${connection.kind}` : `info_pane_${type}`, title: object || schema || connection.name, subtitle: type === "schema" ? "模式" : type === "database" ? namespaceName(connection) : labels[type] ?? type, fields };
}
