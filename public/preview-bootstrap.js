try {
      // 在页面加载任何模块之前安装一个 Tauri IPC 桩，使前端可在浏览器中预览。
      // 仅用于开发期的视觉核对，不参与打包产物。
      (function () {
        // 与参考截图一致的数据（Snipaste_2026-11-02_19-27-43.png 树与对象列表）
        const DBSWITCH_TABLES = [
          ["dbswitch_assignment_config", 1, 1, "任务配置表"],
          ["dbswitch_assignment_job", 0, 0, "JOB信息表"],
          ["dbswitch_assignment_task", 0, 0, "任务信息表"],
          ["dbswitch_blob_triggers", 0, 0, "TRIGGER作为BLOB类型存储"],
          ["dbswitch_calendars", 0, 0, "以BLOB类型存储QUARTZ的日历"],
          ["dbswitch_cron_triggers", 0, 0, "存储CRON类型的触发器"],
          ["dbswitch_database_connection", 2, 2, "数据库连接表"],
          ["dbswitch_fired_triggers", 0, 0, "存储与已触发的TRIGGER相关的状态"],
          ["dbswitch_job_details", 0, 0, "存储每一个已配置的JOB的详细信息"],
          ["dbswitch_job_logback", 0, 0, "JOB执行日志"],
          ["dbswitch_locks", 0, 0, "存储程序的非观察锁"],
          ["dbswitch_paused_trigger_grps", 0, 0, "存储已暂停的TRIGGER组的信息"],
          ["dbswitch_scheduler_state", 0, 0, "存储所有的与SCHEDULER相关的状态"],
          ["dbswitch_schema_history", 0, 5, "Flyway表"],
          ["dbswitch_simple_triggers", 0, 0, "存储已配置的SIMPLE类型的触发器"],
          ["dbswitch_simprop_triggers", 0, 0, "存储CalendarInterval类型的触发器"],
          ["dbswitch_system_log", 3, 2, "系统日志"],
          ["dbswitch_system_user", 1, 0, "系统用户表"],
          ["dbswitch_triggers", 0, 0, "存储已配置的TRIGGER的信息"],
        ];
        const PM_TABLES = [
          ["account", 0, 15, ""],
          ["note_category", 0, 4, ""],
          ["note_item", 0, 0, ""],
          ["password_library", 0, 0, ""],
          ["persistent_logins", 0, 0, ""],
          ["quick_link", 0, 0, ""],
          ["sys_user", 0, 0, ""],
          ["todo_item", 0, 0, ""],
        ];
        // 备份仓库（内存态）
        const backups = {
          "c1/dbswitch": [
            { name: "dbswitch_20261102_093000.sql", modified: "2026-11-02 09:30:12", size: 48213, comment: "上线前全量备份" },
          ],
        };
        let backupSeq = 1;

        // 收藏的查询（内存态，供保存查询 / 查询分组使用）
        let favoritesStore = [
          { id: "f1", name: "查询 1", sql: "SELECT * FROM customers;", connection_id: "c2", timestamp: 0 },
          { id: "f2", name: "统计订单", sql: "SELECT COUNT(*) FROM orders;", connection_id: "c2", timestamp: 0 },
        ];

        // 自动运行任务（内存态）
        let storeJobs = [
          {
            id: "job-1",
            name: "日志清理",
            connection_id: "c1",
            database: "dbswitch",
            sql: "DELETE FROM `dbswitch_system_log` WHERE created_at < NOW() - INTERVAL 30 DAY",
            interval_secs: 3600,
            enabled: true,
            last_run: "2026-11-03 07:00:00",
            last_status: "成功",
          },
        ];
        const jobHistory = [
          { job_id: "job-1", job_name: "日志清理", started_at: "2026-11-03 07:00:00", elapsed_ms: 42, ok: true, detail: "3 行受影响" },
        ];

        // 用户列表（内存态）
        const users = [
          ["mysql.infoschema@localhost", "", "0", "0", "0", "0", "否"],
          ["mysql.session@localhost", "", "0", "0", "0", "0", "否"],
          ["mysql.sys@localhost", "", "0", "0", "0", "0", "否"],
          ["root@localhost", "", "0", "0", "0", "0", "是"],
        ];

        const store = {
          settings: {
            font_family: "Microsoft YaHei UI",
            font_size: 12,
            grid_font_size: 12,
            rows_per_page: 1000,
            query_timeout_secs: 30,
            theme: "light",
            show_system_tray: true,
            confirm_on_close: true,
            auto_complete: true,
            max_recent: 10,
            sql_font_family: "Consolas",
            sql_font_size: 13.6,
          },
          connections: [
            {
              id: "c1",
              name: "localhost",
              kind: "mysql",
              host: "127.0.0.1",
              port: 3306,
              user: "root",
              password: "",
              database: "",
              params: "",
              group: "本地连接",
              read_only: false,
              keep_alive: true,
            },
            {
              id: "c2",
              name: "dev-mysql",
              kind: "mysql",
              host: "10.11.8.96",
              port: 3306,
              user: "root",
              password: "",
              database: "",
              params: "",
              group: "开发环境",
              read_only: false,
              keep_alive: true,
            },
            {
              id: "c3",
              name: "dev-sqlserver",
              kind: "sqlserver",
              host: "10.11.8.96,1436",
              port: 1436,
              user: "TrustService",
              password: "",
              database: "",
              params: "",
              group: "开发环境",
              read_only: false,
              keep_alive: true,
            },
          ],
        };
;

        // 仅预览界面状态夹具，不提供真实事务或数据库写入保证。
        const previewManualTransactions = new Map();
        const inactiveTransaction = () => ({ active: false, schema: "", owner: "", failed: false });
        const responses = {
          manual_transaction_status: args => previewManualTransactions.get(args.id) || inactiveTransaction(),
          manual_transaction_start: args => {
            if (previewManualTransactions.has(args.id)) throw new Error("此连接已有事务");
            const status = { active: true, schema: args.schema, owner: args.owner, failed: false };
            previewManualTransactions.set(args.id, status);
            return { ...status };
          },
          manual_transaction_finish: args => {
            const status = previewManualTransactions.get(args.id);
            if (!status || status.owner !== args.owner) throw new Error("事务所有者不匹配");
            if (args.commit && status.failed) throw new Error("失败事务必须回滚");
            previewManualTransactions.delete(args.id);
            return inactiveTransaction();
          },
          get_settings: () => store.settings,
          set_settings: () => null,
          list_connections: () => store.connections,
          list_groups: () => ["本地连接", "开发环境"],
          add_group: (args) => ["本地连接", "开发环境", args?.name ?? "新分组"],
          // 内存态真实增删：与后端同形（返回更新后的连接清单），保证预览里新连接出现在树上
          save_connection: (args) => {
            const cfg = args && args.config;
            if (cfg) {
              const i = store.connections.findIndex((c) => c.id === cfg.id);
              if (i >= 0) store.connections[i] = { ...store.connections[i], ...cfg };
              else store.connections.push({ ...cfg });
            }
            return store.connections.map((c) => ({ ...c }));
          },
          delete_connection: (args) => {
            store.connections = store.connections.filter((c) => c.id !== args?.id);
            return store.connections.map((c) => ({ ...c }));
          },
          rename_group: () => store.connections,
          test_connection: () => ({
            ok: true,
            message: "连接成功",
            version: "8.0.35",
            charset: "utf8mb4",
          }),
          // 新建/编辑连接对话框实际调用的入口（带保存密码语义），与 test_connection 同响应
          test_connection_with_saved_password: () => ({
            ok: true,
            message: "连接成功",
            version: "8.0.35",
            charset: "utf8mb4",
          }),
          // 驱动状态：内置协议直连；其余按“未找到本地驱动、可下载”呈现
          list_driver_statuses: () => [
            { kind: "mysql", version: "", status: "builtin", architecture: "", message: "内置 MySQL 协议直连，无需驱动", can_download: false, library: null, download_path: null },
            { kind: "mariadb", version: "", status: "builtin", architecture: "", message: "内置 MySQL 协议直连，无需驱动", can_download: false, library: null, download_path: null },
            { kind: "sqlserver", version: "", status: "builtin", architecture: "", message: "内置 TDS 协议直连，无需驱动", can_download: false, library: null, download_path: null },
            { kind: "postgresql", version: "", status: "builtin", architecture: "", message: "内置 PostgreSQL 协议直连，无需驱动", can_download: false, library: null, download_path: null },
            { kind: "sqlite", version: "", status: "builtin", architecture: "", message: "内置 SQLite 直连，无需驱动", can_download: false, library: null, download_path: null },
            { kind: "oracle", version: "", status: "missing", architecture: "x64", message: "未找到本地 Oracle 驱动；可从官方源下载", can_download: true, library: null, download_path: null },
          ],
          open_connection: () => ({
            ok: true,
            message: "连接成功",
            version: "8.0.35",
            charset: "utf8mb4",
          }),
          // 树展开（连接 / 数据库节点）依赖这两个命令，预览必须给出成功响应
          ensure_connection: () => ({
            ok: true,
            message: "连接成功",
            version: "8.0.35",
            charset: "utf8mb4",
          }),
          open_database: () => ({
            ok: true,
            message: "数据库已打开",
            version: "8.0.35",
            charset: "utf8mb4",
          }),
          close_connection: args => { previewManualTransactions.delete(args.id); return null; },
          close_all_connections: () => { previewManualTransactions.clear(); return null; },
          list_databases: (args) => {
            const id = args && args.id;
            if (id === "c3") return [];
            return [
              ["dbswitch", "database"],
              ["information_schema", "system"],
              ["mysql", "system"],
              ["password_manager", "database"],
              ["performance_schema", "system"],
              ["springbatch", "system"],
              ["sys", "system"],
              ["trustproduct", "database"],
            ];
          },
          list_tables: (args) => {
            const id = args && args.id;
            const schema = (args && args.schemaName) || "";
            if (schema === "dbswitch") {
              return [...DBSWITCH_TABLES.map(([n]) => [n, "table"]), ["v_job_summary", "view"]];
            }
            if (schema === "password_manager") return PM_TABLES.map(([n]) => [n, "table"]);
            return [];
          },
          list_routines: (args) => {
            const schema = (args && args.schemaName) || "";
            if (schema === "dbswitch") {
              return [["function", "fn_next_seq"], ["procedure", "sp_cleanup_logs"]];
            }
            return [];
          },
          execute_sql: (args) => {
            const sql = String((args && args.sql) || "");
            if (/note_category/i.test(sql)) {
              return {
                sets: [
                  {
                    columns: [
                      { name: "id", type: "bigint", nullable: true, primary_key: false, auto_increment: false },
                      { name: "created_at", type: "datetime", nullable: true, primary_key: false, auto_increment: false },
                      { name: "name", type: "varchar", nullable: true, primary_key: false, auto_increment: false },
                      { name: "quick_link_id", type: "bigint", nullable: true, primary_key: false, auto_increment: false },
                      { name: "sort_order", type: "int", nullable: true, primary_key: false, auto_increment: false },
                      { name: "updated_at", type: "datetime", nullable: true, primary_key: false, auto_increment: false },
                      { name: "user_id", type: "bigint", nullable: true, primary_key: false, auto_increment: false },
                    ],
                    rows: [
                      ["1", "2026-05-12 09:20:50.42", "默认", "705", "3", "2026-05-12 10:29:39.18", "1"],
                      ["2", "2026-05-12 09:21:14.24", "代码", "705", "1", "2026-05-12 10:29:39.18", "1"],
                      ["3", "2026-05-12 09:21:32.11", "权益", "705", "0", "2026-05-12 10:29:39.18", "1"],
                      ["33", "2026-05-12 10:11:31.82", "测试", "705", "2", "2026-05-12 10:29:39.18", "1"],
                    ],
                    total: 4,
                    affected: 0,
                  },
                ],
                messages: [],
                elapsed_ms: 251,
              };
            }
            return { sets: [], messages: [], elapsed_ms: 1 };
          },
          list_objects: (args) => {
            const schema = (args && args.schemaName) || "";
            if (schema === "dbswitch") {
              return [
                ...DBSWITCH_TABLES.map(([n, ai, rows, comment]) => ({
                  name: n,
                  type: "table",
                  autoIncrement: ai,
                  updateTime: "",
                  dataLength: 16384,
                  engine: "InnoDB",
                  rows: rows,
                  comment: comment,
                })),
                {
                  name: "v_job_summary",
                  type: "view",
                  autoIncrement: 0,
                  updateTime: "",
                  dataLength: 0,
                  engine: "InnoDB",
                  rows: 4,
                  comment: "任务汇总视图",
                },
              ];
            }
            if (schema === "password_manager") {
              return PM_TABLES.map(([n]) => ({
                name: n,
                type: "table",
                autoIncrement: 0,
                updateTime: "",
                dataLength: 16384,
                engine: "InnoDB",
                rows: 0,
                comment: "",
              }));
            }
            return [];
          },
          get_table_schema: (args) => ({
            table: (args && args.table) || "account",
            database: (args && args.schemaName) || "password_manager",
            columns: [
              { name: "id", type: "bigint", nullable: false, default: null, primary_key: true, auto_increment: true, comment: null },
              { name: "account_name", type: "varchar", nullable: false, default: null, primary_key: false, auto_increment: false, comment: null },
              { name: "category", type: "varchar", nullable: true, default: null, primary_key: false, auto_increment: false, comment: null },
              { name: "created_at", type: "datetime", nullable: true, default: null, primary_key: false, auto_increment: false, comment: null },
              { name: "description", type: "longtext", nullable: true, default: null, primary_key: false, auto_increment: false, comment: null },
              { name: "icon", type: "longtext", nullable: true, default: null, primary_key: false, auto_increment: false, comment: null },
              { name: "password", type: "varchar", nullable: true, default: null, primary_key: false, auto_increment: false, comment: null },
              { name: "password_library_id", type: "bigint", nullable: true, default: null, primary_key: false, auto_increment: false, comment: null },
              { name: "updated_at", type: "datetime", nullable: true, default: null, primary_key: false, auto_increment: false, comment: null },
              { name: "user_id", type: "bigint", nullable: false, default: null, primary_key: false, auto_increment: false, comment: null },
              { name: "username", type: "varchar", nullable: true, default: null, primary_key: false, auto_increment: false, comment: null },
              { name: "website", type: "varchar", nullable: true, default: null, primary_key: false, auto_increment: false, comment: null },
            ],
            indexes: [
              { name: "PRIMARY", columns: ["id"], unique: true, primary: true, method: null },
            ],
            foreign_keys: [],
            comment: null,
            engine: "InnoDB",
            collation: "utf8mb4_0900_ai_ci",
            row_count: 15,
          }),
          // 表数据分页（预览 mock）：在固定行集上真实执行筛选与排序，
          // 请求结构与后端一致（filters 分组 OR、组间按 filter_logic 连接；order_rules 依次优先）。
          page_table: (args) => {
            const req = (args && args.req) || args || {};
            const cols = [{ name: "id", type: "bigint", nullable: true, primary_key: false, auto_increment: false },
                      { name: "created_at", type: "datetime", nullable: true, primary_key: false, auto_increment: false },
                      { name: "name", type: "varchar", nullable: true, primary_key: false, auto_increment: false },
                      { name: "quick_link_id", type: "bigint", nullable: true, primary_key: false, auto_increment: false },
                      { name: "sort_order", type: "int", nullable: true, primary_key: false, auto_increment: false },
                      { name: "updated_at", type: "datetime", nullable: true, primary_key: false, auto_increment: false },
                      { name: "user_id", type: "bigint", nullable: true, primary_key: false, auto_increment: false }];
            let rows = [["1", "2026-05-12 09:20:50.42", "默认", "705", "3", "2026-05-12 10:29:39.18", "1"],
                      ["2", "2026-05-12 09:21:14.24", "代码", "705", "1", "2026-05-12 10:29:39.18", "1"],
                      ["3", "2026-05-12 09:21:32.11", "权益", "705", "0", "2026-05-12 10:29:39.18", "1"],
                      ["33", "2026-05-12 10:11:31.82", "测试", "705", "2", "2026-05-12 10:29:39.18", "1"]];
            const colIdx = (n) => cols.findIndex((c) => c.name === n);
            const asNum = (s) => { const n = Number(s); return Number.isFinite(n) && String(s).trim() !== "" ? n : null; };
            // 筛选
            const filters = Array.isArray(req.filters) ? req.filters : [];
            if (filters.length) {
              const matches = (row, cond) => {
                const i = colIdx(cond.column);
                if (i < 0) return true;
                let a = row[i] === null ? "" : String(row[i]);
                let b = cond.value === null || cond.value === undefined ? "" : String(cond.value);
                if (!cond.case_sensitive) { a = a.toLowerCase(); b = b.toLowerCase(); }
                const op = String(cond.operator || "=").toLowerCase();
                if (op === "like") {
                  const rx = new RegExp("^" + b.replace(/[.*+?^${}()|[\]\\]/g, "\\$&").replace(/%/g, "[\\s\\S]*").replace(/_/g, "[\\s\\S]") + "$");
                  return rx.test(a);
                }
                const an = asNum(a), bn = asNum(b);
                const useNum = an !== null && bn !== null;
                const av = useNum ? an : a, bv = useNum ? bn : b;
                if (op === "=" || op === "==") return av === bv;
                if (op === "!=" || op === "<>") return av !== bv;
                if (op === ">") return av > bv;
                if (op === ">=") return av >= bv;
                if (op === "<") return av < bv;
                if (op === "<=") return av <= bv;
                return true;
              };
              const logic = String(req.filter_logic || "AND").toUpperCase();
              rows = rows.filter((row) => {
                const groups = filters.map((g) => (g.conds || []).some((c) => matches(row, c)));
                return logic === "OR" ? groups.some(Boolean) : groups.every(Boolean);
              });
            }
            // 排序（NULL 排最后；数值列按数值比较）
            const rules = Array.isArray(req.order_rules) ? req.order_rules : [];
            const keys = rules
              .map((r) => ({ i: colIdx(r.column), dir: r.ascending === false ? -1 : 1 }))
              .filter((k) => k.i >= 0);
            if (keys.length) {
              rows = [...rows].sort((ra, rb) => {
                for (const k of keys) {
                  const a = ra[k.i], b = rb[k.i];
                  if (a === null && b === null) continue;
                  if (a === null) return 1;
                  if (b === null) return -1;
                  const an = asNum(a), bn = asNum(b);
                  let cmp;
                  if (an !== null && bn !== null && an !== bn) cmp = an < bn ? -1 : 1;
                  else if (String(a) === String(b)) continue;
                  else cmp = String(a) < String(b) ? -1 : 1;
                  if (cmp) return cmp * k.dir;
                }
                return 0;
              });
            }
            return { columns: cols, rows, total: rows.length, primary_keys: ["id"], elapsed_ms: 12 };
          },
          execute_sql: (args) => {
            const sql2 = String((args && args.sql) || "");
            if (/note_category/i.test(sql2)) {
              return {
                sets: [
                  {
                    columns: [{ name: "id", type: "bigint", nullable: true, primary_key: false, auto_increment: false },
                      { name: "created_at", type: "datetime", nullable: true, primary_key: false, auto_increment: false },
                      { name: "name", type: "varchar", nullable: true, primary_key: false, auto_increment: false },
                      { name: "quick_link_id", type: "bigint", nullable: true, primary_key: false, auto_increment: false },
                      { name: "sort_order", type: "int", nullable: true, primary_key: false, auto_increment: false },
                      { name: "updated_at", type: "datetime", nullable: true, primary_key: false, auto_increment: false },
                      { name: "user_id", type: "bigint", nullable: true, primary_key: false, auto_increment: false }],
                    rows: [["1", "2026-05-12 09:20:50.42", "默认", "705", "3", "2026-05-12 10:29:39.18", "1"],
                      ["2", "2026-05-12 09:21:14.24", "代码", "705", "1", "2026-05-12 10:29:39.18", "1"],
                      ["3", "2026-05-12 09:21:32.11", "权益", "705", "0", "2026-05-12 10:29:39.18", "1"],
                      ["33", "2026-05-12 10:11:31.82", "测试", "705", "2", "2026-05-12 10:29:39.18", "1"]],
                    total: 4,
                    affected: 0,
                  },
                ],
                messages: [],
                elapsed_ms: 251,
              };
            }
            return { sets: [], messages: [], elapsed_ms: 1 };
          },
          update_row: () => 1,
          insert_row: () => 1,
          delete_row: () => 1,
          execute_ddl: () => 0,
          list_favorites: () => favoritesStore,
          add_favorite: (a) => {
            // 预览环境内存态收藏（与真实后端同形：返回更新后的清单）
            const fav = a && a.favorite;
            if (!fav) return [];
            favoritesStore = favoritesStore.filter((f) => f.id !== fav.id);
            favoritesStore.unshift(fav);
            return favoritesStore;
          },
          remove_favorite: (a) => {
            favoritesStore = favoritesStore.filter((f) => f.id !== a.id);
            return favoritesStore;
          },
          list_recent: () => [
            { id: "c2:demo.customers", label: "MySQL 服务器 / customers", kind: "table", timestamp: 0 },
          ],
          push_recent: () => [],
          clear_recent: () => [],
          run_import: () => 10,
          run_export: () => 10,
          export_query_result: () => 10,
          preview_csv: () => [["id", "name"], ["1", "abc"]],
          run_transfer: () => 10,
          run_dump: () => 10,
          // ---- 数据库同步（预览 mock）----
          sync_type_mapping: () => [
            { source: "id", source_type: "bigint", target_type: "BIGINT" },
            { source: "account_name", source_type: "varchar", target_type: "VARCHAR(255)" },
            { source: "category", source_type: "varchar", target_type: "VARCHAR(255)" },
            { source: "created_at", source_type: "datetime", target_type: "TIMESTAMP" },
            { source: "description", source_type: "longtext", target_type: "TEXT" },
            { source: "icon", source_type: "longtext", target_type: "TEXT" },
            { source: "password", source_type: "varchar", target_type: "VARCHAR(255)" },
            { source: "password_library_id", source_type: "bigint", target_type: "BIGINT" },
            { source: "updated_at", source_type: "datetime", target_type: "TIMESTAMP" },
            { source: "user_id", source_type: "bigint", target_type: "BIGINT" },
            { source: "username", source_type: "varchar", target_type: "VARCHAR(255)" },
            { source: "website", source_type: "varchar", target_type: "VARCHAR(255)" },
          ],
          run_sync: (args) => ({
            rows: 15,
            created: Boolean(args && args.opts && args.opts.create_target),
            warnings: [],
          }),
          // ---- 备份 / 还原（内存仓库，模拟 data/backups/<conn>/<db>/）----
          list_backups: (args) => {
            const key = `${(args && args.connectionId) || ""}/${(args && args.schemaName) || ""}`;
            const list = backups[key] ?? [];
            return list.map((b) => ({ ...b }));
          },
          create_backup: (args) => {
            const opts = (args && args.opts) || {};
            const key = `${opts.connection_id}/${opts.schema}`;
            const d = new Date();
            const p = (n) => String(n).padStart(2, "0");
            const stamp = `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}_${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
            const size = 18000 + backupSeq * 5237;
            (backups[key] ??= []).push({
              name: `${opts.schema}_${stamp}.sql`,
              modified: `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`,
              size,
              comment: opts.comment || "",
            });
            backupSeq += 1;
            return [opts.include_data ? 137 : 0, size];
          },
          restore_backup: (args) => {
            const key = `${(args && args.connectionId) || ""}/${(args && args.schemaName) || ""}`;
            const list = backups[key] ?? [];
            if (!list.some((b) => b.name === args.file)) {
              return Promise.reject(new Error("备份文件不存在"));
            }
            return 42;
          },
          // 详细版还原（预览 mock）：给出贴近真实的逐条执行报告
          restore_backup_detailed: (args) => {
            const key = `${(args && args.connectionId) || ""}/${(args && args.schemaName) || ""}`;
            const list = backups[key] ?? [];
            if (!list.some((b) => b.name === args.file)) {
              return Promise.reject(new Error("备份文件不存在"));
            }
            const stmts = [
              "SET FOREIGN_KEY_CHECKS = 0",
              "DROP TABLE IF EXISTS `dbswitch_assignment_config`",
              "CREATE TABLE `dbswitch_assignment_config` (\n  `id` bigint NOT NULL AUTO_INCREMENT,\n  `account_name` varchar(64) DEFAULT NULL,\n  PRIMARY KEY (`id`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
              "INSERT INTO `dbswitch_assignment_config` VALUES (1,'默认'),(2,'代码'),(3,'权益')",
              "INSERT INTO `dbswitch_assignment_config` VALUES (33,'测试')",
              "SET FOREIGN_KEY_CHECKS = 1",
            ];
            const steps = stmts.map((sql, i) => ({
              index: i + 1,
              sql,
              ok: true,
              affected: sql.startsWith("INSERT") ? (i === 3 ? 3 : 1) : 0,
              error: null,
              skipped: false,
            }));
            return { total: steps.length, succeeded: steps.length, failed: 0, steps };
          },
          delete_backup: (args) => {
            const key = `${(args && args.connectionId) || ""}/${(args && args.schemaName) || ""}`;
            backups[key] = (backups[key] ?? []).filter((b) => b.name !== args.file);
          },
          open_backup_folder: () => "data/backups/c1/dbswitch",
          list_db_users: (args) => {
            if ((args && args.id) !== "c1") return [];
            return users;
          },
          // ---- 表信息 / DDL ----
          table_status: (args) => {
            const t = (args && args.table) || "";
            return [
              ["Rows", "99847"],
              ["Engine", "InnoDB"],
              ["Auto_increment", "0"],
              ["Row_format", "Dynamic"],
              ["Update_time", "2026-11-05 07:42:56"],
              ["Create_time", "2026-11-05 07:42:55"],
              ["Check_time", ""],
              ["Index_length", "3686400"],
              ["Data_length", "5783552"],
              ["Max_data_length", "0"],
              ["Data_free", "4194304"],
              ["Collation", "utf8mb4_0900_ai_ci"],
              ["Create_options", ""],
              ["Comment", ""],
            ].map(([k, v]) => [k, k === "Comment" && !v ? "" : v]);
          },
          get_table_ddl: (args) => {
            const t = (args && args.table) || "demo";
            return (
              "CREATE TABLE `" + t + "` (\n" +
              "  `id` int NOT NULL,\n" +
              "  `name` varchar(64) DEFAULT NULL,\n" +
              "  `created_at` datetime DEFAULT NULL,\n" +
              "  `amount` decimal(18,4) DEFAULT NULL,\n" +
              "  PRIMARY KEY (`id`),\n" +
              "  KEY `idx_big_name` (`name`)\n" +
              ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"
            );
          },
          // ---- 视图 / 函数定义 ----
          get_view_definition: (args) => {
            if ((args && args.view) === "v_job_summary") {
              return "CREATE ALGORITHM=UNDEFINED VIEW `v_job_summary` AS SELECT job_name, COUNT(*) AS cnt FROM `dbswitch_job_details` GROUP BY job_name";
            }
            return Promise.reject(new Error("视图不存在"));
          },
          get_routine_definition: (args) => {
            if ((args && args.name) === "sp_cleanup_logs") {
              return "CREATE PROCEDURE `sp_cleanup_logs`()\nBEGIN\n  DELETE FROM `dbswitch_system_log` WHERE created_at < NOW() - INTERVAL 30 DAY;\nEND";
            }
            if ((args && args.name) === "fn_next_seq") {
              return "CREATE FUNCTION `fn_next_seq`(p_name VARCHAR(64)) RETURNS INT\nBEGIN\n  DECLARE v INT;\n  SET v = 1;\n  RETURN v;\nEND";
            }
            return Promise.reject(new Error("对象不存在"));
          },
          // ---- 自动运行 ----
          list_jobs: () => storeJobs,
          save_job: (args) => {
            const job = (args && args.job) || {};
            if (!job.id) job.id = "job-" + (storeJobs.length + 1);
            const i = storeJobs.findIndex((j) => j.id === job.id);
            if (i >= 0) storeJobs[i] = job; else storeJobs.push(job);
            return storeJobs;
          },
          delete_job: (args) => {
            storeJobs = storeJobs.filter((j) => j.id !== args.id);
            return storeJobs;
          },
          list_job_history: () => jobHistory,
          run_job_now: (args) => {
            const job = storeJobs.find((j) => j.id === args.id);
            if (!job) return Promise.reject(new Error("任务不存在"));
            const started = new Date().toISOString().slice(0, 19).replace("T", " ");
            jobHistory.unshift({
              job_id: job.id, job_name: job.name, started_at: started,
              elapsed_ms: 36, ok: true, detail: "结果集 4 行",
            });
            job.last_run = started;
            job.last_status = "成功";
            return [36, "结果集 4 行"];
          },
          // ---- 用户管理 ----
          create_db_user: (args) => {
            users.push([`${args.user}@${args.host || "%"}`, "", "0", "0", "0", "0", "否"]);
          },
          drop_db_user: (args) => {
            const name = `${args.user}@${args.host}`;
            const i = users.findIndex((u) => u[0] === name);
            if (i >= 0) users.splice(i, 1);
          },
          alter_db_user_password: () => {},
          server_info: () => ({
            product: "MySQL",
            version: "8.0.35-28",
            host: "127.0.0.1",
            port: 3306,
            charset: "utf8mb4",
            collation: "utf8mb4_general_ci",
            uptime: "12345",
            extra: [["version", "8.0.35-28"]],
          }),
          completion_data: () => ({
            __tables__: ["customers", "orders", "products"],
            customers: ["id", "name", "email", "balance"],
          }),
          read_log: () => ["[2024-03-01] app 已启动"],
          // ---- 数据源同步（预览 mock：可持久化的空配置，清除状态栏告警并让对话框可用）----
          datasource_sync_get_config: () => ({ ...previewSyncConfig }),
          datasource_sync_set_config: (args) => {
            const c = args?.config ?? {};
            previewSyncConfig = {
              url: c.url ?? "", user: c.user ?? "", auto_every: c.auto_every ?? -1,
              remember_password: c.remember_password ?? true,
              password_set: !!c.password, passphrase_set: !!c.passphrase,
              headers_set: !!c.headers, last_sync: null,
            };
            return { ...previewSyncConfig };
          },
          datasource_sync_test: () => "配置格式有效（预览模拟，不发起真实同步）",
          datasource_sync_preview: () => ({
            plan_id: "preview-plan",
            conflicts: [],
            summary: { added: 0, updated: 0, deleted: 0, organization: false },
            warnings: [], remote_exists: false, has_changes: false, requires_confirmation: false,
          }),
          datasource_sync_commit: () => ({
            connections: store.connections.map((c) => ({ ...c })),
            groups: [], changed_ids: [], deleted_ids: [], warnings: [], last_sync: "",
          }),
          datasource_sync_cancel: () => null,
          // ---- SQL 执行日志（预览 mock：两条样例，验证历史日志过滤/清空交互）----
          list_sql_log: () => [
            { ts: "2026-11-07 12:00:01", connection_id: "c1", connection_name: "localhost", kind: "SELECT", database: "password_manager", sql: "SELECT * FROM note_category LIMIT 0,1000", ok: true, elapsed_ms: 12, rows: 4, message: "", origin: "grid" },
            { ts: "2026-11-07 12:00:15", connection_id: "c1", connection_name: "localhost", kind: "SELECT", database: "dbswitch", sql: "SELECT * FROM dbswitch_system_log", ok: false, elapsed_ms: 251, rows: 0, message: "Table 'dbswitch.dbswitch_system_log' doesn't exist", origin: "query" },
          ],
          clear_sql_log: () => null,
          about_info: () => ({
            product: "NavKit",
            version: "1.0.0",
            copyright: "Copyright © 2026 NavKit Contributors",
            publisher: "NavKit",
            url: "",
            support: "",
            dataDir: "data",
          }),
        };

        // 数据源同步配置的预览内存态（set_config 写入、get_config 读回）。
        let previewSyncConfig = {
          url: "", user: "", auto_every: -1, remember_password: true,
          password_set: false, passphrase_set: false, headers_set: false, last_sync: null,
        };

        // 预览 mock：api.ts 的 call() 优先读取 __PREVIEW_MOCK__，
        // 纯浏览器环境下没有真实 IPC，再补一个最小 internals 以免 @tauri-apps/api 报错。
        window.__PREVIEW_MOCK__ = responses;
        if (!window.__TAURI_INTERNALS__) {
          window.__TAURI_INTERNALS__ = {
            invoke: function (cmd, args) {
              const fn = responses[cmd];
              if (fn) return Promise.resolve(fn(args));
              return Promise.reject(new Error("未实现的命令: " + cmd));
            },
            transformCallback: function (cb) {
              return cb;
            },
            metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
          };
        }
        window.__TAURI__ = window.__TAURI__ || { event: { listen: function () { return Promise.resolve(() => {}); } } };
      })();

// 自动演示：展开树到参考截图 1 的状态（本地连接 > localhost > dbswitch(选中,表组展开) + 对象页）
(function () {
  const click = (el) => el && el.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  const nodes = () => [...document.querySelectorAll(".tree-node")];
  const byText = (t) => nodes().find((n) => n.textContent.includes(t));
  const byExact = (t) => nodes().find((n) => n.textContent.trim() === t);
  (async () => {
    for (let i = 0; i < 40; i++) {
      if (nodes().length) break;
      await wait(150);
    }
    await wait(200);
    click(nodes()[0]?.querySelector(".tree-arrow"));
    await wait(300);
    click(byText("localhost")?.querySelector(".tree-arrow"));
    await wait(500);
    click(byText("dbswitch")?.querySelector(".tree-arrow"));
    await wait(500);
    click(byExact("表")?.querySelector(".tree-arrow"));
    await wait(300);
    click(byText("dbswitch"));
    await wait(700);
    const app = window.__APP__;
    if (app && app.tree) app.tree.render();
    const page = new URLSearchParams(location.search).get("page") || "query";
    if (page === "query") {
      if (app && app.openTableTab) {
        await app.openTableTab("c1", "dbswitch", "dbswitch_assignment_config", "table");
        await wait(300);
        await app.openTableTab("c1", "dbswitch", "dbswitch_calendars", "table");
        await wait(300);
        await app.openTableTab("c1", "password_manager", "note_category", "table");
        await wait(400);
      }
      if (app && app.openQueryTab && app.getView) {
        await app.openQueryTab("c1", "SELECT * FROM note_category");
        await wait(300);
        const view = app.getView(app.activeTabId);
        if (view && view.reload) await view.reload();
      }
    } else if (page === "designer") {
      if (app && app.openStructureTab) {
        await app.openStructureTab("c1", "password_manager", "account");
        await wait(500);
      }
    } else if (page === "backup") {
      if (app && app.openSpecialTab) {
        await app.openSpecialTab("backup", "c1", "password_manager");
        await wait(500);
      }
    } else if (page === "users") {
      if (app && app.openSpecialTab) {
        await app.openSpecialTab("users", "c1", "password_manager");
        await wait(500);
      }
    } else if (app && app.openQueryTab && app.getView) {
      await app.openQueryTab("c1", "SELECT * FROM note_category");
      await wait(300);
      const view = app.getView(app.activeTabId);
      if (view && view.reload) await view.reload();
    }
  })();
})();
} catch (e) {
}
