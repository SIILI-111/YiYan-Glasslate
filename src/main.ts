import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type OcrLine = { text: string; x: number; y: number; w: number; h: number };

type OcrPayload = {
  rect: { x: number; y: number; w: number; h: number };
  capture_ms: number;
  total_ms: number;
  lines: OcrLine[];
  words: unknown[];
  image: { w: number; h: number };
  engine_lang: string;
  ocr_ms: number;
};

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const content = $("content");
const emptyBox = $("empty");
const resultBox = $("result");
const errorBox = $("error");
const linesBox = $("lines");
const rawBox = $<HTMLPreElement>("raw");
const metaBox = $("meta");
const errorBody = $("error-body");
const statusBox = $("status");
const engineBox = $("engine");

let lastPayload: OcrPayload | null = null;
let showRaw = false;
let stealth = false;

function setStatus(text: string) {
  statusBox.textContent = text;
}

function show(which: "empty" | "result" | "error") {
  emptyBox.hidden = which !== "empty";
  resultBox.hidden = which !== "result";
  errorBox.hidden = which !== "error";
  content.scrollTop = 0;
}

function renderResult(p: OcrPayload) {
  lastPayload = p;
  show("result");
  linesBox.innerHTML = "";
  for (const line of p.lines) {
    const el = document.createElement("p");
    el.textContent = line.text;
    linesBox.appendChild(el);
  }
  rawBox.textContent = p.lines.map((l) => l.text).join("\n");
  rawBox.hidden = !showRaw;
  linesBox.hidden = showRaw;

  const chars = p.lines.reduce((n, l) => n + l.text.length, 0);
  metaBox.textContent = `${p.lines.length} 行 · ${chars} 字符 · 抓屏 ${p.capture_ms}ms · OCR ${p.ocr_ms}ms`;
  engineBox.textContent = `OCR ${p.engine_lang}`;
  setStatus("识别完成");
}

function renderError(message: string) {
  show("error");
  errorBody.textContent = message;
  setStatus("识别失败");
}

async function startSelection() {
  setStatus("框选中…");
  try {
    await invoke("start_selection");
  } catch (e) {
    setStatus(`唤起失败：${e}`);
  }
}

async function copyAll() {
  const text = lastPayload?.lines.map((l) => l.text).join("\n") ?? "";
  if (!text) {
    setStatus("没有可复制的内容");
    return;
  }
  try {
    await navigator.clipboard.writeText(text);
    setStatus("已复制");
  } catch {
    setStatus("复制失败（剪贴板不可用）");
  }
}

async function toggleStealth() {
  stealth = !stealth;
  void invoke("debug_log", { msg: `toggleStealth ${stealth}` }).catch(() => {});
  try {
    await invoke("set_glass_click_through", { enable: stealth });
    if (stealth) {
      await invoke("start_strip_hover", { stripHeight: 48 });
      setStatus("隐形：点击穿透已开（Alt+E 恢复）");
    } else {
      await invoke("stop_strip_hover");
      setStatus("已恢复可点击");
    }
  } catch (e) {
    setStatus(`切换失败：${e}`);
  }
}

function wire() {
  $("btn-new").addEventListener("click", startSelection);
  $("btn-retry").addEventListener("click", startSelection);
  $("btn-copy").addEventListener("click", copyAll);
  $("btn-retranslate").addEventListener("click", startSelection);
  $("btn-settings").addEventListener("click", () => invoke("show_settings").catch(() => {}));
  $("btn-close").addEventListener("click", () => {
    invoke("hide_main").catch(() => {});
  });
  $("btn-toggle-raw").addEventListener("click", () => {
    showRaw = !showRaw;
    rawBox.hidden = !showRaw;
    linesBox.hidden = showRaw;
  });

  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") invoke("hide_main").catch(() => {});
  });
}

async function boot() {
  wire();
  await listen<OcrPayload>("ocr-result", (e) => renderResult(e.payload));
  await listen<{ message: string }>("ocr-failed", (e) => renderError(e.payload.message));
  await listen("hotkey-retranslate", () => startSelection());
  await listen("hotkey-stealth", () => toggleStealth());
  await listen<{ inside: boolean }>("strip-hover", (e) => {
    if (e.payload.inside) setStatus("工具条区（可点）");
  });

  try {
    const langs = await invoke<string[]>("ocr_languages");
    engineBox.textContent = langs.length ? `OCR ${langs[0]}` : "没有可用 OCR 语言";
  } catch {
    engineBox.textContent = "OCR 未就绪";
  }
  show("empty");
  setStatus("就绪 · Alt+Q 框选");

  // 无人值守自检：设了 GLASSLATE_SELFTEST="x,y,w,h" 时启动就抓这块屏跑一次 OCR，
  // 结果只写日志（dev 终端可见），不打扰界面。
  const rect = await invoke<number[] | null>("selftest_rect").catch(() => null);
  if (rect) {
    const out = await invoke<string>("ocr_rect", {
      x: rect[0],
      y: rect[1],
      w: rect[2],
      h: rect[3],
    }).catch((e) => `ERR ${e}`);
    void invoke("debug_log", {
      msg: `selftest ${String(out).replace(/\n/g, " / ")}`,
    }).catch(() => {});
    // 顺手验证穿透开关真的落到了窗口样式上（结果由 Rust 打日志）
    await invoke("set_glass_click_through", { enable: true }).catch(() => {});
    await invoke("set_glass_click_through", { enable: false }).catch(() => {});
  }
}

boot().catch((e) => {
  setStatus(`初始化失败：${e}`);
});
