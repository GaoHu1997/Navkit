/** 用户管理对话框：新建用户 / 修改密码 / 编辑用户（MySQL 协议族）。 */

import { api } from "../core/api";
import { Dialog, checkbox, field, groupbox, numberInput, passwordInput, select, textInput } from "../ui/Dialog";
import type { DbUserDetail, DbUserDraft, DbUserGrantDraft, DbUserPlan } from "../core/db-user";
import {
  AUTH_PLUGIN_OPTIONS,
  OBJECT_PRIVILEGE_COLUMNS,
  PASSWORD_EXPIRY_OPTIONS,
  SSL_TYPE_OPTIONS,
  draftFromDetail,
  privilegeKey,
} from "../core/db-user";
import "../styles/user-dialog.css";

/** 新建用户。 */
export function openCreateUserDialog(
  connId: string,
  onSaved: () => void,
): Dialog {
  const userInput = textInput("", 200);
  const hostInput = textInput("%", 200);
  const passInput = passwordInput("", 200);
  const pass2Input = passwordInput("", 200);
  const dlg = new Dialog({
    title: "新建用户",
    width: 420,
    render: (body) => {
      body.appendChild(field("用户名:", userInput));
      body.appendChild(field("主机:", hostInput));
      const note = document.createElement("div");
      note.style.cssText = "font-size:var(--fs-11);color:var(--text-tertiary)";
      note.textContent = "允许连接的主机，% 表示任意主机";
      body.appendChild(field("", note));
      body.appendChild(field("密码:", passInput));
      body.appendChild(field("确认密码:", pass2Input));
    },
    buttons: [
      {
        label: "创建",
        primary: true,
        onClick: async () => {
          const user = userInput.value.trim();
          if (!user) {
            dlg.showError("请填写用户名");
            return false;
          }
          if (passInput.value !== pass2Input.value) {
            dlg.showError("两次输入的密码不一致");
            return false;
          }
          try {
            await api.createDbUser(connId, user, hostInput.value.trim(), passInput.value);
            onSaved();
          } catch (e) {
            dlg.showError(String(e));
            return false;
          }
          return true;
        },
      },
      { label: "取消" },
    ],
  });
  return dlg;
}

/** 修改用户密码（快速入口）。 */
export function openChangePasswordDialog(
  connId: string,
  user: string,
  host: string,
  onSaved: () => void,
): Dialog {
  const passInput = passwordInput("", 200);
  const pass2Input = passwordInput("", 200);
  const dlg = new Dialog({
    title: "修改密码（" + user + "@" + host + "）",
    width: 400,
    render: (body) => {
      body.appendChild(field("新密码:", passInput));
      body.appendChild(field("确认密码:", pass2Input));
    },
    buttons: [
      {
        label: "确定",
        primary: true,
        onClick: async () => {
          if (passInput.value !== pass2Input.value) {
            dlg.showError("两次输入的密码不一致");
            return false;
          }
          try {
            await api.alterDbUserPassword(connId, user, host, passInput.value);
            onSaved();
          } catch (e) {
            dlg.showError(String(e));
            return false;
          }
          return true;
        },
      },
      { label: "取消" },
    ],
  });
  return dlg;
}

interface ObjectRowState {
  database: string;
  name: string;
  privileges: Set<string>;
}

interface RoleState {
  role: string;
  host: string;
  input: HTMLInputElement;
}

interface EditTab {
  key: string;
  label: string;
  panel: HTMLElement;
}

/**
 * 编辑用户：常规 / 高级 / 成员属于 / 成员 / 服务器权限 / 权限 / SQL 预览，
 * 参考 Navicat 的「编辑用户」对话框；保存时按差量执行 GRANT / REVOKE / ALTER USER。
 */
export function openEditUserDialog(
  connId: string,
  user: string,
  host: string,
  onSaved: (identity: { user: string; host: string }) => void,
): Dialog {
  let detail: DbUserDetail | null = null;
  let loading = true;

  const tabs: EditTab[] = [];

  // 常规
  const userInput = textInput(user, 220);
  const hostInput = textInput(host, 220);
  const pluginSelect = select([{ value: "", label: "默认" }, ...AUTH_PLUGIN_OPTIONS.map((value) => ({ value, label: value }))]);
  pluginSelect.style.width = "220px";
  const passwordInputEl = passwordInput("", 220);
  const confirmInput = passwordInput("", 220);
  const expirySelect = select(PASSWORD_EXPIRY_OPTIONS, "default");
  expirySelect.style.width = "220px";
  const daysInput = numberInput(0, 120);
  daysInput.min = "1";
  daysInput.hidden = true;
  const lockCheckbox = checkbox("账户已锁定", false);

  // 高级
  const maxQuestions = numberInput(0, 140);
  const maxUpdates = numberInput(0, 140);
  const maxConnections = numberInput(0, 140);
  const maxUserConnections = numberInput(0, 140);
  for (const input of [maxQuestions, maxUpdates, maxConnections, maxUserConnections]) {
    input.min = "0";
    input.step = "1";
  }
  const sslTypeSelect = select(SSL_TYPE_OPTIONS, "");
  sslTypeSelect.style.width = "220px";
  const issuerInput = textInput("", 300);
  const subjectInput = textInput("", 300);
  const cipherInput = textInput("", 300);

  // 成员属于
  const roleList = document.createElement("div");
  roleList.className = "user-role-list";
  const roleStates: RoleState[] = [];
  const roleNameInput = textInput("", 150);
  const roleHostInput = textInput("%", 100);
  const roleAddButton = document.createElement("button");
  roleAddButton.type = "button";
  roleAddButton.className = "btn";
  roleAddButton.textContent = "添加角色";

  // 成员
  const memberList = document.createElement("div");
  memberList.className = "user-member-list";

  // 服务器权限
  const serverPrivilegeGrid = document.createElement("div");
  serverPrivilegeGrid.className = "user-privilege-grid";
  const serverChecks: Array<{ name: string; input: HTMLInputElement }> = [];

  // 权限（对象级）：数据库与表从当前连接的真实元数据下拉选择，避免让用户手填系统已有的库/表名。
  const grantDatabaseSelect = document.createElement("select");
  grantDatabaseSelect.id = "user-grant-database";
  grantDatabaseSelect.style.width = "150px";
  grantDatabaseSelect.setAttribute("aria-label", "数据库");
  const grantTableSelect = document.createElement("select");
  grantTableSelect.id = "user-grant-table";
  grantTableSelect.style.width = "150px";
  grantTableSelect.setAttribute("aria-label", "表（留空=整库）");
  const grantStatus = document.createElement("div");
  grantStatus.className = "user-editor-note";
  grantStatus.setAttribute("role", "status");
  grantStatus.setAttribute("aria-live", "polite");
  const grantAddButton = document.createElement("button");
  grantAddButton.type = "button";
  grantAddButton.className = "btn";
  grantAddButton.textContent = "添加权限";
  grantAddButton.disabled = true;
  const objectTableBody = document.createElement("tbody");
  const objectRows: ObjectRowState[] = [];
  let grantDatabases: string[] = [];
  let grantTablesRequest = 0;

  // SQL 预览
  const previewPre = document.createElement("pre");
  previewPre.className = "user-sql-preview";
  const previewRefresh = document.createElement("button");
  previewRefresh.type = "button";
  previewRefresh.className = "btn";
  previewRefresh.textContent = "刷新预览";

  const dlg = new Dialog({
    title: "编辑用户（" + user + "@" + host + "）",
    width: 900,
    height: 680,
    render: (body) => {
      body.classList.add("user-editor-body");

      const tabsEl = document.createElement("div");
      tabsEl.className = "tabs user-editor";
      const header = document.createElement("div");
      header.className = "tabs-header";
      header.setAttribute("role", "tablist");
      const tabsBody = document.createElement("div");
      tabsBody.className = "tabs-body";

      const addTab = (key: string, label: string, panel: HTMLElement) => {
        const tab = document.createElement("button");
        tab.type = "button";
        tab.className = "tabs-tab";
        tab.dataset.userTab = key;
        tab.setAttribute("role", "tab");
        tab.textContent = label;
        tab.addEventListener("click", () => switchTo(key));
        panel.className = "user-editor-panel";
        panel.dataset.userPanel = key;
        panel.setAttribute("role", "tabpanel");
        panel.hidden = true;
        header.appendChild(tab);
        tabsBody.appendChild(panel);
        tabs.push({ key, label, panel });
        return tab;
      };
      const switchTo = (key: string) => {
        for (const item of tabs) {
          const tab = header.querySelector<HTMLElement>('[data-user-tab="' + item.key + '"]');
          const selected = item.key === key;
          if (tab) {
            tab.classList.toggle("active", selected);
            tab.setAttribute("aria-selected", String(selected));
          }
          item.panel.hidden = !selected;
        }
        tabsBody.scrollTop = 0;
        if (key === "sql") void refreshPreview();
      };

      // 常规面板
      const generalPanel = document.createElement("div");
      const pluginRow = field("插件:", pluginSelect);
      generalPanel.appendChild(field("用户名:", userInput));
      generalPanel.appendChild(field("主机:", hostInput));
      generalPanel.appendChild(pluginRow);
      const pluginNote = document.createElement("div");
      pluginNote.className = "user-editor-note";
      pluginNote.textContent = "认证插件；留空使用服务器默认。";
      generalPanel.appendChild(field("", pluginNote));
      generalPanel.appendChild(field("密码:", passwordInputEl));
      generalPanel.appendChild(field("确认密码:", confirmInput));
      const expiryRow = document.createElement("div");
      expiryRow.className = "user-inline";
      expiryRow.append(expirySelect, daysInput);
      generalPanel.appendChild(field("密码过期策略:", expiryRow));
      generalPanel.appendChild(field("", lockCheckbox));
      addTab("general", "常规", generalPanel);

      // 高级面板
      const advancedPanel = document.createElement("div");
      advancedPanel.appendChild(field("每小时最大查询数:", maxQuestions));
      advancedPanel.appendChild(field("每小时最大更新数:", maxUpdates));
      advancedPanel.appendChild(field("每小时最大连接数:", maxConnections));
      advancedPanel.appendChild(field("最大用户连接数:", maxUserConnections));
      const sslBox = groupbox("SSL");
      sslBox.body.appendChild(field("SSL 类型:", sslTypeSelect));
      sslBox.body.appendChild(field("发行者:", issuerInput));
      sslBox.body.appendChild(field("主旨:", subjectInput));
      sslBox.body.appendChild(field("Cipher:", cipherInput));
      advancedPanel.appendChild(sslBox.el);
      addTab("advanced", "高级", advancedPanel);

      // 成员属于
      const rolePanel = document.createElement("div");
      const roleAddRow = document.createElement("div");
      roleAddRow.className = "user-inline user-add-row";
      const roleFormLabel = document.createElement("span");
      roleFormLabel.textContent = "角色:";
      const roleHostLabel = document.createElement("span");
      roleHostLabel.textContent = "主机:";
      roleAddRow.append(roleFormLabel, roleNameInput, roleHostLabel, roleHostInput, roleAddButton);
      rolePanel.appendChild(roleAddRow);
      rolePanel.appendChild(roleList);
      addTab("roles", "成员属于", rolePanel);

      // 成员
      const memberPanel = document.createElement("div");
      memberPanel.appendChild(memberList);
      addTab("members", "成员", memberPanel);

      // 服务器权限
      const serverPanel = document.createElement("div");
      const serverHint = document.createElement("div");
      serverHint.className = "user-editor-note";
      serverHint.textContent = "勾选要授予的全局权限；“Grant Option”表示允许该用户把权限再授予他人。";
      serverPanel.appendChild(serverHint);
      serverPanel.appendChild(serverPrivilegeGrid);
      addTab("server", "服务器权限", serverPanel);

      // 权限（对象级）
      const objectPanel = document.createElement("div");
      const objectAddRow = document.createElement("div");
      objectAddRow.className = "user-inline user-add-row";
      const dbLabel = document.createElement("span");
      dbLabel.textContent = "数据库:";
      const tableLabel = document.createElement("span");
      tableLabel.textContent = "表（留空=整库）:";
      objectAddRow.append(dbLabel, grantDatabaseSelect, tableLabel, grantTableSelect, grantAddButton);
      objectPanel.appendChild(objectAddRow);
      objectPanel.appendChild(grantStatus);
      const objectTable = document.createElement("table");
      objectTable.className = "user-object-table";
      const objectHead = document.createElement("thead");
      const headRow = document.createElement("tr");
      for (const title of ["数据库", "名", ...OBJECT_PRIVILEGE_COLUMNS, ""]) {
        const th = document.createElement("th");
        th.textContent = title;
        headRow.appendChild(th);
      }
      objectHead.appendChild(headRow);
      objectTable.append(objectHead, objectTableBody);
      const objectScroll = document.createElement("div");
      objectScroll.className = "user-object-scroll";
      objectScroll.appendChild(objectTable);
      objectPanel.appendChild(objectScroll);
      addTab("object", "权限", objectPanel);

      // SQL 预览
      const sqlPanel = document.createElement("div");
      const sqlBar = document.createElement("div");
      sqlBar.className = "user-sql-bar";
      sqlBar.appendChild(previewRefresh);
      const sqlHint = document.createElement("span");
      sqlHint.className = "user-editor-note";
      sqlHint.textContent = "以下为保存时将执行的语句（按当前表单实时计算）。";
      sqlBar.appendChild(sqlHint);
      sqlPanel.append(sqlBar, previewPre);
      addTab("sql", "SQL 预览", sqlPanel);

      tabsEl.append(header, tabsBody);
      body.appendChild(tabsEl);

      const loadingHint = document.createElement("div");
      loadingHint.className = "user-editor-loading";
      loadingHint.textContent = "正在读取用户信息…";
      body.appendChild(loadingHint);

      roleAddButton.addEventListener("click", () => {
        const name = roleNameInput.value.trim();
        if (!name) {
          dlg.showError("请填写角色名");
          return;
        }
        addRole({ role: name, host: roleHostInput.value.trim() || "%", input: null as unknown as HTMLInputElement });
        roleNameInput.value = "";
        roleHostInput.value = "%";
      });
      grantDatabaseSelect.addEventListener("change", () => void loadGrantTables());
      grantAddButton.addEventListener("click", () => {
        const database = grantDatabaseSelect.value.trim();
        if (!database) {
          dlg.showError("请选择数据库");
          return;
        }
        const name = grantTableSelect.value.trim() || "*";
        if (objectRows.some((row) => row.database === database && row.name === name)) {
          dlg.showError("该数据库/表的权限行已存在");
          return;
        }
        objectRows.push({ database, name, privileges: new Set() });
        grantTableSelect.value = "";
        renderObjectRows();
      });
      previewRefresh.addEventListener("click", () => void refreshPreview());

      expirySelect.addEventListener("change", () => {
        daysInput.hidden = expirySelect.value !== "interval";
      });

      switchTo("general");
      void load();
      void loadGrantDatabases();
    },
    buttons: [
      {
        label: "保存",
        primary: true,
        onClick: async () => {
          if (loading || !detail) {
            dlg.showError("用户信息尚未加载完成");
            return false;
          }
          if (!userInput.value.trim()) {
            dlg.showError("用户名不能为空");
            return false;
          }
          if (passwordInputEl.value !== confirmInput.value) {
            dlg.showError("两次输入的密码不一致");
            return false;
          }
          const draft = collectDraft();
          try {
            await api.applyDbUserChanges(connId, draft);
            onSaved({ user: draft.new_user, host: draft.new_host });
          } catch (e) {
            dlg.showError(String(e));
            return false;
          }
          return true;
        },
      },
      { label: "取消" },
    ],
  });

  /** 读取当前连接的真实数据库清单，填充「数据库」下拉。 */
  async function loadGrantDatabases(): Promise<void> {
    grantDatabaseSelect.replaceChildren(new Option("正在加载…", ""));
    grantDatabaseSelect.disabled = true;
    grantAddButton.disabled = true;
    grantDatabases = [];
    try {
      const ready = await api.ensureConnection(connId);
      if (!ready.ok) throw new Error(ready.message || "连接失败");
      const databases = await api.listDatabases(connId);
      grantDatabases = [...new Set(databases.map(([name]) => name).filter(Boolean))];
      grantDatabaseSelect.replaceChildren(
        new Option("请选择…", ""),
        ...grantDatabases.map((name) => new Option(name, name)),
      );
      grantDatabaseSelect.disabled = grantDatabases.length === 0;
      grantStatus.textContent = grantDatabases.length ? "" : "当前连接没有可用数据库或模式。";
    } catch (reason) {
      grantDatabases = [];
      grantDatabaseSelect.replaceChildren(new Option("读取失败", ""));
      grantDatabaseSelect.disabled = true;
      grantStatus.textContent = "数据库元数据读取失败：" + String(reason);
    }
    await loadGrantTables();
  }

  /** 读取所选数据库的真实表/视图清单，填充「表」下拉（空值表示整库）。 */
  async function loadGrantTables(): Promise<void> {
    const database = grantDatabaseSelect.value;
    const request = ++grantTablesRequest;
    grantTableSelect.replaceChildren(new Option("（整库）", ""));
    grantTableSelect.disabled = true;
    grantAddButton.disabled = true;
    if (!database) return;
    grantTableSelect.replaceChildren(new Option("正在加载…", ""));
    try {
      const tables = await api.listTables(connId, database);
      if (request !== grantTablesRequest || database !== grantDatabaseSelect.value) return;
      const names = tables
        .filter(([, type]) => type === "table" || type === "view")
        .map(([name]) => name)
        .filter(Boolean);
      grantTableSelect.replaceChildren(
        new Option("（整库）", ""),
        ...names.map((name) => new Option(name, name)),
      );
      grantTableSelect.disabled = false;
      grantAddButton.disabled = false;
      grantStatus.textContent = "";
    } catch (reason) {
      if (request !== grantTablesRequest || database !== grantDatabaseSelect.value) return;
      grantTableSelect.replaceChildren(new Option("读取失败", ""));
      grantTableSelect.disabled = true;
      grantStatus.textContent = "表元数据读取失败：" + String(reason);
    }
  }

  async function load(): Promise<void> {
    try {
      detail = await api.readDbUser(connId, user, host);
      loading = false;
      fill(detail);
      const hint = dlg.body.querySelector(".user-editor-loading");
      hint?.remove();
    } catch (e) {
      loading = false;
      dlg.showError("读取用户失败：" + String(e));
    }
  }

  function fill(data: DbUserDetail): void {
    const account = data.account;
    userInput.value = account.user;
    hostInput.value = account.host;
    if (account.plugin && !AUTH_PLUGIN_OPTIONS.includes(account.plugin)) {
      const option = document.createElement("option");
      option.value = account.plugin;
      option.textContent = account.plugin;
      pluginSelect.appendChild(option);
    }
    pluginSelect.value = account.plugin;
    expirySelect.value = draftFromDetail(data).password_expiry;
    daysInput.value = String(account.password_lifetime_days ?? 0);
    daysInput.hidden = expirySelect.value !== "interval";
    lockCheckbox.querySelector("input")!.checked = account.account_locked;

    maxQuestions.value = String(account.max_questions);
    maxUpdates.value = String(account.max_updates);
    maxConnections.value = String(account.max_connections);
    maxUserConnections.value = String(account.max_user_connections);
    sslTypeSelect.value = account.ssl_type || "";
    issuerInput.value = account.x509_issuer;
    subjectInput.value = account.x509_subject;
    cipherInput.value = account.ssl_cipher;

    serverChecks.length = 0;
    serverPrivilegeGrid.replaceChildren();
    for (const privilege of data.server_privileges) {
      const box = checkbox(privilege.name, privilege.granted);
      const input = box.querySelector("input")!;
      serverPrivilegeGrid.appendChild(box);
      serverChecks.push({ name: privilege.name, input });
    }

    objectRows.length = 0;
    for (const row of data.object_privileges) {
      objectRows.push({
        database: row.database,
        name: row.name,
        privileges: new Set(row.privileges.map((privilege) => privilegeKey(privilege))),
      });
    }
    renderObjectRows();

    roleStates.length = 0;
    roleList.replaceChildren();
    for (const role of data.roles) {
      addRole({ role: role.role, host: role.host, input: null as unknown as HTMLInputElement });
    }
    if (!data.supports_roles) {
      const note = document.createElement("div");
      note.className = "user-editor-note";
      note.textContent = "当前服务器不支持角色（MySQL 8.0 以下或 MariaDB）。";
      roleList.appendChild(note);
    }

    memberList.replaceChildren();
    if (data.members.length === 0) {
      const empty = document.createElement("div");
      empty.className = "user-editor-note";
      empty.textContent = "没有账号通过该用户继承权限。";
      memberList.appendChild(empty);
    } else {
      for (const member of data.members) {
        const item = document.createElement("div");
        item.className = "user-member-item";
        item.textContent = member.user + "@" + member.host;
        memberList.appendChild(item);
      }
    }

    if (data.warnings.length > 0) {
      for (const warning of data.warnings) dlg.showError(warning);
    }
  }

  function addRole(role: { role: string; host: string; input: HTMLInputElement }): void {
    const box = checkbox(role.role + "@" + role.host, true);
    const input = box.querySelector("input")!;
    role.input = input;
    roleList.appendChild(box);
    roleStates.push({ role: role.role, host: role.host, input });
  }

  function renderObjectRows(): void {
    objectTableBody.replaceChildren();
    for (const row of objectRows) {
      const tr = document.createElement("tr");
      const dbCell = document.createElement("td");
      dbCell.textContent = row.database;
      const nameCell = document.createElement("td");
      nameCell.textContent = row.name === "*" ? "（整库）" : row.name;
      tr.append(dbCell, nameCell);
      for (const column of OBJECT_PRIVILEGE_COLUMNS) {
        const td = document.createElement("td");
        const key = privilegeKey(column);
        const holder = document.createElement("label");
        holder.className = "checkbox user-privilege-cell";
        const input = document.createElement("input");
        input.type = "checkbox";
        input.checked = row.privileges.has(key);
        input.addEventListener("change", () => {
          if (input.checked) row.privileges.add(key);
          else row.privileges.delete(key);
        });
        const span = document.createElement("span");
        holder.append(input, span);
        td.appendChild(holder);
        tr.appendChild(td);
      }
      const removeCell = document.createElement("td");
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "btn user-row-remove";
      remove.textContent = "删除";
      remove.addEventListener("click", () => {
        const index = objectRows.indexOf(row);
        if (index >= 0) objectRows.splice(index, 1);
        renderObjectRows();
      });
      removeCell.appendChild(remove);
      tr.appendChild(removeCell);
      objectTableBody.appendChild(tr);
    }
  }

  function collectDraft(): DbUserDraft {
    const objectGrants: DbUserGrantDraft[] = objectRows.map((row) => ({
      database: row.database,
      name: row.name,
      privileges: [...row.privileges].map((key) => displayColumn(key)),
    }));
    return {
      user,
      host,
      new_user: userInput.value.trim(),
      new_host: hostInput.value.trim(),
      password: passwordInputEl.value.length > 0 ? passwordInputEl.value : null,
      plugin: pluginSelect.value,
      password_expiry: expirySelect.value,
      password_lifetime_days: Number(daysInput.value) || 0,
      account_locked: lockCheckbox.querySelector("input")!.checked,
      max_questions: Number(maxQuestions.value) || 0,
      max_updates: Number(maxUpdates.value) || 0,
      max_connections: Number(maxConnections.value) || 0,
      max_user_connections: Number(maxUserConnections.value) || 0,
      ssl_type: sslTypeSelect.value,
      ssl_cipher: cipherInput.value,
      x509_issuer: issuerInput.value,
      x509_subject: subjectInput.value,
      server_privileges: serverChecks.filter((item) => item.input.checked).map((item) => item.name),
      object_grants: objectGrants,
      roles: roleStates.filter((item) => item.input.checked).map((item) => ({ role: item.role, host: item.host })),
    };
  }

  async function refreshPreview(): Promise<void> {
    if (!detail || loading) return;
    try {
      const plan: DbUserPlan = await api.previewDbUserChanges(connId, collectDraft());
      previewPre.textContent =
        plan.statements.length > 0 ? plan.statements.join(";\n") + ";" : "（无变更）";
      if (plan.warnings.length > 0) previewPre.textContent += "\n\n-- " + plan.warnings.join("\n-- ");
    } catch (e) {
      previewPre.textContent = "预览失败：" + String(e);
    }
  }

  return dlg;
}

/** 权限键转显示名。 */
function displayColumn(key: string): string {
  return key
    .split("_")
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1).toLowerCase())
    .join(" ");
}
