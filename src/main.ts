import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

interface MonitorDto {
  name: string | null;
  logical_pos: [number, number];
  logical_size: [number, number];
  scale: number;
  physical_size_est: [number, number];
}

interface EnvInfo {
  hwnd: number;
  dpi_awareness_note: string;
  tauri_outer_position: [number, number];
  tauri_outer_size: [number, number];
  tauri_scale_factor: number;
  tauri_inner_size: [number, number];
  win32_window_rect: Rect;
  win32_dpi_for_window: number;
  win32_window_monitor: Rect;
  win32_window_monitor_work: Rect;
  cursor: [number, number];
  virtual_screen: Rect;
  monitors: MonitorDto[];
}

interface GrabResult {
  ms: number;
  width: number;
  height: number;
  png_base64: string;
  saved_to: string;
  note: string;
}

const $ = <T extends HTMLElement>(sel: string) => document.querySelector(sel) as T;

const logEl = $<HTMLElement>("#log");
function log(msg: string) {
  logEl.textContent = `${new Date().toLocaleTimeString()} · ${msg}`;
}

const win = getCurrentWindow();

// ------------------------------------------------------------ T1

function renderT1() {
  const dpr = window.devicePixelRatio;
  const frame = window.outerWidth - window.innerWidth;
  const items: Array<[boolean, string]> = [
    [frame <= 2, `无边框：outer-inner = ${frame}px${frame <= 2 ? "（≤2px，无系统边框）" : "（有边框！）"}`],
    [dpr > 1, `devicePixelRatio = ${dpr}${dpr > 1 ? "（高 DPI 生效）" : "（100% 缩放）"}`],
    [true, "透明：卡片外的像素直接透出桌面（肉眼确认）"],
    [true, "置顶：被别的窗口盖住即失败（肉眼确认）"],
  ];
  const ul = $<HTMLUListElement>("#t1-list");
  ul.innerHTML = "";
  for (const [ok, text] of items) {
    const li = document.createElement("li");
    li.className = ok ? "ok" : "warn";
    li.textContent = text;
    ul.appendChild(li);
  }
}

// ------------------------------------------------------------ T3

async function refreshEnv() {
  try {
    const env = await invoke<EnvInfo>("env_info");
    const r = env.win32_window_rect;
    const w = r.right - r.left;
    const h = r.bottom - r.top;
    const lines = [
      `hwnd=${env.hwnd}`,
      `tauri pos=${env.tauri_outer_position} size=${env.tauri_outer_size} scale=${env.tauri_scale_factor} inner=${env.tauri_inner_size}`,
      `win32 rect=(${r.left},${r.top},${r.right},${r.bottom}) 即 ${w}x${h}`,
      `dpi=${env.win32_dpi_for_window}`,
      `monitor=${env.win32_window_monitor.left},${env.win32_window_monitor.top} ${env.win32_window_monitor.right - env.win32_window_monitor.left}x${env.win32_window_monitor.bottom - env.win32_window_monitor.top}`,
      `work=${env.win32_window_monitor_work.right - env.win32_window_monitor_work.left}x${env.win32_window_monitor_work.bottom - env.win32_window_monitor_work.top}`,
      `cursor=${env.cursor}`,
      `virtual=(${env.virtual_screen.left},${env.virtual_screen.top})-(${env.virtual_screen.right},${env.virtual_screen.bottom})`,
      ...env.monitors.map(
        (m) =>
          `• ${m.name ?? "?"} pos=${m.logical_pos} logical=${m.logical_size} scale=${m.scale} physical≈${Math.round(m.physical_size_est[0])}x${Math.round(m.physical_size_est[1])}`,
      ),
    ];
    $<HTMLElement>("#env").textContent = lines.join("\n");

    // 判定：Tauri v2 的 outer_position/outer_size 本身就是物理像素，应与 Win32 逐像素相等
    const tp = env.tauri_outer_position;
    const ts = env.tauri_outer_size;
    const dur = 1;
    const all =
      Math.abs(tp[0] - r.left) <= dur &&
      Math.abs(tp[1] - r.top) <= dur &&
      Math.abs(ts[0] - w) <= dur &&
      Math.abs(ts[1] - h) <= dur;
    const verdict = $<HTMLElement>("#dpi-verdict");
    verdict.innerHTML = all
      ? `<span style="color:#4ade80">✔ Tauri 与 Win32 逐像素一致</span>：物理 ${r.left},${r.top} ${w}x${h}（逻辑 = 物理/${env.tauri_scale_factor}）`
      : `<span style="color:#ef4444">✘ 不一致</span>：Tauri ${tp.join(",")} ${ts.join("x")} vs Win32 ${r.left},${r.top} ${w}x${h}`;

    void invoke<string>("save_debug", {
      name: "env.json",
      text: JSON.stringify(env, null, 2),
    }).catch(() => undefined);

    // 抓屏默认用「本窗口物理矩形」
    $<HTMLInputElement>("#gx").value = String(r.left);
    $<HTMLInputElement>("#gy").value = String(r.top);
    $<HTMLInputElement>("#gw").value = String(w);
    $<HTMLInputElement>("#gh").value = String(h);
    log("env_info 刷新完成");
  } catch (e) {
    log(`env_info 失败：${String(e)}`);
  }
}

// ------------------------------------------------------------ T2

let clickThroughOn = false;

async function setClickThrough(on: boolean) {
  await invoke("set_click_through", { enable: on });
  clickThroughOn = on;
  const btn = $<HTMLButtonElement>("#btn-clickthrough");
  btn.classList.toggle("on", on);
  btn.textContent = on ? "穿透：开" : "穿透：关";
  if (on) {
    await invoke("start_hover_poll", { stripHeight: 48 });
    log("已开启整窗穿透 + 顶部 48px 悬停恢复");
  } else {
    await invoke("stop_hover_poll");
    $<HTMLElement>("#hover-state").textContent = "悬停状态：未启动";
    $<HTMLElement>("#strip").classList.remove("hot");
    log("已关闭穿透");
  }
}

// ------------------------------------------------------------ T4

function showShot(res: GrabResult) {
  $<HTMLElement>("#grab-meta").textContent = `${res.ms}ms · ${res.width}x${res.height} · ${res.saved_to} · ${res.note}`;
  const img = $<HTMLImageElement>("#shot");
  img.src = `data:image/png;base64,${res.png_base64}`;
  img.classList.add("show");
}

let excludeOn = false;

async function toggleExclude() {
  excludeOn = !excludeOn;
  try {
    const msg = await invoke<string>("set_exclude_from_capture", { enable: excludeOn });
    $<HTMLButtonElement>("#btn-exclude").textContent = excludeOn ? "排除抓取：开" : "排除抓取";
    $<HTMLButtonElement>("#btn-exclude").classList.toggle("on", excludeOn);
    log(msg);
  } catch (e) {
    log(`设置排除抓取失败：${String(e)}`);
  }
}

async function grab(kind: "direct" | "hidden" | "exclude") {
  const x = Number($<HTMLInputElement>("#gx").value);
  const y = Number($<HTMLInputElement>("#gy").value);
  const w = Number($<HTMLInputElement>("#gw").value);
  const h = Number($<HTMLInputElement>("#gh").value);
  try {
    const res =
      kind === "hidden"
        ? await invoke<GrabResult>("grab_hidden", { x, y, w, h })
        : await invoke<GrabResult>("grab_rect", { x, y, w, h });
    showShot(res);
    log(`抓屏完成 ${res.ms}ms → ${res.saved_to}`);
  } catch (e) {
    log(`抓屏失败：${String(e)}`);
  }
}

// ------------------------------------------------------------ 绑定

window.addEventListener("DOMContentLoaded", () => {
  renderT1();
  void refreshEnv();
  // 页面重载后把窗口状态复位成确定值（穿透/WDA 状态挂在窗口上，不随页面重载清除）
  void invoke<string>("set_exclude_from_capture", { enable: false }).catch(() => undefined);
  void invoke("stop_hover_poll").catch(() => undefined);
  void invoke("set_click_through", { enable: false }).catch(() => undefined);
  clickThroughOn = false;
  $<HTMLButtonElement>("#btn-clickthrough").textContent = "穿透：关";
  $<HTMLButtonElement>("#btn-clickthrough").classList.remove("on");

  $<HTMLButtonElement>("#btn-env").addEventListener("click", () => void refreshEnv());
  $<HTMLButtonElement>("#btn-move2").addEventListener("click", () => {
    void invoke("set_window_pos", { x: 2560, y: 400 })
      .then(() => new Promise((r) => setTimeout(r, 350)))
      .then(() => refreshEnv())
      .catch((e) => log(`移到副屏失败：${String(e)}`));
  });
  $<HTMLButtonElement>("#btn-move1").addEventListener("click", () => {
    void invoke("set_window_pos", { x: 710, y: 374 })
      .then(() => new Promise((r) => setTimeout(r, 350)))
      .then(() => refreshEnv())
      .catch((e) => log(`回主屏失败：${String(e)}`));
  });
  $<HTMLButtonElement>("#btn-close").addEventListener("click", () => void win.close());
  $<HTMLButtonElement>("#btn-clickthrough").addEventListener("click", () => {
    void setClickThrough(!clickThroughOn);
  });
  $<HTMLButtonElement>("#btn-grab").addEventListener("click", () => void grab("direct"));
  $<HTMLButtonElement>("#btn-hidden").addEventListener("click", () => void grab("hidden"));
  $<HTMLButtonElement>("#btn-exclude").addEventListener("click", () => void toggleExclude());

  void listen<{ inside: boolean; cursor: [number, number] }>("strip-hover", (ev) => {
    const { inside, cursor } = ev.payload;
    $<HTMLElement>("#strip").classList.toggle("hot", inside);
    $<HTMLElement>("#hover-state").textContent = `悬停状态：${inside ? "在工具条区（可点）" : "已点穿"} · cursor=${cursor[0]},${cursor[1]}`;
  });
});
