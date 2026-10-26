import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

const engineEl = document.getElementById("ocr-engine") as HTMLElement;
const langsEl = document.getElementById("ocr-langs") as HTMLElement;

document.getElementById("btn-close")?.addEventListener("click", () => {
  getCurrentWindow().hide().catch(() => {});
});

async function boot() {
  try {
    const langs = await invoke<string[]>("ocr_languages");
    if (langs.length === 0) {
      engineEl.textContent = "没有可用的 OCR 语言包";
      langsEl.textContent = "—";
      return;
    }
    engineEl.textContent = "Windows.Media.Ocr（系统自带，零下载）";
    langsEl.textContent = langs.join("、");
  } catch (e) {
    engineEl.textContent = `检测失败：${e}`;
  }
}

boot().catch(() => {});
