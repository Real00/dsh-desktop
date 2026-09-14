import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type HarnessEvent =
  | { kind: "checking"; message: string }
  | { kind: "installing"; message: string }
  | { kind: "starting"; message: string }
  | { kind: "ready"; url: string }
  | { kind: "error"; message: string }
  | { kind: "update_available"; current: string; latest: string };

const statusEl = () => document.querySelector<HTMLElement>("#status");
const detailEl = () => document.querySelector<HTMLElement>("#detail");
const retryEl = () => document.querySelector<HTMLButtonElement>("#retry");
const updateEl = () => document.querySelector<HTMLElement>("#update");
const updateBtn = () => document.querySelector<HTMLButtonElement>("#update-btn");
const shellEl = () => document.querySelector<HTMLElement>(".shell");

function setStatus(message: string) {
  const el = statusEl();
  if (el) el.textContent = message;
}

function showError(message: string) {
  shellEl()?.classList.add("error");
  setStatus("启动失败");
  const detail = detailEl();
  if (detail) {
    detail.hidden = false;
    detail.textContent = message;
  }
  const retry = retryEl();
  if (retry) retry.hidden = false;
}

function clearError() {
  shellEl()?.classList.remove("error");
  const detail = detailEl();
  if (detail) {
    detail.hidden = true;
    detail.textContent = "";
  }
  const retry = retryEl();
  if (retry) retry.hidden = true;
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

window.addEventListener("DOMContentLoaded", async () => {
  retryEl()?.addEventListener("click", () => {
    void restart();
  });
  updateBtn()?.addEventListener("click", () => {
    void doUpdateRuntime();
  });

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
});
