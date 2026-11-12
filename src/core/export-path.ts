/** Export destinations are real user paths, never the process working directory. */
export function isAbsoluteExportPath(path: string): boolean {
  return /^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+[\\/]|\/)/.test(path) && !/[\0\r\n]/.test(path);
}
export function exportPathProblem(path: string): string | null {
  if (!path.trim()) return "请指定输出文件的完整路径";
  const value = path.trim();
  const name = value.slice(Math.max(value.lastIndexOf("/"), value.lastIndexOf("\\")) + 1);
  return isAbsoluteExportPath(value) && name.trim() && name !== "." && name !== ".."
    ? null : "请选择包含目录和文件名的绝对路径";
}
export function exportFileName(name: string): string {
  return name.replace(/[<>:"/\\|?*\x00-\x1f]/g, "_").replace(/[. ]+$/, "") || "export";
}
export function exportExtension(path: string, format: string): string {
  if (!path) return path;
  const boundary = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const dot = path.lastIndexOf(".");
  return `${dot > boundary + 1 ? path.slice(0, dot) : path}.${format}`;
}
export async function defaultExportPath(name: string): Promise<string> {
  const path = await import("@tauri-apps/api/path");
  for (const directory of [path.downloadDir, path.documentDir, path.homeDir]) {
    try {
      const dir = await directory();
      if (!dir.trim() || !isAbsoluteExportPath(dir)) continue;
      const output = await path.join(dir, exportFileName(name));
      if (!exportPathProblem(output)) return output;
    } catch { /* Another real user directory may be available. */ }
  }
  throw new Error("无法读取用户导出目录，请使用浏览选择完整保存路径");
}
export interface ExportPathBinding { ready: Promise<void>; problem(): string | null; refresh(): void; autoName(): void }
/** Async defaults never replace text entered while the directory IPC was pending. */
export function bindExportPath(input: HTMLInputElement, name: string | (() => string), format?: HTMLSelectElement): ExportPathBinding {
  let loading = true, error = "", directory = "", automatic = "";
  const fileName = () => exportFileName(typeof name === "function" ? name() : name);
  const refresh = () => { input.title = input.value; };
  const autoName = () => {
    if (directory && (!input.value.trim() || input.value === automatic)) {
      const output = directory + fileName();
      input.value = format ? exportExtension(output, format.value) : output;
      automatic = input.value;
    }
    refresh();
  };
  input.placeholder = "正在读取用户导出目录…";
  input.addEventListener("input", refresh);
  format?.addEventListener("change", () => {
    const wasAutomatic = input.value === automatic;
    input.value = exportExtension(input.value, format.value);
    if (wasAutomatic) automatic = input.value;
    autoName();
  });
  const ready = defaultExportPath(fileName()).then(output => {
    directory = output.slice(0, Math.max(output.lastIndexOf("/"), output.lastIndexOf("\\")) + 1);
    autoName();
  }).catch(reason => { error = String(reason); }).finally(() => { loading = false; input.placeholder = "完整保存路径"; refresh(); });
  refresh();
  return { ready, refresh, autoName, problem: () => input.value.trim() ? exportPathProblem(input.value) : loading ? "正在读取用户导出目录，请稍候" : error || "请指定输出文件的完整路径" };
}
