import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openPath } from "@tauri-apps/plugin-opener";

type PluginInfo = {
  id: string;
  label: string;
  description: string;
  installSpec: string;
  recommended?: boolean;
};

type HarnessEvent =
  | { kind: "checking"; message: string }
  | { kind: "installing"; message: string }
  | { kind: "starting"; message: string }
  | { kind: "ready"; url: string }
  | { kind: "error"; message: string }
  | { kind: "update_available"; current: string; latest: string }
  | { kind: "needs_wizard"; plugins: PluginInfo[]; message: string }
  | { kind: "crashed"; message: string; attempt: number; will_retry: boolean }
  | { kind: "restarting"; message: string; attempt: number };

type NpmPreset = { id: string; label: string; url: string };
type NpmSettings = { registry: string; presets: NpmPreset[] };

type DesktopSettings = {
  wizardCompleted: boolean;
  selectedPlugins: string[];
  autostart: boolean;
  globalShortcut: string;
  globalShortcutEnabled: boolean;
  cliShimEnabled: boolean;
  defaultShortcut: string;
};


type InstalledPlugin = {
  id: string;
  version?: string | null;
  core?: boolean;
  removable?: boolean;
};

type ShimStatus = {
  path: string;
  installed: boolean;
  enabled: boolean;
  hint: string;
  platform: string;
};

type AppUpdateInfo = {
  updateAvailable: boolean;
  current: string;
  latest: string;
  notes: string;
  downloadUrl: string | null;
  assetName: string | null;
};

type AppUpdateEvent = {
  kind: "progress";
  received: number;
  total: number | null;
};

type BootStep = "check" | "install" | "plugins" | "start" | "open";

const OFFICIAL = "https://registry.npmjs.org";
const NPMMIRROR = "https://registry.npmmirror.com";
const STEP_ORDER: BootStep[] = ["check", "install", "plugins", "start", "open"];

const statusEl = () => document.querySelector<HTMLElement>("#status");
const detailEl = () => document.querySelector<HTMLElement>("#detail");
const retryEl = () => document.querySelector<HTMLButtonElement>("#retry");
const copyErrorEl = () => document.querySelector<HTMLButtonElement>("#copy-error");
const gatekeeperEl = () => document.querySelector<HTMLElement>("#gatekeeper-note");
const updateEl = () => document.querySelector<HTMLElement>("#update");
const updateBtn = () => document.querySelector<HTMLButtonElement>("#update-btn");
const shellEl = () => document.querySelector<HTMLElement>(".shell");
const npmSettingsEl = () => document.querySelector<HTMLDetailsElement>(".npm-settings");
const npmCurrentEl = () => document.querySelector<HTMLElement>("#npm-current");
const npmCustomUrlEl = () =>
  document.querySelector<HTMLInputElement>("#npm-custom-url");
const npmStatusEl = () => document.querySelector<HTMLElement>("#npm-status");
const npmSaveEl = () => document.querySelector<HTMLButtonElement>("#npm-save");
const npmSaveRetryEl = () =>
  document.querySelector<HTMLButtonElement>("#npm-save-retry");
const appUpdateBannerEl = () =>
  document.querySelector<HTMLElement>("#app-update-banner");
const appUpdateTextEl = () =>
  document.querySelector<HTMLElement>("#app-update-text");
const appUpdateDownloadEl = () =>
  document.querySelector<HTMLButtonElement>("#app-update-download");
const appUpdateLaterEl = () =>
  document.querySelector<HTMLButtonElement>("#app-update-later");
const appUpdateOpenFolderEl = () =>
  document.querySelector<HTMLButtonElement>("#app-update-open-folder");
const appUpdateProgressEl = () =>
  document.querySelector<HTMLElement>("#app-update-progress");
const appUpdateBarWrapEl = () =>
  document.querySelector<HTMLElement>("#app-update-bar-wrap");
const appUpdateBarEl = () =>
  document.querySelector<HTMLElement>("#app-update-bar");
const checkAppUpdateBtnEl = () =>
  document.querySelector<HTMLButtonElement>("#check-app-update-btn");
const pluginWizardEl = () =>
  document.querySelector<HTMLElement>("#plugin-wizard");
const wizardPluginsEl = () =>
  document.querySelector<HTMLElement>("#wizard-plugins");
const wizardStatusEl = () =>
  document.querySelector<HTMLElement>("#wizard-status");

const pluginManagerEl = () =>
  document.querySelector<HTMLElement>("#plugin-manager");
const pluginManagerHintEl = () =>
  document.querySelector<HTMLElement>("#plugin-manager-hint");
const installedPluginsListEl = () =>
  document.querySelector<HTMLElement>("#installed-plugins-list");
const settingsInstalledPluginsEl = () =>
  document.querySelector<HTMLElement>("#settings-installed-plugins");
const pluginManagerStatusEl = () =>
  document.querySelector<HTMLElement>("#plugin-manager-status");


let inErrorState = false;
let lastErrorText = "";
let pendingAppUpdate: AppUpdateInfo | null = null;
let appUpdateDismissedThisSession = false;

function isMac(): boolean {
  const ua = navigator.userAgent.toLowerCase();
  const plat = (navigator.platform || "").toLowerCase();
  return plat.includes("mac") || ua.includes("mac os") || ua.includes("macintosh");
}

function isWindows(): boolean {
  const ua = navigator.userAgent.toLowerCase();
  const plat = (navigator.platform || "").toLowerCase();
  return plat.includes("win") || ua.includes("windows");
}

function setStatus(message: string) {
  const el = statusEl();
  if (el) el.textContent = message;
}

function setBootStep(step: BootStep, opts?: { reveal?: boolean }) {
  const list = document.querySelectorAll<HTMLElement>("#boot-steps li");
  const idx = STEP_ORDER.indexOf(step);
  list.forEach((li) => {
    const id = li.dataset.step as BootStep | undefined;
    if (!id) return;
    const i = STEP_ORDER.indexOf(id);
    if (opts?.reveal && id === step) {
      li.hidden = false;
    }
    li.classList.toggle("active", i === idx && !li.hidden);
    li.classList.toggle("done", i < idx && !li.hidden);
  });
}

function completeAllSteps() {
  document.querySelectorAll<HTMLElement>("#boot-steps li").forEach((li) => {
    if (!li.hidden) {
      li.classList.remove("active");
      li.classList.add("done");
    }
  });
}

function looksLikeNetworkError(message: string): boolean {
  const m = message.toLowerCase();
  return [
    "npm",
    "registry",
    "enetunreach",
    "econnrefused",
    "econnreset",
    "enotfound",
    "etimedout",
    "network",
    "fetch failed",
    "getaddrinfo",
    "certificate",
    "ssl",
    "tls",
    "proxy",
    "npmmirror",
    "404",
    "403",
    "超时",
    "网络",
    "镜像",
  ].some((k) => m.includes(k));
}

function openNpmSettings() {
  const details = npmSettingsEl();
  if (details) details.open = true;
}

function showError(message: string) {
  inErrorState = true;
  lastErrorText = message;
  shellEl()?.classList.add("error");
  setStatus("启动失败");
  const detail = detailEl();
  if (detail) {
    detail.hidden = false;
    detail.textContent = message;
  }
  const retry = retryEl();
  if (retry) retry.hidden = false;
  const copyBtn = copyErrorEl();
  if (copyBtn) {
    copyBtn.hidden = false;
    copyBtn.textContent = "复制错误信息";
  }
  const saveRetry = npmSaveRetryEl();
  if (saveRetry) saveRetry.hidden = false;

  if (looksLikeNetworkError(message)) {
    openNpmSettings();
  }

  // Harness failed — offer plugin recovery without needing the harness UI.
  showPluginManager({
    hint: "若因不兼容插件无法启动，可在此卸载后重启",
    recovery: true,
  });

  const gk = gatekeeperEl();
  if (gk) {
    if (isMac()) {
      gk.hidden = false;
      gk.textContent =
        "macOS：若系统拦截，请在「系统设置 → 隐私与安全性」中允许打开。";
    } else {
      gk.hidden = true;
      gk.textContent = "";
    }
  }
}

function clearError() {
  inErrorState = false;
  lastErrorText = "";
  shellEl()?.classList.remove("error");
  const detail = detailEl();
  if (detail) {
    detail.hidden = true;
    detail.textContent = "";
  }
  const retry = retryEl();
  if (retry) retry.hidden = true;
  const copyBtn = copyErrorEl();
  if (copyBtn) copyBtn.hidden = true;
  const saveRetry = npmSaveRetryEl();
  if (saveRetry) saveRetry.hidden = true;
  const gk = gatekeeperEl();
  if (gk) {
    gk.hidden = true;
    gk.textContent = "";
  }
}

async function copyError() {
  const text = lastErrorText || detailEl()?.textContent || "";
  if (!text) return;
  const btn = copyErrorEl();
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
    } else {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.left = "-9999px";
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
    }
    if (btn) btn.textContent = "已复制";
    window.setTimeout(() => {
      if (btn) btn.textContent = "复制错误信息";
    }, 1500);
  } catch {
    if (btn) btn.textContent = "复制失败";
  }
}

function presetIdForRegistry(registry: string): "official" | "npmmirror" | "custom" {
  const r = registry.trim().replace(/\/$/, "");
  if (!r || r === OFFICIAL) return "official";
  if (r === NPMMIRROR) return "npmmirror";
  return "custom";
}

function selectedPreset(): "official" | "npmmirror" | "custom" {
  const checked = document.querySelector<HTMLInputElement>(
    'input[name="npm-preset"]:checked',
  );
  const v = checked?.value;
  if (v === "npmmirror" || v === "custom") return v;
  return "official";
}

function syncCustomVisibility() {
  const custom = npmCustomUrlEl();
  if (!custom) return;
  custom.hidden = selectedPreset() !== "custom";
}

function setNpmStatus(message: string, isError = false) {
  const el = npmStatusEl();
  if (!el) return;
  el.hidden = !message;
  el.textContent = message;
  el.classList.toggle("error", isError);
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function setAppUpdateProgress(message: string, isError = false) {
  const el = appUpdateProgressEl();
  if (!el) return;
  el.hidden = !message;
  el.textContent = message;
  el.classList.toggle("error", isError);
}

function setDownloadBar(received: number, total: number | null) {
  const wrap = appUpdateBarWrapEl();
  const bar = appUpdateBarEl();
  if (!wrap || !bar) return;
  wrap.hidden = false;
  if (total && total > 0) {
    bar.classList.remove("indeterminate");
    const pct = Math.min(100, Math.round((received / total) * 100));
    bar.style.width = `${pct}%`;
    setAppUpdateProgress(
      `下载中 ${pct}% · ${formatBytes(received)} / ${formatBytes(total)}`,
    );
  } else {
    bar.classList.add("indeterminate");
    bar.style.width = "";
    setAppUpdateProgress(`下载中… ${formatBytes(received)}`);
  }
}

function hideDownloadBar() {
  const wrap = appUpdateBarWrapEl();
  const bar = appUpdateBarEl();
  if (wrap) wrap.hidden = true;
  if (bar) {
    bar.classList.remove("indeterminate");
    bar.style.width = "0%";
  }
}

function hideAppUpdateBanner() {
  const banner = appUpdateBannerEl();
  if (banner) banner.hidden = true;
  pendingAppUpdate = null;
  setAppUpdateProgress("");
  hideDownloadBar();
  const folder = appUpdateOpenFolderEl();
  if (folder) folder.hidden = true;
}

function showAppUpdateBanner(info: AppUpdateInfo) {
  if (appUpdateDismissedThisSession) return;
  pendingAppUpdate = info;
  const banner = appUpdateBannerEl();
  const text = appUpdateTextEl();
  if (banner) banner.hidden = false;
  if (text) {
    text.textContent = `发现应用新版本 ${info.current} → ${info.latest}`;
  }
  const dl = appUpdateDownloadEl();
  if (dl) {
    dl.disabled = !info.downloadUrl;
    dl.textContent = "下载更新";
    dl.hidden = false;
  }
  const later = appUpdateLaterEl();
  if (later) later.hidden = false;
  const folder = appUpdateOpenFolderEl();
  if (folder) folder.hidden = true;
  setAppUpdateProgress("");
  hideDownloadBar();
}

function installNextSteps(): string {
  if (isMac()) {
    return "已打开安装包，拖到应用程序后请重新打开 DSH Desktop";
  }
  if (isWindows()) {
    return "已打开安装程序，完成后请重新启动应用";
  }
  return "已打开安装包，请按提示完成安装后重新打开应用";
}

function applySettingsToUi(settings: NpmSettings) {
  const registry = settings.registry || OFFICIAL;
  const current = npmCurrentEl();
  if (current) {
    current.textContent = `当前：${registry}`;
  }
  const preset = presetIdForRegistry(registry);
  const radio = document.querySelector<HTMLInputElement>(
    `input[name="npm-preset"][value="${preset}"]`,
  );
  if (radio) radio.checked = true;
  const custom = npmCustomUrlEl();
  if (custom) {
    if (preset === "custom") {
      custom.value = registry;
    } else if (!custom.value) {
      custom.value = "";
    }
  }
  syncCustomVisibility();
}

function registryToSave(): string {
  const preset = selectedPreset();
  if (preset === "official") return OFFICIAL;
  if (preset === "npmmirror") return NPMMIRROR;
  return (npmCustomUrlEl()?.value || "").trim();
}

async function loadNpmSettings() {
  try {
    const settings = await invoke<NpmSettings>("get_npm_settings");
    applySettingsToUi(settings);
  } catch (e) {
    setNpmStatus(`读取设置失败：${e}`, true);
  }
}

async function saveNpmRegistry(andRetry: boolean) {
  const registry = registryToSave();
  if (selectedPreset() === "custom" && !registry) {
    setNpmStatus("请输入自定义 registry URL", true);
    return;
  }
  const saveBtn = npmSaveEl();
  const saveRetryBtn = npmSaveRetryEl();
  if (saveBtn) saveBtn.disabled = true;
  if (saveRetryBtn) saveRetryBtn.disabled = true;
  setNpmStatus("正在保存…");
  try {
    const settings = await invoke<NpmSettings>("set_npm_registry", { registry });
    applySettingsToUi(settings);
    setNpmStatus("已保存");
    if (andRetry) {
      await restart();
    }
  } catch (e) {
    setNpmStatus(`保存失败：${e}`, true);
  } finally {
    if (saveBtn) saveBtn.disabled = false;
    if (saveRetryBtn) saveRetryBtn.disabled = false;
  }
}

function hideWizard() {
  const wiz = pluginWizardEl();
  if (wiz) wiz.hidden = true;
  shellEl()?.classList.remove("wizard-open");
}

function showWizard(plugins: PluginInfo[], message: string, selected?: string[]) {
  const wiz = pluginWizardEl();
  const list = wizardPluginsEl();
  if (!wiz || !list) return;
  shellEl()?.classList.add("wizard-open");
  wiz.hidden = false;
  setStatus(message || "请选择要安装的插件");
  const selectedSet = new Set(
    selected && selected.length > 0 ? selected : plugins.map((p) => p.id),
  );
  list.innerHTML = "";
  for (const p of plugins) {
    const label = document.createElement("label");
    label.className = "wizard-plugin";
    const checked = selectedSet.has(p.id);
    label.innerHTML = `
      <input type="checkbox" data-plugin-id="${p.id}" ${checked ? "checked" : ""} />
      <span>
        <strong>${p.label}</strong> <code>${p.id}</code>
        <small>${p.description}</small>
      </span>
    `;
    list.appendChild(label);
  }
  const st = wizardStatusEl();
  if (st) {
    st.hidden = true;
    st.textContent = "";
  }
}

function collectWizardSelection(): string[] {
  const boxes = document.querySelectorAll<HTMLInputElement>(
    '#wizard-plugins input[type="checkbox"]',
  );
  const ids: string[] = [];
  boxes.forEach((b) => {
    if (b.checked && b.dataset.pluginId) ids.push(b.dataset.pluginId);
  });
  return ids;
}

async function finishWizard(opts: { useRecommended: boolean; skip: boolean }) {
  const st = wizardStatusEl();
  if (st) {
    st.hidden = false;
    st.textContent = "正在保存…";
    st.classList.remove("error");
  }
  try {
    const selected = opts.skip ? [] : collectWizardSelection();
    await invoke("complete_plugin_wizard", {
      selected,
      useRecommended: opts.useRecommended,
    });
    hideWizard();
    setStatus(opts.skip ? "已跳过插件安装，正在启动…" : "正在安装所选插件并启动…");
  } catch (e) {
    if (st) {
      st.textContent = `保存失败：${e}`;
      st.classList.add("error");
    }
  }
}

async function reopenWizard() {
  try {
    const catalog = await invoke<{ plugins: PluginInfo[] }>("get_plugin_catalog");
    const desk = await invoke<DesktopSettings>("get_desktop_settings");
    showWizard(
      catalog.plugins,
      "选择要安装的推荐插件（可跳过）",
      desk.selectedPlugins,
    );
    await invoke("open_plugin_wizard");
  } catch (e) {
    setNpmStatus(`打开向导失败：${e}`, true);
  }
}

async function loadDesktopSettings() {
  try {
    const desk = await invoke<DesktopSettings>("get_desktop_settings");
    const auto = document.querySelector<HTMLInputElement>("#autostart-toggle");
    if (auto) {
      try {
        const live = await invoke<boolean>("get_autostart_enabled");
        auto.checked = live;
      } catch {
        auto.checked = desk.autostart;
      }
    }
    const en = document.querySelector<HTMLInputElement>("#shortcut-enabled");
    if (en) en.checked = desk.globalShortcutEnabled;
    const input = document.querySelector<HTMLInputElement>("#shortcut-input");
    if (input) {
      input.value = desk.globalShortcut || desk.defaultShortcut;
    }
  } catch (e) {
    setNpmStatus(`读取桌面设置失败：${e}`, true);
  }
}

async function loadShimStatus() {
  try {
    const status = await invoke<ShimStatus>("get_shim_status");
    const el = document.querySelector<HTMLElement>("#shim-status");
    if (el) {
      el.textContent = status.installed
        ? `已安装：${status.path}`
        : `未安装（目标：${status.path}）`;
    }
    const hint = document.querySelector<HTMLElement>("#shim-hint");
    if (hint) hint.textContent = status.hint;
  } catch (e) {
    const el = document.querySelector<HTMLElement>("#shim-status");
    if (el) el.textContent = `读取 shim 状态失败：${e}`;
  }
}

async function saveAutostart() {
  const auto = document.querySelector<HTMLInputElement>("#autostart-toggle");
  if (!auto) return;
  try {
    await invoke("set_autostart", { enabled: auto.checked });
    setNpmStatus(auto.checked ? "已开启开机启动" : "已关闭开机启动");
  } catch (e) {
    setNpmStatus(`设置开机启动失败：${e}`, true);
  }
}

async function saveShortcut() {
  const en = document.querySelector<HTMLInputElement>("#shortcut-enabled");
  const input = document.querySelector<HTMLInputElement>("#shortcut-input");
  try {
    await invoke("set_global_shortcut", {
      shortcut: (input?.value || "").trim(),
      enabled: !!en?.checked,
    });
    setNpmStatus("快捷键已保存");
  } catch (e) {
    setNpmStatus(`保存快捷键失败：${e}`, true);
  }
}

async function installShim() {
  try {
    const status = await invoke<ShimStatus>("install_cli_shim");
    setNpmStatus(`已安装 shim：${status.path}`);
    await loadShimStatus();
  } catch (e) {
    setNpmStatus(`安装 shim 失败：${e}`, true);
  }
}

async function removeShim() {
  try {
    await invoke("remove_cli_shim");
    setNpmStatus("已移除 shim");
    await loadShimStatus();
  } catch (e) {
    setNpmStatus(`移除 shim 失败：${e}`, true);
  }
}


function setPluginManagerStatus(message: string, isError = false) {
  const el = pluginManagerStatusEl();
  if (!el) return;
  el.hidden = !message;
  el.textContent = message;
  el.classList.toggle("error", isError);
  setNpmStatus(message, isError);
}

function showPluginManager(opts?: { hint?: string; recovery?: boolean }) {
  const panel = pluginManagerEl();
  if (!panel) return;
  panel.hidden = false;
  panel.classList.toggle("recovery", !!opts?.recovery || !!opts?.hint);
  const hint = pluginManagerHintEl();
  if (hint) {
    const text =
      opts?.hint ||
      "若因不兼容插件无法启动，可在此卸载后重启";
    hint.textContent = text;
    hint.hidden = !(opts?.recovery || opts?.hint);
  }
  openNpmSettings();
  void loadInstalledPlugins();
}

function hidePluginManagerHintOnly() {
  const hint = pluginManagerHintEl();
  if (hint) hint.hidden = true;
  pluginManagerEl()?.classList.remove("recovery");
}

function renderInstalledPlugins(
  plugins: InstalledPlugin[],
  target: HTMLElement | null,
) {
  if (!target) return;
  target.innerHTML = "";
  const removable = plugins.filter((p) => p.removable !== false && !p.core);
  const core = plugins.filter((p) => p.core);
  const show = [...removable, ...core];
  if (show.length === 0) {
    const empty = document.createElement("p");
    empty.className = "installed-plugins-empty";
    empty.textContent = "暂无已安装的第三方插件";
    target.appendChild(empty);
    return;
  }
  for (const p of show) {
    const row = document.createElement("div");
    row.className = "installed-plugin-row";
    const meta = document.createElement("div");
    meta.className = "installed-plugin-meta";
    const title = document.createElement("strong");
    title.textContent = p.id;
    meta.appendChild(title);
    const sub = document.createElement("small");
    const bits: string[] = [];
    if (p.version) bits.push(p.version);
    if (p.core) bits.push("核心（不可卸载）");
    sub.textContent = bits.join(" · ") || "已安装";
    meta.appendChild(sub);
    row.appendChild(meta);
    if (!p.core && p.removable !== false) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "retry secondary";
      btn.textContent = "卸载";
      btn.addEventListener("click", () => {
        void uninstallPlugin(p.id);
      });
      row.appendChild(btn);
    }
    target.appendChild(row);
  }
}

async function loadInstalledPlugins() {
  try {
    const result = await invoke<{ plugins: InstalledPlugin[] }>(
      "list_installed_plugins",
    );
    const plugins = result.plugins || [];
    renderInstalledPlugins(plugins, installedPluginsListEl());
    renderInstalledPlugins(plugins, settingsInstalledPluginsEl());
  } catch (e) {
    setPluginManagerStatus(`读取插件列表失败：${e}`, true);
    const msg = `读取失败：${e}`;
    for (const el of [
      installedPluginsListEl(),
      settingsInstalledPluginsEl(),
    ]) {
      if (!el) continue;
      el.innerHTML = "";
      const p = document.createElement("p");
      p.className = "installed-plugins-empty";
      p.textContent = msg;
      el.appendChild(p);
    }
  }
}

async function uninstallPlugin(name: string) {
  if (!confirm(`确认卸载插件「${name}」？\n卸载后建议重启 dsh。`)) {
    return;
  }
  setPluginManagerStatus(`正在卸载 ${name}…`);
  try {
    await invoke("remove_plugin", { name });
    setPluginManagerStatus(`已卸载 ${name}，请点击「重启 dsh」`);
    await loadInstalledPlugins();
  } catch (e) {
    setPluginManagerStatus(`卸载失败：${e}`, true);
  }
}

async function disableAllPlugins() {
  if (
    !confirm(
      "确认禁用全部第三方插件？\n将备份 package.json 并清空依赖，然后可重启 dsh 恢复启动。",
    )
  ) {
    return;
  }
  setPluginManagerStatus("正在禁用全部插件…");
  try {
    const result = await invoke<{
      ok: boolean;
      backup?: string;
      cleared?: string[];
    }>("safe_disable_all_plugins");
    const n = result.cleared?.length ?? 0;
    setPluginManagerStatus(
      `已禁用 ${n} 个插件（备份：${result.backup || "—"}）。请重启 dsh。`,
    );
    await loadInstalledPlugins();
  } catch (e) {
    setPluginManagerStatus(`禁用失败：${e}`, true);
  }
}

async function restart() {
  clearError();
  hideWizard();
  hidePluginManagerHintOnly();
  setBootStep("check");
  setStatus("正在重新启动…");
  await invoke("restart_harness");
}

async function doUpdateRuntime() {
  const btn = updateBtn();
  if (btn) {
    btn.disabled = true;
    btn.textContent = "更新中…";
  }
  setStatus("正在更新本地 dsh 运行时…");
  try {
    const result = await invoke<{ installed: string }>("update_dsh_runtime");
    setStatus(`运行时已更新到 ${result.installed}，正在重启…`);
    const u = updateEl();
    if (u) u.hidden = true;
    if (btn) btn.hidden = true;
    await invoke("restart_harness");
  } catch (e) {
    showError(String(e));
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.textContent = "更新 dsh 运行时";
    }
  }
}

async function pollExistingUrl() {
  try {
    const url = await invoke<string | null>("harness_url");
    if (url) {
      setStatus(`已就绪，正在打开界面…`);
      setBootStep("open");
      completeAllSteps();
      return true;
    }
  } catch {
    // ignore
  }
  return false;
}

async function checkAppUpdate(opts?: { announceUpToDate?: boolean }) {
  const announce = opts?.announceUpToDate ?? false;
  const btn = checkAppUpdateBtnEl();
  if (btn) btn.disabled = true;
  try {
    const info = await invoke<AppUpdateInfo>("check_app_update");
    if (info.updateAvailable) {
      showAppUpdateBanner(info);
      if (announce) {
        setNpmStatus(`发现应用新版本 ${info.current} → ${info.latest}`);
      }
    } else {
      if (!appUpdateDismissedThisSession) {
        hideAppUpdateBanner();
      }
      if (announce) {
        setNpmStatus(`已是最新版本 ${info.current}`);
      }
    }
  } catch (e) {
    if (announce) {
      setNpmStatus(`检查应用更新失败：${e}`, true);
    }
  } finally {
    if (btn) btn.disabled = false;
  }
}

async function openUpdatesFolder() {
  try {
    const dir = await invoke<string>("get_updates_dir");
    await openPath(dir);
  } catch (e) {
    setAppUpdateProgress(`无法打开下载文件夹：${e}`, true);
  }
}

async function downloadAppUpdate() {
  const info = pendingAppUpdate;
  if (!info?.downloadUrl) {
    setAppUpdateProgress("当前平台没有可用的安装包", true);
    return;
  }
  const dl = appUpdateDownloadEl();
  const later = appUpdateLaterEl();
  if (dl) {
    dl.disabled = true;
    dl.textContent = "下载中…";
  }
  if (later) later.disabled = true;
  hideDownloadBar();
  setDownloadBar(0, null);

  try {
    const result = await invoke<{ path: string; bytes?: number }>(
      "download_app_update",
      { url: info.downloadUrl },
    );
    const bar = appUpdateBarEl();
    if (bar) {
      bar.classList.remove("indeterminate");
      bar.style.width = "100%";
    }
    setAppUpdateProgress(`下载完成，正在打开安装包…`);
    await openPath(result.path);
    setAppUpdateProgress(installNextSteps());
    const folder = appUpdateOpenFolderEl();
    if (folder) folder.hidden = false;
  } catch (e) {
    setAppUpdateProgress(`下载或打开失败：${e}`, true);
    hideDownloadBar();
  } finally {
    if (dl) {
      dl.disabled = false;
      dl.textContent = "下载更新";
    }
    if (later) later.disabled = false;
  }
}

function handleHarnessEvent(payload: HarnessEvent) {
  switch (payload.kind) {
    case "checking":
      clearError();
      hideWizard();
      setStatus(payload.message);
      setBootStep("check");
      break;
    case "installing": {
      clearError();
      hideWizard();
      setStatus(payload.message);
      const msg = payload.message;
      if (msg.includes("插件")) {
        setBootStep("plugins", { reveal: true });
      } else {
        setBootStep("install", { reveal: true });
      }
      break;
    }
    case "starting":
      clearError();
      hideWizard();
      setStatus(payload.message);
      if (payload.message.includes("打开")) {
        setBootStep("open");
      } else {
        setBootStep("start");
      }
      break;
    case "ready":
      hideWizard();
      setStatus(`已就绪，正在打开界面…`);
      setBootStep("open");
      completeAllSteps();
      break;
    case "error":
      showError(payload.message);
      break;
    case "update_available": {
      const u = updateEl();
      if (u) {
        u.hidden = false;
        u.textContent = `发现新版 dsh：${payload.current} → ${payload.latest}`;
      }
      const btn = updateBtn();
      if (btn) btn.hidden = false;
      break;
    }
    case "needs_wizard":
      clearError();
      showWizard(payload.plugins, payload.message);
      break;
    case "crashed":
      setStatus(
        payload.will_retry
          ? `${payload.message}（将自动重启）`
          : payload.message,
      );
      if (!payload.will_retry) {
        showError(
          `${payload.message}\n已停止自动重启，请点击「重启 dsh」。`,
        );
      }
      break;
    case "restarting":
      clearError();
      setStatus(payload.message);
      setBootStep("start");
      break;
  }
}

window.addEventListener("DOMContentLoaded", async () => {
  setBootStep("check");

  retryEl()?.addEventListener("click", () => {
    void restart();
  });
  document
    .querySelector<HTMLButtonElement>("#restart-harness-btn")
    ?.addEventListener("click", () => {
      void restart();
    });
  document
    .querySelector<HTMLButtonElement>("#settings-restart-btn")
    ?.addEventListener("click", () => {
      void restart();
    });
  copyErrorEl()?.addEventListener("click", () => {
    void copyError();
  });
  updateBtn()?.addEventListener("click", () => {
    void doUpdateRuntime();
  });

  document.querySelectorAll<HTMLInputElement>('input[name="npm-preset"]').forEach((el) => {
    el.addEventListener("change", () => {
      syncCustomVisibility();
      setNpmStatus("");
    });
  });
  npmSaveEl()?.addEventListener("click", () => {
    void saveNpmRegistry(false);
  });
  npmSaveRetryEl()?.addEventListener("click", () => {
    void saveNpmRegistry(true);
  });
  checkAppUpdateBtnEl()?.addEventListener("click", () => {
    void checkAppUpdate({ announceUpToDate: true });
  });
  appUpdateDownloadEl()?.addEventListener("click", () => {
    void downloadAppUpdate();
  });
  appUpdateLaterEl()?.addEventListener("click", () => {
    appUpdateDismissedThisSession = true;
    hideAppUpdateBanner();
  });
  appUpdateOpenFolderEl()?.addEventListener("click", () => {
    void openUpdatesFolder();
  });

  document
    .querySelector<HTMLButtonElement>("#wizard-install")
    ?.addEventListener("click", () => {
      void finishWizard({ useRecommended: false, skip: false });
    });
  document
    .querySelector<HTMLButtonElement>("#wizard-recommended")
    ?.addEventListener("click", () => {
      void finishWizard({ useRecommended: true, skip: false });
    });
  document
    .querySelector<HTMLButtonElement>("#wizard-skip")
    ?.addEventListener("click", () => {
      void finishWizard({ useRecommended: false, skip: true });
    });
  document
    .querySelector<HTMLButtonElement>("#reopen-wizard-btn")
    ?.addEventListener("click", () => {
      void reopenWizard();
    });
  document
    .querySelector<HTMLInputElement>("#autostart-toggle")
    ?.addEventListener("change", () => {
      void saveAutostart();
    });
  document
    .querySelector<HTMLButtonElement>("#shortcut-save")
    ?.addEventListener("click", () => {
      void saveShortcut();
    });
  document
    .querySelector<HTMLButtonElement>("#shim-install")
    ?.addEventListener("click", () => {
      void installShim();
    });
  document
    .querySelector<HTMLButtonElement>("#shim-remove")
    ?.addEventListener("click", () => {
      void removeShim();
    });

  document
    .querySelector<HTMLButtonElement>("#plugin-manager-refresh")
    ?.addEventListener("click", () => {
      void loadInstalledPlugins();
    });
  document
    .querySelector<HTMLButtonElement>("#plugin-manager-restart")
    ?.addEventListener("click", () => {
      void restart();
    });
  document
    .querySelector<HTMLButtonElement>("#plugin-manager-disable-all")
    ?.addEventListener("click", () => {
      void disableAllPlugins();
    });
  document
    .querySelector<HTMLButtonElement>("#settings-plugins-refresh")
    ?.addEventListener("click", () => {
      void loadInstalledPlugins();
    });
  document
    .querySelector<HTMLButtonElement>("#settings-open-plugin-mgr")
    ?.addEventListener("click", () => {
      showPluginManager();
    });

  await listen<HarnessEvent>("harness", (event) => {
    handleHarnessEvent(event.payload);
  });

  await listen<{ hint?: string }>("open-plugin-manager", (event) => {
    showPluginManager({
      hint: event.payload?.hint,
      recovery: true,
    });
  });

  await listen<AppUpdateEvent>("app-update", (event) => {
    const p = event.payload;
    if (p.kind === "progress") {
      setDownloadBar(p.received, p.total);
    }
  });

  void loadNpmSettings().then(() => {
    if (inErrorState) {
      const saveRetry = npmSaveRetryEl();
      if (saveRetry) saveRetry.hidden = false;
    }
  });
  void loadDesktopSettings();
  void loadShimStatus();
  void loadInstalledPlugins();
  void checkAppUpdate();

  if (!(await pollExistingUrl())) {
    const timer = window.setInterval(() => {
      void pollExistingUrl().then((ok) => {
        if (ok) window.clearInterval(timer);
      });
    }, 500);
    window.setTimeout(() => window.clearInterval(timer), 120000);
  }
});
