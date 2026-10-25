import { Pet } from "./pet.js";
import { GRID_W, GRID_H, drawPet, particleCanvas } from "./sprite.js";
import { PHRASES, TOOL_PHRASES } from "./phrases.js";
import { Brain } from "./brain.js";

const UNIT = 5;
const MAX_PARTICLES = 14;

// Running outside Tauri (plain browser) is handy for debugging the pet itself.
const tauri = window.__TAURI__;
const invoke = tauri ? tauri.core.invoke : () => Promise.resolve();
const listen = tauri ? tauri.event.listen : () => Promise.resolve();

const canvas = document.getElementById("pet");
const bubble = document.getElementById("bubble");
const ctx = canvas.getContext("2d");

const nowSec = () => performance.now() / 1000;
const pick = (items) => items[Math.floor(Math.random() * items.length)];

let dpr = 0;
let cell = 0;
let frameKey = null;
let particles = 0;
let lastPhrase = null;
let bubbleUntil = 0;
let bubbleImportant = false;
let lang = navigator.language?.toLowerCase().startsWith("ru") ? "ru" : "en";

const brain = new Brain(invoke);
const pet = new Pet(UNIT, {
  say,
  sayText,
  spawn,
  hideBubble,
});

function fitCanvas() {
  if (window.devicePixelRatio === dpr) return;
  dpr = window.devicePixelRatio || 1;
  // Whole device pixels per grid cell keep the pixel art crisp at fractional scales.
  cell = Math.max(1, Math.round(UNIT * dpr));
  canvas.width = GRID_W * cell;
  canvas.height = GRID_H * cell;
  canvas.style.width = `${canvas.width / dpr}px`;
  canvas.style.height = `${canvas.height / dpr}px`;
  pet.setUnit(cell / dpr);
  frameKey = null;
}

function linesFor(category, vars) {
  if (category.startsWith("tool:")) {
    const tools = TOOL_PHRASES[lang];
    return (vars.detail && tools[category.slice(5)]) || tools.default;
  }
  return PHRASES[lang][category] || [category];
}

// `vars` fill {placeholders}; lines that need a missing one are skipped.
function say(category, force = false, vars = {}) {
  if (category === "idle" && Math.random() < 0.75) {
    const generated = brain.take();
    if (generated) return sayText(generated, force);
  }
  const all = linesFor(category, vars);
  const usable = all.filter((p) => [...p.matchAll(/\{(\w+)\}/g)].every(([, k]) => vars[k]));
  const phrases = usable.length ? usable : all;
  const options = phrases.filter((p) => p !== lastPhrase);
  const text = pick(options.length ? options : phrases).replace(/\{(\w+)\}/g, (_, k) => vars[k] ?? "");
  sayText(text, force);
}

// Forced lines (Claude's thoughts, reports) aren't talked over by idle chatter.
function sayText(text, force = false) {
  const now = nowSec();
  if (!force && bubbleImportant && now < bubbleUntil) return;
  lastPhrase = text;
  bubbleImportant = force;
  bubble.textContent = text;
  bubble.classList.add("visible");
  bubbleUntil = now + 3.5 + text.length * 0.08;
}

function hideBubble() {
  bubble.classList.remove("visible");
  bubbleUntil = 0;
  bubbleImportant = false;
}

function spawn(kind) {
  if (particles >= MAX_PARTICLES) return;
  const s = Math.max(2, Math.round(pet.unit * 0.75 * dpr));
  const el = particleCanvas(kind, s);
  const bw = el.width / dpr;
  const bh = el.height / dpr;
  el.className = "particle";
  el.style.width = `${bw}px`;
  el.style.height = `${bh}px`;

  const headY = pet.y + pet.hopOff + pet.unit * 2 - bh;
  let sx;
  let sy;
  let drift;
  if (kind === "zzz") {
    sx = pet.x + pet.w * 0.72;
    sy = headY + pet.unit * 2;
    drift = 18;
  } else {
    sx = pet.x + pet.w / 2 - bw / 2 + (Math.random() - 0.5) * pet.w * 0.5;
    sy = headY;
    drift = (Math.random() - 0.5) * 32;
  }
  const rot = (Math.random() - 0.5) * 30;
  const rise = 40 + Math.random() * 18;
  document.body.appendChild(el);
  particles++;
  const anim = el.animate(
    [
      { transform: `translate(${sx}px, ${sy}px) rotate(${rot}deg) scale(0.5)`, opacity: 1 },
      { transform: `translate(${sx + drift}px, ${sy - rise}px) rotate(${rot}deg) scale(1)`, opacity: 0 },
    ],
    { duration: kind === "zzz" ? 1600 : 1100, easing: "ease-out" },
  );
  anim.onfinish = () => {
    el.remove();
    particles--;
  };
}

function render(now) {
  const frame = pet.frame(now);
  const key = JSON.stringify(frame);
  if (key !== frameKey) {
    frameKey = key;
    drawPet(ctx, frame, cell);
  }
  const y = pet.y + pet.hopOff;
  canvas.style.transform = `translate(${pet.x}px, ${y}px) rotate(${pet.rotation(now)}deg)`;

  if (bubbleUntil && now >= bubbleUntil) hideBubble();
  if (bubble.classList.contains("visible")) {
    const bw = bubble.offsetWidth;
    const bh = bubble.offsetHeight;
    const margin = 4;
    const bx = Math.min(Math.max(pet.x + pet.w / 2 - bw / 2, margin), window.innerWidth - bw - margin);
    let by = y + pet.unit - bh - margin;
    if (by < 0) by = y + pet.h + margin;
    bubble.style.transform = `translate(${bx}px, ${by}px)`;
  }
}

// The window covers the whole work area, so it must let clicks through
// everywhere except Clawd himself.
let passthrough = null;
function updatePassthrough() {
  const c = pet.cursor;
  const over = !!c && pet.hitTest(c.x, c.y);
  const want = !(over || pet.held);
  document.body.classList.toggle("over", over && !pet.held);
  document.body.classList.toggle("held", pet.held);
  if (want === passthrough) return;
  passthrough = want;
  invoke("set_click_through", { enabled: want }).catch((e) => console.error(e));
}

window.addEventListener("pointerdown", (e) => {
  if (e.button !== 0 || !pet.hitTest(e.clientX, e.clientY)) return;
  document.documentElement.setPointerCapture(e.pointerId);
  pet.pointerDown(e.clientX, e.clientY, nowSec());
  updatePassthrough();
});

window.addEventListener("pointermove", (e) => {
  const now = nowSec();
  pet.setCursor(e.clientX, e.clientY, now);
  pet.pointerMove(e.clientX, e.clientY, now);
  updatePassthrough();
});

window.addEventListener("pointerup", () => pet.pointerUp(nowSec()));
window.addEventListener("pointercancel", () => pet.pointerUp(nowSec(), true));
window.addEventListener("contextmenu", (e) => e.preventDefault());

// While click-through is on the webview gets no mouse events, so Rust polls the cursor for us.
listen("cursor", ({ payload }) => {
  if (pet.held) return;
  pet.setCursor(payload.x, payload.y, nowSec());
  updatePassthrough();
});
listen("recall", () => pet.recall(nowSec()));
listen("audio", ({ payload }) => pet.onAudio(payload, nowSec()));
listen("claude", ({ payload }) => pet.onClaude(payload, nowSec()));
listen("notice", ({ payload }) => {
  if (payload.phrase === "claude_found") brain.forget();
  say(payload.phrase, true);
});
listen("language", ({ payload }) => {
  lang = payload;
  brain.forget();
  say("language", true);
});
listen("context", ({ payload }) => {
  const now = nowSec();
  pet.onTyping(payload.kind === "typing" ? payload.kpm : null, now);
  const app = brain.onContext(payload, now);
  if (!app || !pet.canChat()) return;
  brain.comment(pet, now, app).then((line) => {
    if (line) sayText(line);
    else if (Math.random() < 0.3) say("window", false, { app });
  });
});

let nextRefill = 0;

function tick(ms) {
  if (ms / 1000 >= nextRefill) {
    nextRefill = ms / 1000 + 1;
    brain.maybeRefill(pet, ms / 1000);
  }
  fitCanvas();
  pet.setArea(window.innerWidth, window.innerHeight);
  pet.step(ms / 1000);
  render(ms / 1000);
  updatePassthrough();
  requestAnimationFrame(tick);
}

// Clawd greets on his first frame, so learn the saved language before starting.
const timeout = new Promise((resolve) => setTimeout(resolve, 500));
Promise.race([invoke("get_language").catch(() => null), timeout]).then((value) => {
  if (value) lang = value;
  fitCanvas();
  requestAnimationFrame(tick);
});
