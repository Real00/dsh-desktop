import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type HarnessEvent =
  | { kind: "checking"; message: string }
  | { kind: "installing"; message: string }
  | { kind: "starting"; message: string }
  | { kind: "ready"; url: string }
  | { kind: "error"; message: string };

const statusEl = () => document.querySelector<HTMLElement>("#status");
const detailEl = () => document.querySelector<HTMLElement>("#detail");
const retryEl = () => document.querySelector<HTMLButtonElement>("#retry");
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

window.addEventListener("DOMContentLoaded", async () => {
  retryEl()?.addEventListener("click", () => {
    void restart();
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
    }
  });
});
