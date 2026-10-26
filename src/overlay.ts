import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Geo = { x: number; y: number; w: number; h: number; dpr: number };

const sel = document.getElementById("sel") as HTMLDivElement;
const sizeTag = document.getElementById("size") as HTMLDivElement;
const dim = document.getElementById("dim") as HTMLDivElement;

let geo: Geo = { x: 0, y: 0, w: 0, h: 0, dpr: 1 };
let dragging = false;
let startX = 0;
let startY = 0;

async function refreshGeo() {
  try {
    geo = await invoke<Geo>("overlay_geo");
  } catch {
    /* 拿不到就沿用旧值 */
  }
}

function rectFromDrag(x: number, y: number): { left: number; top: number; w: number; h: number } {
  const left = Math.min(startX, x);
  const top = Math.min(startY, y);
  return { left, top, w: Math.abs(x - startX), h: Math.abs(y - startY) };
}

function paint(left: number, top: number, w: number, h: number) {
  sel.style.display = "block";
  sel.style.left = `${left}px`;
  sel.style.top = `${top}px`;
  sel.style.width = `${w}px`;
  sel.style.height = `${h}px`;

  const physW = Math.round(w * geo.dpr);
  const physH = Math.round(h * geo.dpr);
  sizeTag.style.display = "block";
  sizeTag.textContent = `${physW} × ${physH}`;
  const tagTop = top > 26 ? top - 24 : top + h + 6;
  sizeTag.style.left = `${left}px`;
  sizeTag.style.top = `${tagTop}px`;
}

function reset() {
  dragging = false;
  sel.style.display = "none";
  sizeTag.style.display = "none";
}

window.addEventListener("mousedown", (e) => {
  if (e.button !== 0) return;
  dragging = true;
  startX = e.clientX;
  startY = e.clientY;
  void invoke("debug_log", { msg: `overlay down css=${e.clientX},${e.clientY}` }).catch(() => {});
  paint(startX, startY, 0, 0);
});

window.addEventListener("mousemove", (e) => {
  if (!dragging) return;
  const r = rectFromDrag(e.clientX, e.clientY);
  paint(r.left, r.top, r.w, r.h);
});

window.addEventListener("mouseup", async (e) => {
  if (!dragging) return;
  const r = rectFromDrag(e.clientX, e.clientY);
  dragging = false;

  // 太小的框当作误触，直接取消
  if (r.w < 6 || r.h < 6) {
    reset();
    await invoke("cancel_selection").catch(() => {});
    return;
  }

  // CSS 像素 → 屏幕物理像素（映射窗口原点 + DPI 缩放）
  const x = Math.round(geo.x + r.left * geo.dpr);
  const y = Math.round(geo.y + r.top * geo.dpr);
  const w = Math.round(r.w * geo.dpr);
  const h = Math.round(r.h * geo.dpr);

  dim.style.opacity = "0";
  reset();
  void invoke("debug_log", {
    msg: `overlay up css=${r.left},${r.top} ${r.w}x${r.h} -> phys=${x},${y} ${w}x${h} dpr=${geo.dpr}`,
  }).catch(() => {});
  try {
    await invoke("finish_selection", { x, y, w, h });
  } catch {
    /* 错误会通过 ocr-failed 事件回到主窗 */
  }
});

window.addEventListener("keydown", async (e) => {
  if (e.key === "Escape") {
    reset();
    await invoke("cancel_selection").catch(() => {});
  }
});

window.addEventListener("dblclick", async () => {
  reset();
  await invoke("cancel_selection").catch(() => {});
});

async function boot() {
  const log = (m: string) => invoke("debug_log", { msg: `overlay: ${m}` }).catch(() => {});
  window.addEventListener("error", (e) => log(`error ${e.message}`));
  log("boot");

  // 注意：不能等 overlay-ready 事件才显示暗层——遮罩窗口的 webview 常常比
  // Rust 那边的 emit_to 晚一步加载，事件就丢了，窗口会"显示了但全透明"，
  // 表现就是按了 Alt+Q 什么都没发生，而且鼠标点哪都没反应（被透明窗挡住）。
  await refreshGeo();
  log(`geo ${geo.x},${geo.y} ${geo.w}x${geo.h} dpr=${geo.dpr}`);
  await listen<Geo>("overlay-ready", (e) => {
    geo = e.payload;
    reset();
  });
  dim.style.opacity = "1";
  reset();
  const r = dim.getBoundingClientRect();
  const cs = getComputedStyle(dim);
  log(
    `ready viewport=${window.innerWidth}x${window.innerHeight} dimRect=${Math.round(r.width)}x${Math.round(r.height)} bg=${cs.backgroundColor} opacity=${cs.opacity} z=${cs.zIndex}`,
  );
}

boot().catch(() => {});
