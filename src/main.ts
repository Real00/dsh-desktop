import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openPath } from "@tauri-apps/plugin-opener";

type HarnessEvent =
  | { kind: "checking"; message: string }
  | { kind: "installing"; message: string }
  | { kind: "starting"; message: string }
  | { kind: "ready"; url: string }
  | { kind: "error"; message: string }
  | { kind: "update_available"; current: string; latest: string };

type NpmPreset = { id: string; label: string; url: string };
type NpmSettings = { registry: string; presets: NpmPreset[] };

type AppUpdateInfo = {
  updateAvailable: boolean;
  current: string;
  latest: string;
  notes: string;
  downloadUrl: string | null;
  assetName: string | null;
};

const OFFICIAL = "https://registry.npmjs.org";
const NPMMIRROR = "https://registry.npmmirror.com";

const statusEl = () => document.querySelector<HTMLElement>("#status");
const detailEl = () => document.querySelector<HTMLElement>("#detail");
const retryEl = () => document.querySelector<HTMLButtonElement>("#retry");
const updateEl = () => document.querySelector<HTMLElement>("#update");
const updateBtn = () => document.querySelector<HTMLButtonElement>("#update-btn");
const shellEl = () => document.querySelector<HTMLElement>(".shell");
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
const appUpdateProgressEl = () =>
  document.querySelector<HTMLElement>("#app-update-progress");
const checkAppUpdateBtnEl = () =>
  document.querySelector<HTMLButtonElement>("#check-app-update-btn");

let inErrorState = false;
let pendingAppUpdate: AppUpdateInfo | null = null;

function setStatus(message: string) {
  const el = statusEl();
  if (el) el.textContent = message;
}

function showError(message: string) {
  inErrorState = true;
  shellEl()?.classList.add("error");
  setStatus("启动失败");
  const detail = detailEl();
  if (detail) {
    detail.hidden = false;
    detail.textContent = message;
  }
  const retry = retryEl();
  if (retry) retry.hidden = false;
  const saveRetry = npmSaveRetryEl();
  if (saveRetry) saveRetry.hidden = false;
}

function clearError() {
  inErrorState = false;
  shellEl()?.classList.remove("error");
  const detail = detailEl();
  if (detail) {
    detail.hidden = true;
    detail.textContent = "";
  }
  const retry = retryEl();
  if (retry) retry.hidden = true;
  const saveRetry = npmSaveRetryEl();
  if (saveRetry) saveRetry.hidden = true;
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

function setAppUpdateProgress(message: string, isError = false) {
  const el = appUpdateProgressEl();
  if (!el) return;
  el.hidden = !message;
  el.textContent = message;
  el.classList.toggle("error", isError);
}

function hideAppUpdateBanner() {
  const banner = appUpdateBannerEl();
  if (banner) banner.hidden = true;
  pendingAppUpdate = null;
  setAppUpdateProgress("");
}

function showAppUpdateBanner(info: AppUpdateInfo) {
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
  }
  setAppUpdateProgress("");
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

async function restart() {
  clearError();
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
      setStatus(`已就绪，正在打开 ${url}`);
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
      hideAppUpdateBanner();
      if (announce) {
        setNpmStatus(`已是最新应用版本（${info.current}）`);
      }
    }
  } catch (e) {
    if (announce) {
      setNpmStatus(`检查应用更新失败：${e}`, true);
    }
    // Silent on auto-check so splash boot is not blocked by network errors.
  } finally {
    if (btn) btn.disabled = false;
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
  setAppUpdateProgress(
    `正在下载 ${info.assetName || "安装包"}…（保存到 ~/.dsh-desktop/updates/）`,
  );
  try {
    const result = await invoke<{ path: string }>("download_app_update", {
      url: info.downloadUrl,
    });
    setAppUpdateProgress(`下载完成：${result.path}，正在打开…`);
    await openPath(result.path);
    setAppUpdateProgress(`已打开安装包：${result.path}`);
  } catch (e) {
    setAppUpdateProgress(`下载或打开失败：${e}`, true);
  } finally {
    if (dl) {
      dl.disabled = false;
      dl.textContent = "下载更新";
    }
    if (later) later.disabled = false;
  }
}

window.addEventListener("DOMContentLoaded", async () => {
  retryEl()?.addEventListener("click", () => {
    void restart();
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
    hideAppUpdateBanner();
  });

  await loadNpmSettings();

  // Non-blocking app update check (do not await).
  void checkAppUpdate();

  // If Ready fired before the splash listener attached, recover via poll.
  if (!(await pollExistingUrl())) {
    const timer = window.setInterval(() => {
      void pollExistingUrl().then((ok) => {
        if (ok) window.clearInterval(timer);
      });
    }, 500);
    window.setTimeout(() => window.clearInterval(timer), 120000);
  }

  await listen<HarnessEvent>("harness", (event) => {
    const payload = event.payload;
    switch (payload.kind) {
      case "checking":
      case "installing":
      case "starting":
        clearError();
        setStatus(payload.message);
        break;
      case "ready":
        setStatus(`已就绪，正在打开 ${payload.url}`);
        // Keep the Tauri window origin; full document navigation to localhost
        // was leaving a running process with zero windows on macOS.
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
    }
  });

  // Keep save-and-retry visible if we were already in error when settings loaded.
  if (inErrorState) {
    const saveRetry = npmSaveRetryEl();
    if (saveRetry) saveRetry.hidden = false;
  }
});
