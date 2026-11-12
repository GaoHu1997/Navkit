/**
 * 「新建连接」数据源类型选择器。
 *
 * 版式参照参考程序截图：标题「新建连接」，顶部为视图切换（网格 / 列表）、
 * 搜索框与「驱动管理」入口；左侧为数据源分类，右侧为数据源类型卡片
 * （搜索时跨分类过滤）；底部为「已选择: X」与「下一步」。
 */

import {
  DB_CATEGORIES,
  DB_KINDS,
  kindIcon,
  kindLabel,
  type KindInfo,
} from "../core/dbcatalog";
import { Dialog } from "../ui/Dialog";
import { driverApi, type DriverDefinition, type DatabaseDriverStatus } from "../core/api";
import { currentDriverStatuses, ensureDriverStatuses, refreshDriverStatuses, subscribeDriverStatuses } from "../core/driverStatus";
import { openDriverManager } from "./DriverManagerDialog";
import { prepareConnectionDialog } from "./ConnectionDialog";
import {
  iconSvgByName,
  svg,
  ICON_DOWNLOAD,
  ICON_EXTERNAL_LINK,
  ICON_GRID_VIEW,
  ICON_LIST_VIEW,
  ICON_SPINNER,
} from "../ui/icons";

/** 数据源图标资源名：规划中的类型用 `_off` 灰阶变体（置灰）。 */
function kindIconRes(k: KindInfo): string {
  return kindIcon(k.id) + (k.planned ? "_off" : "");
}

/** A sibling of the selection button, never an interactive child of another button. */
export function makeDriverAction(
  kind: string,
  label: string,
  state: DatabaseDriverStatus | undefined,
  busy: boolean,
  activate: () => void,
): HTMLButtonElement | undefined {
  if (state?.status === "builtin" || state?.status === "ready" || state?.status === "unsupported") return undefined;
  const button = document.createElement("button");
  button.type = "button";
  button.className = "conn-kind-tag conn-kind-download";
  button.dataset.driverKind = kind;
  // 角标只放图标：下载=托盘箭头、获取=外链、检查中/下载中=旋转环（卡片角标空间小，文字会挤压）
  const mode = busy || !state ? "busy" : state.can_download ? "download" : "manual";
  const mirroredSource = ["cache", "sundb", "oscar"].includes(kind);
  const hint = busy
    ? state?.can_download
      ? mirroredSource ? "正在下载并校验自有镜像原件（第三方固定版本）…" : "正在下载并校验官方驱动…"
      : "正在检查本地驱动…"
    : !state
      ? "正在检查当前架构的完整本地驱动…"
      : state.can_download
        ? mirroredSource ? "下载驱动：从自有镜像 Navkit-drivers 下载原件并校验完整性（第三方固定版本）" : "下载驱动：从官方源直接下载并校验完整性"
        : `获取驱动：无法自动下载（${state.message || "受授权或平台限制"}），需按官方渠道手动获取`;
  button.dataset.mode = mode;
  button.innerHTML = svg(
    mode === "download" ? ICON_DOWNLOAD : mode === "manual" ? ICON_EXTERNAL_LINK : ICON_SPINNER,
    14,
  );
  button.disabled = !state;
  button.setAttribute("aria-disabled", String(busy || !state));
  button.setAttribute("aria-label", `${label}：${hint}`);
  button.title = hint;
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    if (!button.disabled && button.getAttribute("aria-disabled") !== "true") {
      // 鼠标激活也先确定焦点所属；随后的同步重绘会移除此按钮。
      button.focus({ preventScroll: true });
      activate();
    }
  });
  button.addEventListener("dblclick", (event) => { event.stopPropagation(); event.preventDefault(); });
  return button;
}

/** 打开选择器；用户确认后回调选中的类型 id。 */
export function openConnectionPicker(
  onPick: (kind: string, driver?: DriverDefinition) => void,
  defaultKind = "mysql",
): Dialog {
  const previousFocus = document.activeElement as HTMLElement | null;
  let customDrivers: DriverDefinition[] = [];
  let customSelected: DriverDefinition | undefined;
  let refresh = () => {};
  let refreshDriverControls = () => {};
  let guardNext = (): boolean => true;
  let loadError = "";
  const driverStatuses = new Map<string, DatabaseDriverStatus>();
  const downloading = new Set<string>();
  let driverFeedback = "正在检查完整本地驱动…";
  let closed = false;
  /** 打开时若已有预热快照则立即渲染（无感），之后靠订阅静默更新。 */
  const hadSnapshot = currentDriverStatuses() != null;
  for (const status of currentDriverStatuses() ?? []) {
    if (!driverStatuses.has(status.kind)) driverStatuses.set(status.kind, status);
  }
  if (driverStatuses.size > 0) driverFeedback = "";
  let initialCheckDone = driverStatuses.size > 0;
  const applyStatuses = (statuses: DatabaseDriverStatus[]) => {
    driverStatuses.clear();
    // 后端按推荐版本优先返回；新建连接默认使用首项，历史版本不能覆盖它。
    for (const status of statuses) {
      if (!driverStatuses.has(status.kind)) driverStatuses.set(status.kind, status);
    }
    if (!initialCheckDone && driverStatuses.size > 0) { driverFeedback = ""; initialCheckDone = true; }
    refreshDriverControls();
  };
  const recheckDrivers = async () => {
    applyStatuses(await ensureDriverStatuses());
  };
  const unsubscribeStatuses = subscribeDriverStatuses((statuses) => {
    if (!closed) applyStatuses(statuses);
  });
  let selected: KindInfo =
    DB_KINDS.find((k) => k.id === defaultKind) ?? DB_KINDS[0];
  let category: string = selected.category;
  let query = "";
  let view: "grid" | "list" = "grid";
  /** Dialog 在同步 render 期间尚未完成构造，footer 只能等构造结束后再取。 */
  let footerEl: HTMLElement | undefined;

  const dlg = new Dialog({
    title: "新建连接",
    width: 900,
    height: 640,
    render: (body) => {
      body.classList.add("conn-picker");
      body.style.display = "flex";
      body.style.flexDirection = "column";
      body.style.padding = "0";

      // ---- 顶部工具条 ----
      const toolbar = document.createElement("div");
      toolbar.className = "conn-picker-toolbar";

      const viewGroup = document.createElement("div");
      viewGroup.className = "conn-picker-views";
      const gridBtn = document.createElement("button");
      gridBtn.className = "conn-picker-viewbtn active";
      gridBtn.title = "网格视图";
      gridBtn.innerHTML = svg(ICON_GRID_VIEW);
      const listBtn = document.createElement("button");
      listBtn.className = "conn-picker-viewbtn";
      listBtn.title = "列表视图";
      listBtn.innerHTML = svg(ICON_LIST_VIEW);
      viewGroup.appendChild(gridBtn);
      viewGroup.appendChild(listBtn);

      const search = document.createElement("input");
      search.type = "search";
      search.className = "conn-picker-search";
      search.placeholder = "搜索数据库类型";

      const spacer = document.createElement("div");
      spacer.style.flex = "1";

      toolbar.appendChild(viewGroup);
      toolbar.appendChild(search);
      toolbar.appendChild(spacer);
      const managerBtn = document.createElement("button");
      managerBtn.className = "btn"; managerBtn.textContent = "驱动管理";
      managerBtn.addEventListener("click", () => openDriverManager((drivers) => {
        customDrivers = drivers;
        customSelected = drivers.find((d) => d.id === customSelected?.id);
        category = "custom"; loadError = ""; query = ""; search.value = ""; render();
      }, () => { void recheckDrivers().catch((error) => { driverFeedback = `驱动重检失败：${String(error)}`; refresh(); }); }));
      toolbar.appendChild(managerBtn);
      gridBtn.setAttribute("aria-label", "网格视图");
      listBtn.setAttribute("aria-label", "列表视图");
      search.setAttribute("aria-label", "搜索数据库类型和自定义驱动");

      // ---- 主体：分类侧栏 + 卡片区 ----
      const main = document.createElement("div");
      main.className = "conn-picker-main";

      const sidebar = document.createElement("div");
      sidebar.className = "conn-picker-cats";

      const content = document.createElement("div");
      content.className = "conn-picker-content";

      main.appendChild(sidebar);
      main.appendChild(content);

      // ---- 底部状态条（已选择 / 提示）----
      const statusBar = document.createElement("div");
      statusBar.className = "conn-picker-status";

      const downloadFeedback = document.createElement("div");
      downloadFeedback.className = "conn-driver-feedback";
      downloadFeedback.setAttribute("role", "status");
      downloadFeedback.setAttribute("aria-live", "polite");
      downloadFeedback.setAttribute("aria-atomic", "true");
      async function activateDriver(k: KindInfo): Promise<void> {
        const state = driverStatuses.get(k.id);
        if (!state || downloading.has(k.id)) return;
        if (!state.can_download) { driverFeedback = `${k.label}：${state.message}`; downloadFeedback.textContent = driverFeedback; return; }
        downloading.add(k.id);
        driverFeedback = `${k.label}：正在直接下载官方驱动并校验完整性，请稍候…`;
        updateDriverControls();
        try {
          const result = await driverApi.download(k.id);
          if (closed) return;
          driverStatuses.set(k.id, result);
          driverFeedback = `${k.label}：${result.message}${result.download_path ? ` 保存位置：${result.download_path}` : ""}`;
          // 下载改变了托管驱动文件（后端指纹变化）：后台强刷全量状态，订阅者静默更新徽标。
          void refreshDriverStatuses().catch(() => {});
        } catch (error) {
          if (!closed) driverFeedback = `${k.label} 驱动下载/重检失败：${String(error)}。可以重试；未完成包不会标为可用。`;
        } finally {
          downloading.delete(k.id);
          if (!closed) updateDriverControls();
        }
      }
      const catEls = new Map<string, HTMLElement>();
      for (const c of [...DB_CATEGORIES, { id: "custom", label: "自定义驱动" }]) {
        const el = document.createElement("button");
        el.type = "button";
        el.className = "conn-picker-cat";
        el.textContent = c.label;
        el.addEventListener("click", () => {
          category = c.id;
          if (c.id !== "custom") customSelected = undefined;
          search.value = "";
          query = "";
          render();
        });
        catEls.set(c.id, el);
        sidebar.appendChild(el);
      }

      function matches(k: KindInfo): boolean {
        if (!query) return true;
        const q = query.toLowerCase();
        return (
          k.id.toLowerCase().includes(q) ||
          k.label.toLowerCase().includes(q) ||
          k.note.toLowerCase().includes(q)
        );
      }

      /** 驱动尚未确认完整可用时，ODBC 及 H2 不能绕过检测进入下一步。 */
      function needsDriver(k: KindInfo): boolean {
        if (k.planned) return false;
        const state = driverStatuses.get(k.id);
        if (!state) return k.protocol === "odbc" || k.id === "h2";
        return state.status !== "builtin" && state.status !== "ready";
      }
      function selectedNeedsDriver(): boolean {
        return !customSelected && needsDriver(selected);
      }
      /** 缺驱动或规划中时禁用「下一步」，并在按钮上说明原因。 */
      function updateNextButton(): void {
        // 构造期 footer 还不存在，此时跳过；构造完成后 refresh() 会再调用一次。
        const btn = footerEl?.querySelector<HTMLButtonElement>(".btn.primary");
        if (!btn) return;
        const blocked = selectedNeedsDriver() || (!!selected.planned && !customSelected);
        btn.disabled = blocked;
        btn.title = selectedNeedsDriver()
          ? !driverStatuses.has(selected.id)
            ? "完整驱动尚未确认可用，请等待检测完成或重试检测"
            : "该数据源需要先下载或手动安装完整驱动才能继续"
          : selected.planned && !customSelected
            ? "该数据源类型尚未支持"
            : "";
      }

      function select(k: KindInfo, el: HTMLElement): void {
        // 规划中的数据源类型置灰且不可选中
        if (k.planned) return;
        selected = k;
        customSelected = undefined;
        content
          .querySelectorAll(".conn-kind-card, .conn-kind-row")
          .forEach((x) => x.classList.remove("selected"));
        el.classList.add("selected");
        content.querySelectorAll<HTMLButtonElement>("button[data-select-kind]")
          .forEach((button) => button.setAttribute("aria-pressed", String(button.dataset.selectKind === k.id)));
        renderStatus();
      }

      function renderStatus(): void {
        statusBar.innerHTML = "";
        statusBar.setAttribute("role", "status");
        if (customSelected) {
          statusBar.textContent = `已选择：${customSelected.name} · ${customSelected.protocol === "jdbc" ? "自定义 JDBC" : "自定义 ODBC"} · ${customSelected.target}`;
          updateNextButton();
          return;
        }
        const icon = document.createElement("span");
        icon.className = "conn-picker-status-icon";
        icon.innerHTML = iconSvgByName(kindIconRes(selected));
        statusBar.appendChild(icon);
        if (selected.planned) {
          const warn = document.createElement("span");
          warn.className = "conn-picker-status-note";
          warn.textContent = `${selected.label}：${selected.note}`;
          statusBar.appendChild(warn);
        } else {
          const text = document.createElement("span");
          text.textContent = `已选择: ${selected.label}`;
          statusBar.appendChild(text);
          if (selected.note) {
            const note = document.createElement("span");
            note.className = "conn-picker-status-note";
            note.title = selected.note;
            note.textContent = selected.note;
            statusBar.appendChild(note);
          }
        }
        if (needsDriver(selected)) {
          const warn = document.createElement("span");
          warn.className = "conn-picker-status-note conn-picker-driver-warn";
          const state = driverStatuses.get(selected.id);
          warn.textContent = !state
            ? "完整驱动尚未确认可用，请等待检测完成或重试检测"
            : state.can_download
              ? "需要先下载驱动才能继续"
              : "需要先手动获取并安装驱动才能继续";
          statusBar.appendChild(warn);
        }
        updateNextButton();
      }

      function makeKind(k: KindInfo, layout: "card" | "row"): HTMLElement {
        const el = document.createElement("div");
        el.className = `conn-kind-${layout}` + (k.id === selected.id ? " selected" : "") + (k.planned ? " disabled" : "") + (needsDriver(k) ? " needs-driver" : "");
        const choose = document.createElement("button");
        choose.type = "button";
        choose.className = "conn-kind-select";
        choose.dataset.selectKind = k.id;
        choose.disabled = k.planned;
        choose.setAttribute("aria-label", `选择 ${k.label}`);
        choose.setAttribute("aria-pressed", String(k.id === selected.id && !customSelected));
        const icon = document.createElement("span"); icon.className = "conn-kind-icon";
        icon.innerHTML = iconSvgByName(kindIconRes(k)); icon.setAttribute("aria-hidden", "true");
        const name = document.createElement("span"); name.className = "conn-kind-name"; name.textContent = kindLabel(k.id);
        choose.append(icon, name);
        if (layout === "row") {
          const cat = document.createElement("span"); cat.className = "conn-kind-cat";
          cat.textContent = DB_CATEGORIES.find((c) => c.id === k.category)?.label ?? "";
          const note = document.createElement("span"); note.className = "conn-kind-note"; note.textContent = k.planned ? "规划中" : k.note;
          choose.append(cat, note);
        }
        choose.addEventListener("click", () => select(k, el));
        choose.addEventListener("dblclick", () => {
          if (k.planned) return;
          // 双击 = 选中；只有驱动就绪、且与「下一步」按钮同一套守卫放行时才进入下一步，
          // 缺驱动时绝不允许双击绕过下载步骤。
          select(k, el);
          if (selected.id !== k.id || !guardNext()) return;
          dlg.close(); onPick(k.id);
        });
        el.append(choose);
        if (k.planned && layout === "card") {
          const tag = document.createElement("span"); tag.className = "conn-kind-tag"; tag.textContent = "规划中"; el.append(tag);
        } else if (!k.planned) {
          const action = makeDriverAction(k.id, k.label, driverStatuses.get(k.id), downloading.has(k.id), () => { void activateDriver(k); });
          if (action) el.append(action);
        }
        return el;
      }
      /** Status-only refresh keeps the original controls attached, including a busy trigger. */
      function updateDriverControls(): void {
        if (closed) return;
        const buttons = Array.from(content.querySelectorAll<HTMLButtonElement>("button"));
        for (const k of DB_KINDS) {
          const choose = buttons.find((button) => button.dataset.selectKind === k.id);
          const card = choose?.parentElement;
          if (!choose || !card || k.planned) continue;
          card.classList.toggle("needs-driver", needsDriver(k));
          const current = buttons.find((button) => button.dataset.driverKind === k.id);
          const next = makeDriverAction(k.id, k.label, driverStatuses.get(k.id), downloading.has(k.id), () => { void activateDriver(k); });
          if (current && next) {
            current.dataset.mode = next.dataset.mode;
            current.disabled = next.disabled;
            current.title = next.title;
            current.setAttribute("aria-disabled", next.getAttribute("aria-disabled") ?? "false");
            current.setAttribute("aria-label", next.getAttribute("aria-label") ?? "");
            if (current.innerHTML !== next.innerHTML) current.innerHTML = next.innerHTML;
          } else if (current) {
            // Transfer focus before removing the completed trigger, never via BODY.
            if (current === document.activeElement || current.contains(document.activeElement)) choose.focus({ preventScroll: true });
            current.remove();
          } else if (next) {
            card.append(next);
          }
        }
        renderStatus();
        downloadFeedback.textContent = driverFeedback;
      }
      function makeCard(k: KindInfo): HTMLElement { return makeKind(k, "card"); }
      function makeRow(k: KindInfo): HTMLElement { return makeKind(k, "row"); }

      function makeCustom(driver: DriverDefinition): HTMLElement {
        const el = document.createElement("button"); el.type = "button";
        el.className = (view === "grid" ? "conn-kind-card" : "conn-kind-row") + (driver.id === customSelected?.id ? " selected" : "");
        el.style.textAlign = view === "grid" ? "center" : "left";
        const icon = document.createElement("div"); icon.className = "conn-kind-icon";
        icon.innerHTML = iconSvgByName("info_pane_conn_odbc"); icon.setAttribute("aria-hidden", "true");
        const name = document.createElement("div"); name.className = "conn-kind-name"; name.textContent = driver.name;
        const note = document.createElement("div"); note.className = view === "grid" ? "conn-kind-tag" : "conn-kind-note";
        note.textContent = driver.protocol === "jdbc" ? "自定义 JDBC" : "自定义 ODBC"; el.title = driver.target;
        el.append(icon, name, note);
        el.addEventListener("click", () => { customSelected = driver; render(); });
        el.addEventListener("dblclick", () => { dlg.close(); onPick("odbc", driver); });
        return el;
      }

      function render(): void {
        if (closed) return;
        const focused = document.activeElement as HTMLElement | null;
        const focusKind = content.contains(focused) ? focused?.dataset.driverKind ?? focused?.dataset.selectKind : undefined;
        const wasDownload = !!focused?.dataset.driverKind;
        // 分类高亮；搜索时不高亮任何分类
        for (const [cid, el] of catEls) {
          el.classList.toggle("active", !query && cid === category);
          el.classList.toggle("dimmed", !!query);
        }
        content.innerHTML = "";
        const visibleKinds = DB_KINDS;
        const source = query
          ? visibleKinds.filter(matches)
          : visibleKinds.filter(
              (k) => k.category === category && matches(k),
            );
        if (view === "grid") {
          content.classList.add("grid");
          content.classList.remove("list");
          for (const k of source) content.appendChild(makeCard(k));
        } else {
          content.classList.add("list");
          content.classList.remove("grid");
          for (const k of source) content.appendChild(makeRow(k));
        }
        const driverSource = customDrivers.filter((d) => query
          ? `${d.name} ${d.target} ODBC`.toLowerCase().includes(query.toLowerCase())
          : category === "custom");
        for (const driver of driverSource) content.appendChild(makeCustom(driver));
        if (!source.length && !driverSource.length) {
          const empty = document.createElement("div");
          empty.className = "conn-picker-empty";
          empty.textContent = category === "custom"
            ? loadError || "暂无自定义驱动。点击「驱动管理」添加已安装驱动、本地驱动文件或 DSN。"
            : "没有匹配的数据源类型";
          content.appendChild(empty);
        }
        renderStatus();
        downloadFeedback.textContent = driverFeedback;
        if (focusKind) {
          const controls = Array.from(content.querySelectorAll<HTMLButtonElement>("button"));
          const matching = controls.find((button) => wasDownload && button.dataset.driverKind === focusKind)
            ?? controls.find((button) => button.dataset.selectKind === focusKind);
          matching?.focus();
        }
      }

      gridBtn.addEventListener("click", () => {
        view = "grid";
        gridBtn.classList.add("active");
        listBtn.classList.remove("active");
        render();
      });
      listBtn.addEventListener("click", () => {
        view = "list";
        listBtn.classList.add("active");
        gridBtn.classList.remove("active");
        render();
      });
      search.addEventListener("input", () => {
        query = search.value.trim();
        render();
      });

      refresh = render;
      refreshDriverControls = updateDriverControls;
      guardNext = () => {
        if (selectedNeedsDriver()) { renderStatus(); return false; }
        return true;
      };
      body.appendChild(toolbar);
      body.appendChild(main);
      body.appendChild(statusBar);
      body.appendChild(downloadFeedback);
      render();
    },
    buttons: [
      {
        label: "下一步",
        primary: true,
        onClick: () => {
          // 规划中的类型不可进入下一步
          if (customSelected) { dlg.close(); onPick("odbc", customSelected); return; }
          if (selected.planned || category === "custom" && !query) return false;
          // 驱动未就绪：停留在本页并提示，必须先把驱动下载/安装好
          if (!guardNext()) return false;
          dlg.close();
          onPick(selected.id);
        },
      },
    ],
  });
  prepareConnectionDialog(dlg);
  // Dialog builds its footer after synchronous render; apply the initial driver guard now.
  footerEl = dlg.footer;
  refresh();
  dlg.onClose(() => { closed = true; unsubscribeStatuses(); if (previousFocus?.isConnected) previousFocus.focus(); });
  void recheckDrivers().then(() => {
    // 预热命中的静默打开不再提示「检查完成」；首次真实检查才说明一次。
    if (!closed && !hadSnapshot) {
      driverFeedback = "本地驱动检查完成。下载会直接保存官方包并验证完整性；「获取驱动」仅说明授权/手动安装限制。";
      refreshDriverControls();
    }
  })
    .catch((error) => { if (!closed) { driverFeedback = `驱动检测失败：${String(error)}。状态未知，不会标为可用。`; refresh(); } });
  void driverApi.list().then((drivers) => { customDrivers = drivers; refresh(); })
    .catch((error) => { loadError = `读取驱动失败：${String(error)}`; refresh(); });
  return dlg;
}
