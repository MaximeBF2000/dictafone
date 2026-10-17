const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open } = window.__TAURI__.dialog;

const $ = (id) => document.getElementById(id);
const idleControls = $("idle-controls");
const recControls = $("rec-controls");
const statusEl = $("status");
const pathInput = $("path");
const timerEl = $("timer");
const canvas = $("wave");
const ctx = canvas.getContext("2d");

// ---- Waveform -------------------------------------------------------------

const BAR_W = 3;
const GAP = 2;
let levels = [];
let target = 0;
let smooth = 0;
let recording = false;

function resizeCanvas() {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = canvas.clientWidth * dpr;
  canvas.height = canvas.clientHeight * dpr;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}
resizeCanvas();
window.addEventListener("resize", resizeCanvas);

function draw() {
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  const count = Math.floor(w / (BAR_W + GAP));
  ctx.clearRect(0, 0, w, h);

  if (recording) {
    smooth += (target - smooth) * 0.5;
    levels.push(smooth);
  }
  if (levels.length > count) levels = levels.slice(-count);

  const color = getComputedStyle(document.documentElement).getPropertyValue("--wave");
  const mid = h / 2 - 6;
  // Right-aligned so new samples appear on the right and scroll left.
  const offset = count - levels.length;
  for (let i = 0; i < count; i++) {
    const v = i >= offset ? levels[i - offset] : 0;
    // Perceptual scaling so quiet speech is still visible.
    const amp = Math.min(1, Math.sqrt(v) * 2.2);
    const barH = Math.max(2, amp * (mid - 2) * 2);
    const x = i * (BAR_W + GAP);
    ctx.fillStyle = color;
    ctx.globalAlpha = recording ? 0.35 + 0.65 * (i / count) : 0.25;
    ctx.beginPath();
    ctx.roundRect(x, mid - barH / 2, BAR_W, barH, BAR_W / 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;
  requestAnimationFrame(draw);
}
requestAnimationFrame(draw);

listen("level", (e) => {
  target = e.payload;
});

// ---- Timer ----------------------------------------------------------------

let startedAt = 0;
let timerHandle = null;

function tick() {
  const s = Math.floor((Date.now() - startedAt) / 1000);
  const pad = (n) => String(n).padStart(2, "0");
  const h = Math.floor(s / 3600);
  const mmss = `${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}`;
  timerEl.textContent = h ? `${h}:${mmss}` : mmss;
}

// ---- State ----------------------------------------------------------------

function setStatus(text, isError = false) {
  statusEl.textContent = text;
  statusEl.title = text;
  statusEl.classList.toggle("error", isError);
}

function setRecording(on) {
  recording = on;
  document.body.classList.toggle("recording", on);
  idleControls.classList.toggle("hidden", on);
  recControls.classList.toggle("hidden", !on);
  clearInterval(timerHandle);
  if (on) {
    levels = [];
    target = smooth = 0;
    startedAt = Date.now();
    tick();
    timerHandle = setInterval(tick, 250);
  } else {
    levels = [];
  }
}

$("start").addEventListener("click", async () => {
  setStatus("");
  try {
    await invoke("start_recording");
    setRecording(true);
  } catch (err) {
    setStatus(String(err), true);
  }
});

$("stop").addEventListener("click", async () => {
  setRecording(false);
  setStatus("Saving…");
  try {
    const path = await invoke("stop_recording");
    setStatus(`Saved ${path.split("/").pop()}`);
  } catch (err) {
    setStatus(String(err), true);
  }
});

$("cancel").addEventListener("click", async () => {
  setRecording(false);
  await invoke("cancel_recording");
  setStatus("Recording discarded");
});

// ---- Storage path ---------------------------------------------------------

async function savePath(value) {
  const path = value.trim();
  if (!path) return loadPath();
  try {
    await invoke("set_storage_path", { path });
    pathInput.value = path;
  } catch (err) {
    setStatus(String(err), true);
  }
}

async function loadPath() {
  pathInput.value = await invoke("get_storage_path");
}

pathInput.addEventListener("change", () => savePath(pathInput.value));
pathInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") pathInput.blur();
});

$("browse").addEventListener("click", async () => {
  const dir = await open({ directory: true, defaultPath: pathInput.value });
  if (typeof dir === "string") await savePath(dir);
});

// Space bar toggles start/stop, Escape cancels.
window.addEventListener("keydown", (e) => {
  if (document.activeElement === pathInput) return;
  if (e.code === "Space") {
    e.preventDefault();
    (recording ? $("stop") : $("start")).click();
  } else if (e.key === "Escape" && recording) {
    $("cancel").click();
  }
});

loadPath();
