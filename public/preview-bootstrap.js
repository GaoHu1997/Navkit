try {
      // 在页面加载任何模块之前安装一个 Tauri IPC 桩，使前端可在浏览器中预览。
      // 仅用于开发期的视觉核对，不参与打包产物。
      (function () {
        // 与参考截图一致的数据（Snipaste_2026-11-02_19-27-43.png 树与对象列表）
        const NAVKIT_TABLES = [
          ["navkit_assignment_config", 1, 1, "任务配置表"],
          ["navkit_assignment_job", 0, 0, "JOB信息表"],
          ["navkit_assignment_task", 0, 0, "任务信息表"],
          ["navkit_blob_triggers", 0, 0, "TRIGGER作为BLOB类型存储"],
          ["navkit_calendars", 0, 0, "以BLOB类型存储QUARTZ的日历"],
          ["navkit_cron_triggers", 0, 0, "存储CRON类型的触发器"],
          ["navkit_database_connection", 2, 2, "数据库连接表"],
          ["navkit_fired_triggers", 0, 0, "存储与已触发的TRIGGER相关的状态"],
          ["navkit_job_details", 0, 0, "存储每一个已配置的JOB的详细信息"],
          ["navkit_job_logback", 0, 0, "JOB执行日志"],
          ["navkit_locks", 0, 0, "存储程序的非观察锁"],
          ["navkit_paused_trigger_grps", 0, 0, "存储已暂停的TRIGGER组的信息"],
          ["navkit_scheduler_state", 0, 0, "存储所有的与SCHEDULER相关的状态"],
          ["navkit_schema_history", 0, 5, "Flyway表"],
          ["navkit_simple_triggers", 0, 0, "存储已配置的SIMPLE类型的触发器"],
          ["navkit_simprop_triggers", 0, 0, "存储CalendarInterval类型的触发器"],
          ["navkit_system_log", 3, 2, "系统日志"],
          ["navkit_system_user", 1, 0, "系统用户表"],
          ["navkit_triggers", 0, 0, "存储已配置的TRIGGER的信息"],
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
          "c1/navkit": [
            { name: "navkit_20261102_093000.sql", modified: "2026-11-02 09:30:12", size: 48213, comment: "上线前全量备份" },
          ],
        };
        let backupSeq = 1;

        // 收藏的查询（内存态，供保存查询 / 查询分组使用）
        let favoritesStore = [
          { id: "f1", name: "查询 1", sql: "SELECT * FROM customers;", connection_id: "c2", timestamp: 0 },
          { id: "f2", name: "统计订单", sql: "SELECT COUNT(*) FROM orders;", connection_id: "c2", timestamp: 0 },
        ];

        // 自动任务（内存态）
        let storeJobs = [
          {
            id: "job-1",
            name: "日志清理",
            connection_id: "c1",
            database: "navkit",
            sql: "DELETE FROM `navkit_system_log` WHERE created_at < NOW() - INTERVAL 30 DAY",
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

        // ---- mock 公共工具：稳定 id / 指纹 / 时间 / 表结构夹具 ----
        let mockSeq = 0;
        const uuid = () => "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
          const r = (Math.random() * 16) | 0;
          return (c === "x" ? r : (r & 0x3) | 0x8).toString(16);
        });
        const fingerprint = () => Array.from({ length: 64 }, () => "0123456789abcdef"[(Math.random() * 16) | 0]).join("");
        const nowIso = () => new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
        const isoPlus = (ms) => new Date(Date.now() + ms).toISOString().replace(/\.\d{3}Z$/, "Z");
        const deadline = (id) => ({ execution_id: id, budget_ms: null, elapsed_ms: 0, deadline_reached: false, stop_confirmed: false });
        const outcome = (id, extra) => ({
          execution_id: id, state: "succeeded", result: null, error: null, deadline: deadline(id),
          effects: "preview_fixture", transaction_state: "committed", ...(extra || {}),
        });
        // 最近一次 execute_sql 的结果集（按 连接|库 记录），供查询证据/缓存分页复用
        const spoolStore = new Map();
        // 与 get_table_schema 同源的表结构夹具；表设计快照复用同一份结构
        const tableSchemaFixture = (schemaName, tableName) => ({
          table: tableName || "account",
          database: schemaName || "password_manager",
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
        });
        const tableDesignCapabilities = () => ({
          checks: { supported: true, reason: null },
          triggers: { supported: true, reason: null },
          options: { supported: true, reason: null },
          rebuild: { supported: true, reason: null },
          clear: { supported: true, reason: null },
          copy_structure: { supported: true, reason: null },
          copy_data: { supported: true, reason: null },
          supported_options: ["engine", "charset", "collation", "row_format"],
          limitations: [],
        });
        // 由 TableDesignSnapshot.schema 组装设计 payload（编辑器全页草稿基线）
        const designPayloadOf = (schema) => ({
          schema: schema.database, table: schema.table,
          columns: schema.columns.map((c) => ({
            name: c.name, data_type: c.type, nullable: c.nullable, default_value: c.default,
            auto_increment: c.auto_increment, comment: c.comment,
          })),
          primary_key: schema.columns.filter((c) => c.primary_key).map((c) => c.name),
          comment: schema.comment,
          indexes: schema.indexes.filter((i) => !i.primary).map((i) => ({ name: i.name, columns: i.columns, unique: !!i.unique })),
          foreign_keys: [],
          checks: [],
          triggers: [],
          options: { engine: schema.engine, charset: "utf8mb4", collation: schema.collation },
        });
        const snapshotOfDesign = (args, ownerId) => {
          const design = (args && args.design) || {};
          const schema = tableSchemaFixture(design.schema, design.table);
          if (Array.isArray(design.columns)) {
            schema.columns = design.columns.map((c) => ({
              name: c.name, type: c.data_type || "text", nullable: c.nullable !== false,
              default: c.default_value ?? null, primary_key: (design.primary_key || []).includes(c.name),
              auto_increment: !!c.auto_increment, comment: c.comment ?? null,
            }));
          }
          if (Array.isArray(design.indexes)) {
            schema.indexes = [
              ...(design.primary_key || []).length ? [{ name: "PRIMARY", columns: design.primary_key, unique: true, primary: true, method: null }] : [],
              ...design.indexes.map((i) => ({ name: i.name, columns: i.columns, unique: !!i.unique, primary: false, method: null })),
            ];
          }
          if (design.comment !== undefined) schema.comment = design.comment;
          return {
            schema, objects: { checks: [], triggers: [], completeness: "complete", limitations: [], table_definition: null },
            options: { engine: schema.engine, charset: "utf8mb4", collation: schema.collation },
            design, revision: "preview-rev-0001", source_fingerprint: fingerprint(),
            owner_id: ownerId || "", completeness: "complete", capabilities: tableDesignCapabilities(),
          };
        };
        // 内存态调度任务 / 模板（scheduler_* 命令）
        let schedulerJobs = [];
        let schedulerTemplates = [];
        const schedulerRecord = (job) => ({
          job, schedule_revision: job.revision || "rev-1",
          next_run_utc: job.enabled ? isoPlus(60000) : null,
          queued: null, blocked_run_id: null, paused_reason: null,
        });
        const runSummary = (job, status) => ({
          id: uuid(), job_id: job ? job.id : uuid(), job_revision: (job && job.revision) || "rev-1",
          schedule_revision: "rev-1", scheduled_at_utc: nowIso(), claimed_at_utc: nowIso(),
          started_at_utc: nowIso(), completed_at_utc: status === "running" ? null : nowIso(),
          status: status || "succeeded", resources_pending: false, cancel_requested: false,
          actions: [], journal: [],
        });

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
              ["navkit", "database"],
              ["information_schema", "system"],
              ["mysql", "system"],
              ["password_manager", "database"],
              ["performance_schema", "system"],
              ["springbatch", "system"],
              ["sys", "system"],
              ["trustproduct", "database"],
            ];
          },
          table_option_choices: () => ({
            engines: ["InnoDB", "MyISAM", "MEMORY"],
            charsets: ["utf8mb4", "utf8", "latin1"],
            collations: ["utf8mb4_bin", "utf8mb4_general_ci", "latin1_swedish_ci"],
            row_formats: ["DEFAULT", "DYNAMIC", "COMPRESSED", "COMPACT", "REDUNDANT", "FIXED"],
          }),
          list_tables: (args) => {
            const id = args && args.id;
            const schema = (args && args.schemaName) || "";
            if (schema === "navkit") {
              return [...NAVKIT_TABLES.map(([n]) => [n, "table"]), ["v_job_summary", "view"]];
            }
            if (schema === "password_manager") return PM_TABLES.map(([n]) => [n, "table"]);
            return [];
          },
          list_routines: (args) => {
            const schema = (args && args.schemaName) || "";
            if (schema === "navkit") {
              return [["function", "fn_next_seq"], ["procedure", "sp_cleanup_logs"]];
            }
            return [];
          },
          list_objects: (args) => {
            const schema = (args && args.schemaName) || "";
            if (schema === "navkit") {
              return [
                ...NAVKIT_TABLES.map(([n, ai, rows, comment]) => ({
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
          get_table_schema: (args) => tableSchemaFixture(args && args.schemaName, args && args.table),
          // Browser-only metadata fixture; the desktop uses real database catalogs.
          get_diagram_metadata: (args) => {
            const schema = (args && args.schemaName) || "";
            const catalog = schema === "navkit" ? NAVKIT_TABLES : schema === "password_manager" ? PM_TABLES : [];
            return {
              tables: catalog.map(([name]) => ({ ...tableSchemaFixture(schema, name), row_count: null })),
              failures: [],
              limitations: [],
            };
          },
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
            let result;
            if (/note_category/i.test(sql2)) {
              result = {
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
            } else {
              result = { sets: [], messages: [], elapsed_ms: 1 };
            }
            // 记录最近一次执行结果，供 query_result_snapshot / fetch_query_rows 复用同一行集
            spoolStore.set(`${(args && args.id) || ""}|${(args && args.database) || ""}`,
              result.sets.map((s) => ({ columns: s.columns, rows: s.rows })));
            return result;
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
            const name = opts.file_name ? `${String(opts.file_name).replace(/\.sql$/i, "")}.sql` : `${opts.schema}_${stamp}.sql`;
            (backups[key] ??= []).push({
              name,
              modified: `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`,
              size,
              comment: opts.comment || "",
            });
            backupSeq += 1;
            return { rows: opts.include_data ? 137 : 0, bytes: size, path: `C:\\NavKit\\data\\backups\\${opts.connection_id}\\${opts.schema}\\${name}`, warnings: [] };
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
              "DROP TABLE IF EXISTS `navkit_assignment_config`",
              "CREATE TABLE `navkit_assignment_config` (\n  `id` bigint NOT NULL AUTO_INCREMENT,\n  `account_name` varchar(64) DEFAULT NULL,\n  PRIMARY KEY (`id`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
              "INSERT INTO `navkit_assignment_config` VALUES (1,'默认'),(2,'代码'),(3,'权益')",
              "INSERT INTO `navkit_assignment_config` VALUES (33,'测试')",
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
          open_backup_folder: () => "data/backups/c1/navkit",
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
              return "CREATE ALGORITHM=UNDEFINED VIEW `v_job_summary` AS SELECT job_name, COUNT(*) AS cnt FROM `navkit_job_details` GROUP BY job_name";
            }
            return Promise.reject(new Error("视图不存在"));
          },
          get_routine_definition: (args) => {
            if ((args && args.name) === "sp_cleanup_logs") {
              return "CREATE PROCEDURE `sp_cleanup_logs`()\nBEGIN\n  DELETE FROM `navkit_system_log` WHERE created_at < NOW() - INTERVAL 30 DAY;\nEND";
            }
            if ((args && args.name) === "fn_next_seq") {
              return "CREATE FUNCTION `fn_next_seq`(p_name VARCHAR(64)) RETURNS INT\nBEGIN\n  DECLARE v INT;\n  SET v = 1;\n  RETURN v;\nEND";
            }
            return Promise.reject(new Error("对象不存在"));
          },
          // ---- 自动任务 ----
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
          read_db_user: (args) => {
            const name = String((args && args.user) || "") + "@" + String((args && args.host) || "");
            const row = users.find((u) => u[0] === name) || [name, "", "0", "0", "0", "0", "否"];
            return {
              account: {
                user: (args && args.user) || "", host: (args && args.host) || "",
                plugin: "caching_sha2_password", password_expired: false,
                password_lifetime_days: null, account_locked: false,
                max_questions: Number(row[2]) || 0, max_updates: Number(row[3]) || 0,
                max_connections: Number(row[4]) || 0, max_user_connections: Number(row[5]) || 0,
                ssl_type: row[1] || "", ssl_cipher: "", x509_issuer: "", x509_subject: "",
              },
              server_privileges: [
                "Alter", "Alter Routine", "Create", "Create Routine", "Create Temporary Tables",
                "Create User", "Create View", "Delete", "Drop", "Event", "Execute", "File", "Index",
                "Insert", "Lock Tables", "Process", "References", "Reload", "Replication Client",
                "Replication Slave", "Select", "Show Databases", "Show View", "Shutdown", "Super",
                "Trigger", "Update",
              ].map((name) => ({ name, granted: name === "Select", grantable: false }))
                .concat([{ name: "Grant Option", granted: false, grantable: false }]),
              object_privileges: [],
              roles: [],
              members: [],
              supports_roles: false,
              warnings: ["预览环境：角色页需要 MySQL 8.0+ 的 mysql.role_edges。"],
            };
          },
          preview_db_user_changes: (args) => {
            const draft = (args && args.draft) || {};
            const statements = [];
            if (draft.new_user && draft.new_user !== draft.user) {
              statements.push("RENAME USER '" + draft.user + "'@'" + draft.host + "' TO '" + draft.new_user + "'@'" + draft.new_host + "'");
            }
            if (draft.password) statements.push("ALTER USER '" + draft.user + "'@'" + draft.host + "' IDENTIFIED BY '***'");
            return { statements, warnings: statements.length ? [] : ["没有需要应用的变更。"] };
          },
          apply_db_user_changes: (args) => {
            const draft = (args && args.draft) || {};
            if (draft.new_user && draft.new_user !== draft.user) {
              const old = String(draft.user) + "@" + String(draft.host);
              const row = users.find((u) => u[0] === old);
              if (row) row[0] = String(draft.new_user) + "@" + String(draft.new_host);
            }
            return { statements: [], executed: 0 };
          },
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
            const sameEndpoint = (c.url ?? "") === previewSyncConfig.url && (c.user ?? "") === previewSyncConfig.user;
            const headers = c.headers ?? (sameEndpoint ? previewSyncConfig.headers : "");
            previewSyncConfig = {
              url: c.url ?? "", user: c.user ?? "", auto_every: c.auto_every ?? -1,
              remember_password: c.remember_password ?? true,
              password_set: !!c.password, passphrase_set: !!c.passphrase,
              headers_set: !!headers.trim(), headers, last_sync: null,
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
            { ts: "2026-11-07 12:00:15", connection_id: "c1", connection_name: "localhost", kind: "SELECT", database: "navkit", sql: "SELECT * FROM navkit_system_log", ok: false, elapsed_ms: 251, rows: 0, message: "Table 'navkit.navkit_system_log' doesn't exist", origin: "query" },
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
          // ---- 软件更新（需求 1.2：静态预览给同形夹具，真实桌面走 updater 命令）----
          update_check: () => ({ current_version: "1.0.0", update_available: false, manifest: null }),
          update_download: () => { throw new Error("静态预览没有真实更新下载通道"); },
          update_status: () => ({ phase: "idle", done: 0, total: 0, speed: 0, message: "", staged_file: null }),
          update_apply: () => { throw new Error("静态预览没有真实更新安装通道"); },
          // ---- 数据字典导出（需求 1.1）----
          dictionary_metadata: () => ({
            name: "password_manager",
            doc_title: "password_manager数据库设计文档",
            tables: [
              {
                name: "account", comment: "账号表", db_type: "MySQL",
                columns: [
                  { order: 1, name: "id", type_name: "bigint", length: "", primary_key: true, auto_increment: true, nullable: false, default_value: "", comment: "主键" },
                  { order: 2, name: "user_name", type_name: "varchar", length: "50", primary_key: false, auto_increment: false, nullable: false, default_value: "", comment: "用户名" },
                ],
              },
            ],
            views: [{ name: "v_account", comment: "账号视图", script: "CREATE VIEW v_account AS SELECT * FROM account;", db_type: "MySQL" }],
            routines: [{ name: "sp_demo", comment: "", script: "CREATE PROCEDURE sp_demo() BEGIN SELECT 1; END", db_type: "MySQL" }],
          }),
          dictionary_export: (args) => {
            if (!args.filePath) throw new Error("缺少导出路径");
            return 2048;
          },
          // ---- 通用 ----
          open_path_in_explorer: () => null,
          // =====================================================================
          // 全量命令覆盖：前端可能调用的每一个 IPC 命令在这里都必须有响应，
          // 否则静态预览会以「未实现的命令」IpcError 弹错（tools/tests/preview-mock-coverage.test.mjs 防回归）。
          // =====================================================================

          // ---- 受控执行通道（api.ts 预览分支已拦截，这里兜底同形响应）----
          execution_capabilities: () => ({
            version: 1, protocol: "preview", read_only: false,
            arbitrary_sql: { level: "supported", reason: "" },
            cancel: { level: "supported", reason: "" },
            deadline: { level: "supported", reason: "" },
            manual_transaction: { level: "supported", reason: "" },
            safe_browse: { level: "supported", reason: "" },
          }),
          issue_execution_grant: (args) => ({ ...(args.request || {}), execution_grant: "preview_fixture" }),
          execute_sql_v2: (args) => {
            const request = (args && args.request) || {};
            const legacy = responses.execute_sql({ id: request.connection_id, sql: request.sql, database: request.namespace, transactionOwner: request.transaction_owner, origin: request.origin });
            return outcome(request.execution_id, { result: legacy });
          },
          cancel_execution: (args) => ({ execution_id: (args && args.executionId) || "", accepted: false, state: "unknown", message: "预览环境没有可停止的真实执行" }),
          execution_status: (args) => ({ execution_id: (args && args.executionId) || "", state: "unknown", deadline: deadline((args && args.executionId) || ""), resources_pending: false }),

          // ---- 查询结果缓存 / 行写入 ----
          fetch_query_rows: (args) => {
            const key = `${(args && args.connectionId) || ""}|${(args && args.namespace) || ""}`;
            const set = (spoolStore.get(key) || [])[Number(args && args.index) || 0];
            if (!set) return { rows: [], total: 0, truncated: false, message: "预览没有可用的执行缓存" };
            const offset = Number(args && args.offset) || 0;
            const limit = Number(args && args.limit) || 0;
            return { rows: set.rows.slice(offset, offset + limit), total: set.rows.length, truncated: false, message: "" };
          },
          export_query_spooled: () => 10,
          drop_query_spool: (args) => {
            spoolStore.delete(`${(args && args.connectionId) || ""}|${(args && args.namespace) || ""}`);
            return null;
          },
          query_result_snapshot: (args) => {
            const ctx = (args && args.context) || {};
            const sets = spoolStore.get(`${ctx.connection_id || ""}|${ctx.namespace || ""}`) || [];
            const set = sets[Number(ctx.result_index) || 0] || { columns: [], rows: [] };
            const keyIdx = new Set(set.columns.map((c, i) => (c.primary_key ? i : -1)).filter((i) => i >= 0));
            if (!keyIdx.size && set.columns.length) keyIdx.add(0);
            return {
              rows: set.rows.map(() => ({ token: uuid(), row_token: uuid(), values: set.columns.map(() => ({ kind: "text", value: "" })) })),
              loaded_rows: set.rows.length,
              total_rows: set.rows.length,
              editability: {
                source_execution_id: ctx.execution_id || uuid(),
                result_index: Number(ctx.result_index) || 0,
                generation: uuid(),
                result_token: uuid(),
                mode: "single_table",
                reason: "预览夹具提供的写入证据",
                schema: "main",
                table: "preview_rows",
                metadata_revision: fingerprint(),
                columns: set.columns.map((c, i) => ({
                  result_column_id: `c${i}`, result_index: i, name: c.name, base_column: c.name,
                  data_type: c.type || "text", writable: !keyIdx.has(i), key: keyIdx.has(i),
                  generated: false, insertable: true, nullable: c.nullable !== false,
                  auto_increment: !!c.auto_increment, default_value: null,
                  reason: keyIdx.has(i) ? "主键列不直接可写" : "预览夹具列",
                })),
                key_column_ids: [...keyIdx].map((i) => `c${i}`),
                actions: {
                  update: { supported: true, reason: "" },
                  delete: { supported: true, reason: "" },
                  insert: { supported: true, reason: "" },
                },
              },
            };
          },
          preview_query_row_mutation: (args) => ({
            plan_id: uuid(), sql: "/* 预览写入 */ UPDATE ...", binds: [],
            base_revision: "preview-rev-0001", expected_rows: 1, summary: "预览：写入 1 行",
          }),
          execute_row_mutation: (args) => ({
            plan_id: (args && args.planId) || "", status: "applied", transaction: "committed",
            affected: 1, verified: true, mismatches: [], sql: "/* 预览写入 */",
          }),
          release_query_result: () => null,
          preview_row_mutation: (args) => {
            const draft = (args && args.draft) || {};
            const verb = draft.kind === "insert" ? "INSERT INTO" : draft.kind === "delete" ? "DELETE FROM" : "UPDATE";
            return {
              plan_id: uuid(), sql: `/* 预览 */ ${verb} \`${draft.table || "t"}\` ...`, binds: [],
              base_revision: "preview-rev-0001", expected_rows: 1,
              summary: draft.kind === "insert" ? "插入 1 行" : draft.kind === "delete" ? "删除 1 行" : "更新 1 行",
            };
          },

          // ---- 表设计（设计器全页草稿）----
          read_table_design_snapshot: (args) => {
            const schema = tableSchemaFixture(args && args.schema, args && args.table);
            return {
              schema,
              objects: { checks: [], triggers: [], completeness: "complete", limitations: [], table_definition: null },
              options: { engine: schema.engine, charset: "utf8mb4", collation: schema.collation },
              design: designPayloadOf(schema),
              revision: "preview-rev-0001",
              source_fingerprint: fingerprint(),
              owner_id: (args && args.ownerId) || "",
              completeness: "complete",
              capabilities: tableDesignCapabilities(),
            };
          },
          preview_table_design: (args) => ({
            request_sql: "ALTER TABLE `…`（预览模拟，不执行真实 DDL）",
            steps: [{ label: "应用表结构", sql: "ALTER TABLE `…`（预览模拟）" }],
            destructive: false,
            revision: "preview-rev-0002",
            capabilities: tableDesignCapabilities(),
            limitations: ["静态预览不执行真实 DDL"],
          }),
          execute_table_design: (args) => ({
            outcome: outcome((args && args.request && args.request.execution_id) || uuid()),
            receipt: {
              steps: [{ label: "应用表结构", sql: "ALTER TABLE `…`（预览模拟）", status: "applied" }],
              verified: true, unverified: false, mismatches: [], transaction: "committed",
              schema: snapshotOfDesign(args).schema,
              snapshot: snapshotOfDesign(args, args && args.request && args.request.owner_id),
            },
          }),
          apply_table_design: (args) => ({
            steps: [{ label: "应用表结构", sql: "ALTER TABLE `…`（预览模拟）", status: "applied" }],
            verified: true, unverified: false, mismatches: [], transaction: "committed",
            schema: snapshotOfDesign(args).schema,
          }),

          // ---- 表操作（清空 / 复制结构 / 复制数据）----
          preview_table_operation: (args) => {
            const op = (args && args.operation) || {};
            const labels = { clear: "清空表数据", copy_structure: "复制表结构", copy_data: "复制表数据" };
            return {
              request_sql: `/* ${labels[op.kind] || op.kind}（预览模拟） */`,
              steps: [{ label: labels[op.kind] || op.kind, sql: `/* ${labels[op.kind] || op.kind}（预览模拟） */` }],
              destructive: op.kind === "clear",
              revision: "preview-rev-0001",
              capabilities: tableDesignCapabilities(),
              limitations: ["静态预览不执行真实写入"],
              operation: op,
            };
          },
          execute_table_operation: (args) => {
            const op = (args && args.operation) || {};
            const labels = { clear: "清空表数据", copy_structure: "复制表结构", copy_data: "复制表数据" };
            return {
              outcome: outcome((args && args.request && args.request.execution_id) || uuid()),
              receipt: {
                kind: op.kind, source_table: op.table, target_table: op.target_table ?? null,
                affected_rows: op.kind === "clear" ? 15 : 15, copied_rows: op.kind === "copy_data" ? 15 : 0,
                verified: true, mismatches: [], transaction: "committed",
                steps: [{ label: labels[op.kind] || op.kind, sql: "/* 预览模拟 */", status: "applied" }],
                snapshot: null,
              },
            };
          },
          list_table_constraints: () => ({ checks: [], triggers: [], completeness: "complete", limitations: [], table_definition: null }),
          drop_table_trigger: () => 1,

          // ---- 连接健康 ----
          check_connection_health: (args) => ({
            connection_id: (args && args.id) || "", state: "healthy", last_checked_at: nowIso(),
            last_error: null, probe_skipped_reason: null, interval_secs: 30, timeout_secs: 5,
            enabled: true, generation: null, resources_pending: false, next_due_in_ms: 30000,
          }),
          get_connection_health: (args) => ({
            connection_id: (args && args.id) || "", state: "healthy", last_checked_at: nowIso(),
            last_error: null, probe_skipped_reason: null, interval_secs: 30, timeout_secs: 5,
            enabled: true, generation: null, resources_pending: false, next_due_in_ms: 30000,
          }),

          // ---- 受控批任务（数据生成 / SQL 文件）----
          // The generic preview has no native schema/session authority. Do not
          // fabricate a signed recipe sample or a successful database write.
          preview_generated_recipe: () => {
            const error = new Error("RECIPE_PREVIEW_NATIVE_REQUIRED: 配方样本需要真实原生连接；通用预览不伪造结构指纹或授权");
            error.code = "RECIPE_PREVIEW_NATIVE_REQUIRED";
            throw error;
          },
          register_batch_execution: (args) => {
            const request = (args && args.request) || {};
            const task = request.task || {};
            const rows = task.kind === "generated_rows" ? Number(task.request && task.request.rows) || 0 : null;
            return {
              execution_id: uuid(), connection_id: request.connection_id || "", namespace: request.namespace || "",
              owner_id: request.owner_id || "", task_kind: task.kind || "generated_rows",
              execution_grant: "preview_ticket", expires_in_ms: 120000,
              summary: task.kind === "sql_file" ? "SQL 文件（预览模拟）" : `生成 ${rows} 行`,
              total_rows: rows, source_bytes: task.kind === "sql_file" ? 2048 : null,
            };
          },
          run_batch_execution: (args) => {
            const ticket = (args && args.ticket) || {};
            const rows = Number(ticket.total_rows) || 0;
            return {
              context: { execution_id: ticket.execution_id, connection_id: ticket.connection_id, namespace: ticket.namespace, owner_id: ticket.owner_id },
              kind: ticket.task_kind, status: "succeeded",
              metrics: { scanned: rows || 1, executed: rows, errors: 0, elapsed_ms: 36 },
              scan_complete: true, parse_error: null,
              affected_rows: rows, total_rows: ticket.total_rows ?? null,
              transaction_state: "committed",
              logs: [
                { sequence: 1, level: "info", phase: "queued", statement: null, row: null, message: "任务已排队（预览模拟）", sql_template: null },
                { sequence: 2, level: "info", phase: "execute", statement: 1, row: rows || null, message: rows ? `写入 ${rows} 行` : "没有可执行语句", sql_template: "INSERT /* 预览 */", elapsed_ms: 24, affected_rows: rows },
                { sequence: 3, level: "success", phase: "commit", statement: null, row: null, message: "事务已提交", sql_template: null },
              ],
              next_sequence: 4, dropped_before_sequence: 0,
              outcome: outcome(ticket.execution_id, { result: { sets: [], messages: [], elapsed_ms: 36 } }),
            };
          },
          batch_execution_status: (args) => responses.run_batch_execution(args),

          // ---- 驱动管理 ----
          download_database_driver: (args) => ({
            kind: (args && args.kind) || "", version: (args && args.version) || "", status: "ready",
            architecture: "x64", message: "预览：驱动已就绪", can_download: false,
            library: "preview.dll", download_path: "drivers/preview",
          }),
          delete_managed_driver: () => null,
          list_custom_drivers: () => [],
          save_custom_driver: (args) => [(args && args.driver) || {}],
          delete_custom_driver: () => [],
          test_custom_driver: () => ({ ok: true, message: "连接成功（预览模拟）", version: "1.0", charset: "UTF8" }),
          list_odbc_inventory: () => ({
            drivers: [{ name: "ODBC Driver 17 for SQL Server", library: "msodbcsql17.dll" }],
            dsns: [], architecture: "x64",
          }),
          validate_connection_options: () => [],

          // ---- Java 运行时 ----
          java_runtime_status: () => ({
            installed: true, version: "17.0.10", path: "C:/Program Files/Java/jdk-17",
            source: "system", message: "检测到系统 Java 运行时（预览模拟）", can_install: false, download_size: 0,
          }),
          install_java_runtime: () => ({
            installed: true, version: "17.0.10", path: "C:/Program Files/Java/jdk-17",
            source: "system", message: "预览模拟，不安装真实运行时", can_install: false, download_size: 0,
          }),

          // ---- 数据库会话 / 进程 ----
          list_db_processes: () => [
            { id: "8", user: "root", host: "localhost:52100", db: "navkit", command: "Query", state: "Sending data", elapsed_secs: 3, sql: "SELECT * FROM navkit_job_details", is_self: false },
            { id: "9", user: "root", host: "localhost", db: "mysql", command: "Sleep", state: "", elapsed_secs: null, sql: "", is_self: true },
          ],
          kill_db_process: () => "OK",

          // ---- 对象目录 / 对象定义 / 对象删除 ----
          list_object_catalog: (args) => {
            const schema = (args && args.schema) || "";
            const connId = (args && args.id) || "";
            const objects = responses.list_objects({ schemaName: schema }).map((o) => ({
              object: { connection_id: connId, catalog: (args && args.catalog) || null, schema, kind: o.type, name: o.name, identity_arguments: null, parent: null },
              object_key: JSON.stringify([connId, null, schema, o.type, o.name, null, null]),
              display_name: o.name, backend_object_id: null, owner: null, enabled: true,
              definition_readable: true,
              actions: {
                read: { level: "supported", reason: "" },
                definition: { level: "supported", reason: "" },
                read_definition: { level: "supported", reason: "" },
              },
              properties: { engine: o.engine || "", rows: String(o.rows ?? ""), comment: o.comment || "" },
            }));
            return {
              objects, completeness: "complete",
              sections: { table: { state: "known", reason: null }, view: { state: "known", reason: null } },
              revision: "preview-rev-0001", next_cursor: null,
            };
          },
          get_object_definition: (args) => {
            const object = (args && args.object) || {};
            const fallback = responses.get_table_ddl({ table: object.name });
            let definition = "";
            if (object.kind === "view" && object.name === "v_job_summary") {
              definition = "CREATE ALGORITHM=UNDEFINED VIEW `v_job_summary` AS SELECT job_name, COUNT(*) AS cnt FROM `navkit_job_details` GROUP BY job_name";
            } else if (object.name === "sp_cleanup_logs") {
              definition = "CREATE PROCEDURE `sp_cleanup_logs`()\nBEGIN\n  DELETE FROM `navkit_system_log` WHERE created_at < NOW() - INTERVAL 30 DAY;\nEND";
            } else if (object.name === "fn_next_seq") {
              definition = "CREATE FUNCTION `fn_next_seq`(p_name VARCHAR(64)) RETURNS INT\nBEGIN\n  DECLARE v INT;\n  SET v = 1;\n  RETURN v;\nEND";
            }
            return {
              object, backend_object_id: null,
              definition: definition || fallback,
              revision: "preview-rev-0001", properties: {}, completeness: "complete",
            };
          },
          read_object_mutation_baseline: (args) => ({
            baseline_id: uuid(), object: (args && args.object) || {}, source_fingerprint: fingerprint(),
            base_revision: "preview-rev-0001", backend_object_id: "1", metadata_state: "known", expires_in_ms: 60000,
          }),
          preview_object_mutation: (args) => {
            const draft = (args && args.draft) || {};
            return {
              plan_id: uuid(), baseline_id: draft.baseline_id || uuid(), object: draft.object || {},
              source_fingerprint: draft.source_fingerprint || fingerprint(), base_revision: draft.base_revision || "preview-rev-0001",
              review_digest: fingerprint(), request_sql: `DROP TRIGGER IF EXISTS \`${(draft.object && draft.object.name) || "t"}\`（预览模拟）`,
              expires_in_ms: 60000, confirmation_required: true, transaction: "immediate",
            };
          },
          execute_object_mutation: (args) => {
            const work = (args && args.work) || {};
            return {
              outcome: outcome((args && args.request && args.request.execution_id) || uuid()),
              receipt: {
                plan_id: work.plan_id || "", baseline_id: (work.draft && work.draft.baseline_id) || "",
                object: (work.draft && work.draft.object) || {}, backend_object_id: "1",
                source_fingerprint: (work.draft && work.draft.source_fingerprint) || fingerprint(),
                before_revision: "preview-rev-0001", after_revision: "preview-rev-0002",
                status: "applied", verified: true, transaction: "committed", mismatches: [],
              },
            };
          },

          // ---- 例程（读 / 改 / 建 / 调用）----
          read_routine_design: (args) => {
            const name = (args && args.name) || "";
            const definition = name === "sp_cleanup_logs"
              ? "CREATE PROCEDURE `sp_cleanup_logs`()\nBEGIN\n  DELETE FROM `navkit_system_log` WHERE created_at < NOW() - INTERVAL 30 DAY;\nEND"
              : name === "fn_next_seq"
                ? "CREATE FUNCTION `fn_next_seq`(p_name VARCHAR(64)) RETURNS INT\nBEGIN\n  DECLARE v INT;\n  SET v = 1;\n  RETURN v;\nEND"
                : `CREATE ROUTINE \`${name}\`（预览模拟）`;
            return {
              object: { connection_id: (args && args.id) || "", catalog: null, schema: (args && args.schemaName) || "", kind: (args && args.routineType) || "procedure", name, identity_arguments: (args && args.identityArguments) ?? "" },
              definition, revision: "preview-rev-0001", overloads: 1, completeness: "complete",
            };
          },
          read_exact_routine_design: (args) => {
            const base = responses.read_routine_design(args && args.object);
            return {
              ...base,
              object: (args && args.object) || base.object,
              source_fingerprint: fingerprint(), backend_object_id: null,
              creation_context: {}, security_completeness: "complete", replacement_reason: "",
            };
          },
          preview_routine_replacement: (args) => {
            const draft = (args && args.draft) || {};
            return {
              object: draft.object, base_revision: draft.base_revision, backend_object_id: null,
              source_fingerprint: draft.source_fingerprint,
              request_sql: "ALTER ROUTINE …（预览模拟）",
              steps: [{ label: "替换例程定义", sql: draft.definition || "（预览模拟）" }],
              strategy: draft.allow_drop_recreate ? "drop_recreate" : "alter",
              executable: true, warnings: ["静态预览不执行真实 DDL"],
            };
          },
          execute_routine_replacement: (args) => {
            const draft = (args && args.draft) || {};
            return {
              outcome: outcome((args && args.request && args.request.execution_id) || uuid()),
              receipt: {
                steps: [{ label: "替换例程定义", sql: draft.definition || "", status: "applied" }],
                verified: true, mismatches: [], strategy: draft.allow_drop_recreate ? "drop_recreate" : "alter",
                transaction: "committed", revision: "preview-rev-0002", definition: draft.definition || "",
              },
            };
          },
          apply_routine_replacement: (args) => {
            const draft = (args && args.draft) || {};
            return {
              steps: [{ label: "替换例程定义", sql: (draft && draft.definition) || "", status: "applied" }],
              verified: true, mismatches: [], strategy: "alter", transaction: "committed",
              revision: "preview-rev-0002", definition: (draft && draft.definition) || "",
            };
          },
          read_routine_creation_baseline: (args) => ({
            baseline_id: uuid(), object: (args && args.object) || {}, source_fingerprint: fingerprint(),
            base_revision: "preview-absent-0001", completeness: "complete", absent: true,
            expires_in_ms: 60000, strategy: "create_transactional", warnings: [],
          }),
          preview_routine_creation: (args) => {
            const draft = (args && args.draft) || {};
            return {
              baseline_id: draft.baseline_id, object: draft.object, source_fingerprint: draft.source_fingerprint,
              base_revision: draft.base_revision,
              steps: [{ label: "创建例程", sql: draft.definition || "" }],
              request_sql: draft.definition || "", strategy: "create_transactional", executable: true,
              expires_in_ms: 60000, warnings: ["静态预览不执行真实 DDL"],
            };
          },
          execute_routine_creation: (args) => {
            const draft = (args && args.draft) || {};
            return {
              outcome: outcome((args && args.request && args.request.execution_id) || uuid()),
              receipt: {
                object: draft.object, source_fingerprint: draft.source_fingerprint, baseline_id: draft.baseline_id,
                status: "applied", verified: true,
                steps: [{ label: "创建例程", sql: draft.definition || "", status: "applied", error: null }],
                transaction: "committed",
                snapshot: {
                  object: draft.object, definition: draft.definition || "", revision: "preview-rev-0001",
                  source_fingerprint: draft.source_fingerprint, overloads: 1, completeness: "complete",
                  backend_object_id: null, creation_context: {}, security_completeness: "complete", replacement_reason: "",
                },
                warnings: [],
              },
            };
          },
          get_routine_call_metadata: (args) => ({
            object: (args && args.object) || {}, backend_object_id: null, revision: "preview-rev-0001",
            source_fingerprint: fingerprint(), parameters: [], result_type: null,
            definition_revision: "preview-rev-0001", completeness: "complete",
            call: { level: "supported", reason: "" }, notes: [],
          }),
          preview_routine_call: (args) => {
            const draft = (args && args.draft) || {};
            return {
              plan_id: uuid(), metadata_revision: draft.metadata_revision || "preview-rev-0001",
              source_fingerprint: draft.source_fingerprint || fingerprint(), object: draft.object || {},
              sql_preview: `CALL \`${(draft.object && draft.object.name) || "routine"}\`(…)（预览模拟）`,
              parameter_count: (draft.arguments || []).length, expires_in_ms: 60000, warnings: [],
            };
          },
          execute_routine_call: (args) => ({
            outcome: outcome((args && args.request && args.request.execution_id) || uuid()),
            call: { sets: [], outputs: [], warnings: [] },
          }),

          // ---- 数据比较 / 结构同步 ----
          preview_diff_memory: () => {
            const error = new Error("DIFF_MEMORY_PREVIEW_NATIVE_REQUIRED：可用内存预算预估需要原生 OS 采样；浏览器预览不提供模拟硬件额度。正常兼容比较仍可使用。");
            error.code = "DIFF_MEMORY_PREVIEW_NATIVE_REQUIRED";
            throw error;
          },
          sync_diff: () => ({
            source_rows: 15, target_rows: 15, keyed: true,
            missing_in_target: 0, extra_in_target: 0, changed: 0, changed_samples: [],
            target_missing_columns: [], target_extra_columns: [],
            message: "预览：源表与目标表数据一致",
          }),
          preview_controlled_structure_sync: (args) => {
            const fp = fingerprint();
            const request = (args && args.request) || {};
            const binding = {
              principal: "preview", owner: (args && args.ownerId) || "",
              source_connection_id: request.source_connection || "", source_fingerprint: fp,
              source_schema: request.source_schema || "", source_table: request.source_table || "",
              target_connection_id: request.target_connection || "", target_fingerprint: fp,
              target_schema: request.target_schema || "", target_table: request.target_table || "",
            };
            return {
              target_exists: true, changes: [], warnings: [], alterable: true, unsupported: [],
              source_revision: "preview-rev-0001", target_revision: "preview-rev-0001",
              summary: "结构与目标一致（预览模拟）", final_design: null, steps: [],
              review_digest: fp, review_binding: binding,
              request_sql: "/* 结构差异预览（预览模拟） */",
              source_identity: { database: { kind: "mysql", identity: "preview", canonical_path: null }, schema: request.source_schema || "", table: request.source_table || "", rootpage: 2 },
              target_identity: { database: { kind: "mysql", identity: "preview", canonical_path: null }, schema: request.target_schema || "", table: request.target_table || "", rootpage: 2 },
              same_database: false, source_consistency: "ok", observation_digest: fp,
              observation_binding: binding, observation_only: true,
            };
          },
          execute_structure_sync: (args) => ({
            outcome: outcome((args && args.request && args.request.execution_id) || uuid()),
            receipt: {
              report: responses.sync_diff(),
              receipt: {
                steps: [{ label: "应用结构差异", sql: "/* 预览模拟 */", status: "applied" }],
                verified: true, unverified: false, mismatches: [], transaction: "committed",
                schema: tableSchemaFixture("", ""),
              },
            },
          }),
          apply_structure_sync: () => ({
            report: responses.sync_diff(),
            receipt: {
              steps: [{ label: "应用结构差异", sql: "/* 预览模拟 */", status: "applied" }],
              verified: true, unverified: false, mismatches: [], transaction: "committed",
              schema: tableSchemaFixture("", ""),
            },
          }),

          // ---- 调度器 V2 ----
          scheduler_list_jobs: () => schedulerJobs.map(schedulerRecord),
          scheduler_save_job: (args) => {
            const job = (args && args.job) || {};
            if (!job.revision) job.revision = "rev-1";
            const i = schedulerJobs.findIndex((j) => j.id === job.id);
            if (i >= 0) schedulerJobs[i] = job; else schedulerJobs.push(job);
            return schedulerRecord(job);
          },
          scheduler_delete_job: (args) => {
            schedulerJobs = schedulerJobs.filter((j) => j.id !== (args && args.id));
            return null;
          },
          scheduler_list_templates: () => schedulerTemplates.map((t) => ({
            id: t.id, revision: t.revision, name: t.name, kind: t.kind,
            connection_id: t.connection_id, namespace: t.namespace, read_only: !!t.read_only,
          })),
          scheduler_load_template: (args) => {
            const t = schedulerTemplates.find((x) => x.id === (args && args.id));
            if (!t) throw new Error("模板不存在（预览）");
            return t;
          },
          scheduler_save_template: (args) => {
            const template = (args && args.template) || {};
            if (!template.revision) template.revision = "rev-1";
            const i = schedulerTemplates.findIndex((t) => t.id === template.id);
            if (i >= 0) schedulerTemplates[i] = template; else schedulerTemplates.push(template);
            return template;
          },
          scheduler_preview_times: (args) => {
            const schedule = (args && args.schedule) || {};
            if (schedule.kind === "manual") return [];
            return [1, 2, 3].map((n) => ({ utc: isoPlus(n * 3600000), local: isoPlus(n * 3600000), timezone: (schedule && schedule.timezone) || "UTC" }));
          },
          scheduler_prepare_approval: (args) => {
            const job = schedulerJobs.find((j) => j.id === (args && args.id));
            const digest = fingerprint();
            return {
              plan_id: uuid(), review_digest: digest, job_id: (args && args.id) || "",
              job_revision: (job && job.revision) || "rev-1", expires_at_utc: isoPlus(600000),
              actions: (job && job.actions || []).map((a, i) => ({
                action_id: a.id || `action-${i}`, template_name: `模板 ${i + 1}`,
                connection_label: "预览连接", namespace: "navkit", read_only: false,
                details: ["预览模拟动作"], destructive: [],
              })),
            };
          },
          scheduler_approve_job: (args) => {
            const record = schedulerRecord(schedulerJobs[0] || { id: "", revision: "rev-1", enabled: true });
            return record;
          },
          scheduler_run_now: () => runSummary(schedulerJobs[0], "running"),
          scheduler_history: () => [],
          scheduler_cancel_run: (args) => {
            const summary = runSummary(schedulerJobs[0], "cancelled");
            summary.id = (args && args.runId) || summary.id;
            return summary;
          },
          scheduler_prepare_legacy_migration: (args) => ({
            plan_id: uuid(), review_digest: fingerprint(), source_digest: fingerprint(),
            target_revision: "rev-1", timezone: (args && args.timezone) || "UTC",
            prepared_at_utc: nowIso(), expires_at_utc: isoPlus(600000), items: [],
          }),
          scheduler_confirm_legacy_migration: (args) => ({
            plan_id: (args && args.planId) || "", review_digest: (args && args.reviewDigest) || "",
            source_digest: fingerprint(), jobs: [], blocked_count: 0,
          }),

          // ---- 任务型传输 / 批量导出 / SQL 文件 ----
          transfer_task_progress: (args) => ({
            task_id: (args && args.taskId) || "", status: "completed", phase: "done",
            processed: 10, total: 10, message: "预览任务已完成",
          }),
          cancel_transfer_task: () => null,
          run_import_task: () => 10,
          run_export_task: () => 10,
          run_batch_export: (args) => {
            const request = (args && args.request) || {};
            return {
              items: (request.objects || []).map((o) => ({ object: o, file: `${o.name}.${request.format || "csv"}`, rows: 4, status: "completed", error: null })),
              combined_file: null, message: "预览：批量导出完成（不写真实文件）",
            };
          },
          run_sync_database: (args) => {
            const req = (args && args.req) || {};
            return (req.tables || []).map((t) => ({ source_table: t.source, target_table: t.target, rows: 4, created: false, skipped: false, error: null, warnings: [] }));
          },
          dump_database_to_file: () => [137, 48213],
          read_sql_file: () => "",
          run_sql_file: () => ({ total: 0, succeeded: 0, failed: 0, steps: [] }),

          // ---- 其它 ----
          insert_generated_rows: (args) => Number(args && args.req && args.req.rows) || 0,
          manual_transaction_mutate: () => ({ sets: [], messages: ["预览：手动事务写入已模拟"], elapsed_ms: 1 }),
          move_connection: (args) => {
            const conn = store.connections.find((c) => c.id === (args && args.id));
            if (conn && args.group) conn.group = args.group;
            return store.connections.map((c) => ({ ...c }));
          },
          rename_backup: (args) => {
            const key = `${(args && args.connectionId) || ""}/${(args && args.schemaName) || ""}`;
            const list = backups[key] ?? [];
            const item = list.find((b) => b.name === (args && args.file));
            if (item && args && args.newName) item.name = args.newName;
            return null;
          },
          record_job_history: (args) => {
            jobHistory.unshift({
              job_id: (args && args.jobId) || "", job_name: (args && args.jobName) || "",
              started_at: nowIso(), elapsed_ms: Number(args && args.elapsedMs) || 0,
              ok: !!(args && args.ok), detail: (args && args.detail) || "",
            });
            return null;
          },
          runtime_profile_view: () => null,
          test_runtime_connection: () => ({ ok: true, message: "连接成功（预览模拟）", version: "8.0.35", charset: "utf8mb4" }),
        };

        // 数据源同步配置的预览内存态（set_config 写入、get_config 读回）。
        let previewSyncConfig = {
          url: "", user: "", auto_every: -1, remember_password: true,
          password_set: false, passphrase_set: false, headers_set: false, headers: "", last_sync: null,
        };

        // 预览 mock：api.ts 的 call() 判定「真实 Tauri bridge」时禁用 mock；
        // 浏览器预览注入的最小 internals 带 __previewShim 标记，api.ts 据此恢复 mock 优先。
        // 生产/dev exe 内加载本文件时 __TAURI_INTERNALS__ 已是真实 bridge（无标记）→ 保持真实 IPC。
        window.__PREVIEW_MOCK__ = responses;
        if (!window.__TAURI_INTERNALS__) {
          window.__TAURI_INTERNALS__ = {
            __previewShim: true,
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

// 自动演示：展开树到参考截图 1 的状态（本地连接 > localhost > navkit(选中,表组展开) + 对象页）
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
    click(byText("navkit")?.querySelector(".tree-arrow"));
    await wait(500);
    click(byExact("表")?.querySelector(".tree-arrow"));
    await wait(300);
    click(byText("navkit"));
    await wait(700);
    const app = window.__APP__;
    if (app && app.tree) app.tree.render();
    const page = new URLSearchParams(location.search).get("page") || "query";
    if (page === "query") {
      if (app && app.openTableTab) {
        await app.openTableTab("c1", "navkit", "navkit_assignment_config", "table");
        await wait(300);
        await app.openTableTab("c1", "navkit", "navkit_calendars", "table");
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
