import { Pet } from "./pet.js";
import { GRID_W, GRID_H, drawPet, particleCanvas } from "./sprite.js";
import { PHRASES, TOOL_PHRASES } from "./phrases.js";
import { Brain } from "./brain.js";

const UNIT = 5;
const MAX_PARTICLES = 14;
const VITALS_SAVE_EVERY = 60;
const LAYOUT_EVERY = 5;
// While Clawd's computer is off he gets hungry slower, and never starves.
const OFFLINE_FOOD_DECAY = 3;
const OFFLINE_FOOD_FLOOR = 15;

// Running outside Tauri (plain browser) is handy for debugging the pet itself.
const tauri = window.__TAURI__;
const invoke = tauri ? tauri.core.invoke : () => Promise.resolve();
const listen = tauri ? tauri.event.listen : () => Promise.resolve();

const canvas = document.getElementById("pet");
const bubble = document.getElementById("bubble");
const chatForm = document.getElementById("chat");
const chatInput = document.getElementById("chat-input");
const confirmBox = document.getElementById("confirm");
const confirmCmd = document.getElementById("confirm-cmd");
const confirmWarn = document.getElementById("confirm-warn");
const confirmRun = document.getElementById("confirm-run");
const confirmCancel = document.getElementById("confirm-cancel");
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
let chatOpen = false;
let chatHistory = [];

const PLACEHOLDER = { ru: "Напиши Clawd… (Esc — закрыть)", en: "Message Clawd… (Esc to close)" };
const UI = {
  ru: { run: "Выполнить", cancel: "Отмена", danger: "⚠ Осторожно: команда может что-то удалить или остановить", running: "Выполняю…", declined: "Ладно, не буду" },
  en: { run: "Run", cancel: "Cancel", danger: "⚠ Careful: this may delete or stop something", running: "Running…", declined: "Okay, I won't" },
};
// Commands that delete, kill, power off or rewrite disks get a red warning.
const DANGER = /\b(rm|rmdir|del|erase|kill|killall|pkill|taskkill|stop-process|remove-item|shutdown|reboot|poweroff|halt|mkfs|format|dd|chmod|chown|truncate|shred|wipefs|diskpart|reg\s+delete)\b|>\s*\/dev\/|:\(\)\s*\{/i;
let pendingCommand = null;

const brain = new Brain(invoke);
const pet = new Pet(UNIT, {
  say,
  sayText,
  spawn,
  hideBubble,
  cross: crossEdge,
});

// In a plain browser, expose the pet for poking at from the dev console.
if (!tauri) window.clawd = pet;

// After a move to another monitor the page may report the old size for a frame
// or two; use the size Rust gave us until it catches up.
let areaHold = null;

function crossEdge(side, y) {
  invoke("cross_edge", { side, y })
    .then((layout) => {
      if (!layout) return pet.crossFailed(side);
      areaHold = { width: layout.width, height: layout.height, until: nowSec() + 1.5 };
      pet.arrive(side, layout, nowSec());
    })
    .catch(() => pet.crossFailed(side));
}

function refreshLayout() {
  invoke("monitor_layout")
    .then((layout) => layout && pet.setEdges(layout))
    .catch(() => {});
}

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
// `hold` keeps the bubble up for at least that many seconds.
function sayText(text, force = false, hold = 0) {
  const now = nowSec();
  if (!force && bubbleImportant && now < bubbleUntil) return;
  lastPhrase = text;
  bubbleImportant = force;
  bubble.textContent = text;
  bubble.classList.add("visible");
  bubbleUntil = now + Math.max(hold, 3.5 + text.length * 0.08);
}

// One line from a category, for text that's stored rather than spoken right away.
function phrase(category) {
  return pick(PHRASES[lang][category] || [category]);
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
    const lift = chatOpen ? chatForm.offsetHeight + margin + (pendingCommand ? confirmBox.offsetHeight + margin : 0) : 0;
    let by = y + pet.unit - bh - margin - lift;
    if (by < 0) by = y + pet.h + margin;
    bubble.style.transform = `translate(${bx}px, ${by}px)`;
  }
  if (chatOpen) {
    const cw = chatForm.offsetWidth;
    const cx = Math.min(Math.max(pet.x + pet.w / 2 - cw / 2, 4), window.innerWidth - cw - 4);
    const cy = Math.max(y + pet.unit - chatForm.offsetHeight - 4, 4);
    chatForm.style.transform = `translate(${cx}px, ${cy}px)`;
    if (pendingCommand) {
      const bw = confirmBox.offsetWidth;
      const bx = Math.min(Math.max(pet.x + pet.w / 2 - bw / 2, 4), window.innerWidth - bw - 4);
      const by = Math.max(cy - confirmBox.offsetHeight - 4, 4);
      confirmBox.style.transform = `translate(${bx}px, ${by}px)`;
    }
  }
}

// The window covers the whole work area, so it must let clicks through
// everywhere except Clawd himself.
let passthrough = null;
function updatePassthrough() {
  const c = pet.cursor;
  const over = !!c && (pet.hitTest(c.x, c.y) || overChat(c.x, c.y));
  const want = !(over || pet.held);
  document.body.classList.toggle("over", over && !pet.held);
  document.body.classList.toggle("held", pet.held);
  if (want === passthrough) return;
  passthrough = want;
  invoke("set_click_through", { enabled: want }).catch((e) => console.error(e));
}

function overChat(x, y) {
  if (!chatOpen) return false;
  const inside = (el) => {
    const r = el.getBoundingClientRect();
    return r.width > 0 && x >= r.left - 4 && x <= r.right + 4 && y >= r.top - 4 && y <= r.bottom + 4;
  };
  return inside(chatForm) || (pendingCommand !== null && inside(confirmBox));
}

function openChat() {
  if (!chatOpen) {
    chatOpen = true;
    chatInput.placeholder = PLACEHOLDER[lang];
    chatInput.value = "";
    chatInput.disabled = false;
    chatForm.classList.add("open");
    pet.startChat(nowSec());
  }
  // The overlay starts unfocused; the box needs keyboard focus to type into.
  invoke("focus_window")
    .catch(() => {})
    .finally(() => chatInput.focus());
}

function closeChat() {
  if (!chatOpen) return;
  if (pendingCommand) cancelCommand();
  chatOpen = false;
  chatForm.classList.remove("open");
  chatInput.blur();
  pet.endChat();
}

// What to say when the chat call fails, by what went wrong.
function chatErrorPhrase(error) {
  const text = String(error);
  if (/\bdisabled\b/.test(text)) return "chat_error_disabled";
  if (/not found/i.test(text)) return "chat_error_missing";
  if (/authenticat|log ?in|oauth|credential|unauthori|401/i.test(text)) return "chat_error_auth";
  if (/timed out/i.test(text)) return "chat_error_timeout";
  if (/rate.?limit|usage limit|limit reached|429/i.test(text)) return "chat_error_limit";
  return "chat_error";
}

const clockTime = (unix) => new Date(unix * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

function setReminder(minutes, text) {
  return invoke("add_reminder", { minutes, text: text || phrase("remind_default") }).then((at) => at || 0);
}

const UNITS = { s: 1 / 60, m: 1, h: 60 };
const REMIND_IN = /(?:напомни|remind)\S*\s+(?:мне\s+|me\s+)?(?:через|in)\s+(\d+(?:[.,]\d+)?)\s*(сек\S*|с|sec\S*|s\b|мин\S*|м|min\S*|m\b|час\S*|ч|hour\S*|h\b)\s*/i;
const REMIND_AT = /(?:напомни|remind)\S*\s+(?:мне\s+|me\s+)?(?:в|at)\s+(\d{1,2})[:.](\d{2})\s*/i;
const POMODORO = /помодор|pomodoro/i;
const CANCEL = /(отмен|сбрось|удали|убери)\S*\s.{0,20}напомин|(cancel|clear|delete)\s.{0,20}remind/i;
const FORGET = /^\s*(забудь\s+(всё|все|наш\S* разговор\S*)|forget\s+(everything|our\s+chats?))/i;

function reminderText(message, match) {
  const rest = message.replace(match[0], " ").replace(/^\s*(напомни\S*|remind)\s*(мне|me)?/i, "");
  return rest.replace(/^\s*(о том,? что|что|чтобы|про|о|to|about|that)\s+/i, "").replace(/^[\s,.:;—-]+|[\s.!]+$/g, "");
}

// Reminders, pomodoro and "forget everything" are handled here without asking
// Haiku, so they work offline and cost no tokens. Returns true when handled.
function localIntent(message) {
  if (CANCEL.test(message)) {
    invoke("clear_reminders").catch(() => {});
    say("reminders_cleared", true);
    return true;
  }
  if (FORGET.test(message)) {
    invoke("forget_memory").catch(() => {});
    chatHistory = [];
    say("forgot", true);
    return true;
  }
  if (POMODORO.test(message)) {
    setReminder(25, phrase("pomodoro_break"));
    setReminder(30, phrase("pomodoro_back"));
    say("pomodoro_set", true);
    return true;
  }
  const inMatch = message.match(REMIND_IN);
  if (inMatch) {
    const amount = parseFloat(inMatch[1].replace(",", "."));
    const unit = inMatch[2].toLowerCase();
    const scale = /^(с|s)/.test(unit) ? UNITS.s : /^(ч|h)/.test(unit) ? UNITS.h : UNITS.m;
    setReminder(amount * scale, reminderText(message, inMatch)).then((at) => say("remind_set", true, { time: clockTime(at) }));
    return true;
  }
  const atMatch = message.match(REMIND_AT);
  if (atMatch) {
    const target = new Date();
    target.setHours(+atMatch[1], +atMatch[2], 0, 0);
    if (target <= new Date()) target.setDate(target.getDate() + 1);
    const minutes = (target - new Date()) / 60000;
    setReminder(minutes, reminderText(message, atMatch)).then((at) => say("remind_set", true, { time: clockTime(at) }));
    return true;
  }
  return false;
}

function saveRecent() {
  invoke("save_recent", { recent: chatHistory.slice(-6) }).catch(() => {});
}

// Sends a message to Haiku; the answer is spoken and may offer a command to confirm.
function askClawd(message) {
  chatInput.disabled = true;
  pet.awaitingReply = true;
  sayText("…", true);
  const context = brain.describe(pet, nowSec());
  invoke("claude_chat", { context, history: chatHistory, message })
    .then((reply) => {
      if (!reply || (!reply.text && !reply.command)) throw new Error("empty reply");
      const note = reply.command ? `${reply.text} [offered to run: ${reply.command}]` : reply.text;
      chatHistory.push(["user", message], ["clawd", note]);
      chatHistory = chatHistory.slice(-16);
      saveRecent();
      if (reply.text) sayText(reply.text, true);
      pet.hop(260);
      if (reply.remind) {
        setReminder(reply.remind.minutes, reply.remind.text).then((at) => {
          if (!reply.text) say("remind_set", true, { time: clockTime(at) });
        });
      }
      if (reply.memory) {
        invoke("remember", { fact: reply.memory }).catch(() => {});
        spawn("spark");
      }
      if (reply.command) showConfirm(reply.command);
    })
    .catch((e) => say(chatErrorPhrase(e), true))
    .finally(() => {
      pet.awaitingReply = false;
      chatInput.disabled = false;
      if (chatOpen && !pendingCommand) chatInput.focus();
    });
}

chatForm.addEventListener("submit", (e) => {
  e.preventDefault();
  const message = chatInput.value.trim();
  if (!message) return closeChat();
  if (chatInput.disabled || pendingCommand) return;
  chatInput.value = "";
  pet.cheer(3);
  if (localIntent(message)) {
    pet.hop(260);
    return;
  }
  askClawd(message);
});

function showConfirm(command) {
  pendingCommand = command;
  confirmCmd.textContent = command;
  confirmWarn.textContent = UI[lang].danger;
  confirmRun.textContent = UI[lang].run;
  confirmCancel.textContent = UI[lang].cancel;
  confirmBox.classList.toggle("danger", DANGER.test(command));
  confirmBox.classList.add("open");
  // Cancel is the default: a stray Enter must never run anything.
  confirmCancel.focus();
}

function hideConfirm() {
  pendingCommand = null;
  confirmBox.classList.remove("open", "danger");
}

function cancelCommand() {
  if (!pendingCommand) return;
  chatHistory.push(["user", `(declined to run: ${pendingCommand})`]);
  hideConfirm();
  sayText(UI[lang].declined, true);
  if (chatOpen) chatInput.focus();
}

confirmCancel.addEventListener("click", cancelCommand);
confirmRun.addEventListener("click", () => {
  const command = pendingCommand;
  if (!command) return;
  hideConfirm();
  chatInput.disabled = true;
  pet.awaitingReply = true;
  sayText(UI[lang].running, true);
  invoke("run_command", { command })
    .then((result) => {
      const status = result.timed_out ? "it timed out after 30 s" : `exit code ${result.code}`;
      const output = result.output || "(no output)";
      askClawd(
        `(system) The user confirmed and the command \`${command}\` ran: ${status}. Output:\n${output}\n` +
          "Tell the user briefly what happened. Offer another command only if it's the natural next step.",
      );
    })
    .catch((e) => {
      pet.awaitingReply = false;
      chatInput.disabled = false;
      sayText(String(e), true);
    });
});
confirmBox.addEventListener("keydown", (e) => {
  if (e.key === "Escape") cancelCommand();
});

chatInput.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeChat();
});
// Clicking into another app takes focus away: put the boxes away too.
window.addEventListener("blur", () => {
  if (!chatInput.disabled) closeChat();
});

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
// Right-click on Clawd opens the chat box.
window.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  if (pet.hitTest(e.clientX, e.clientY)) openChat();
});

// While click-through is on the webview gets no mouse events, so Rust polls the cursor for us.
listen("cursor", ({ payload }) => {
  if (pet.held) return;
  pet.setCursor(payload.x, payload.y, nowSec());
  updatePassthrough();
});
listen("recall", () => {
  pet.recall(nowSec());
  setTimeout(refreshLayout, 300);
});
listen("platform", ({ payload }) => pet.setPlatform(payload, nowSec()));
listen("remind", ({ payload }) => pet.remind(payload.text, nowSec()));
listen("feed", () => pet.feed(nowSec()));
listen("memory_cleared", () => {
  chatHistory = [];
});
listen("chat_open", () => openChat());
listen("audio", ({ payload }) => pet.onAudio(payload, nowSec()));
listen("claude", ({ payload }) => pet.onClaude(payload, nowSec()));
listen("notice", ({ payload }) => {
  if (payload.phrase === "claude_found") brain.forget();
  if (payload.phrase === "forgot") chatHistory = [];
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
let nextVitalsSave = VITALS_SAVE_EVERY;
let nextLayout = LAYOUT_EVERY;

function tick(ms) {
  const now = ms / 1000;
  if (now >= nextRefill) {
    nextRefill = now + 1;
    brain.maybeRefill(pet, now);
  }
  if (now >= nextVitalsSave) {
    nextVitalsSave = now + VITALS_SAVE_EVERY;
    invoke("save_vitals", pet.vitals).catch(() => {});
  }
  if (now >= nextLayout) {
    nextLayout = now + LAYOUT_EVERY;
    refreshLayout();
  }
  fitCanvas();
  const caughtUp = areaHold && Math.abs(window.innerWidth - areaHold.width) < 2;
  if (areaHold && (caughtUp || now > areaHold.until)) areaHold = null;
  if (areaHold) pet.setArea(areaHold.width, areaHold.height);
  else pet.setArea(window.innerWidth, window.innerHeight);
  pet.step(ms / 1000);
  render(ms / 1000);
  updatePassthrough();
  requestAnimationFrame(tick);
}

// Mood and fullness from last time, minus what wore off while Clawd was closed.
function loadVitals() {
  return invoke("get_vitals")
    .then((saved) => {
      if (!saved) return;
      const hours = Math.max(0, Date.now() / 1000 - saved.at) / 3600;
      const food = Math.max(Math.min(saved.food, OFFLINE_FOOD_FLOOR), saved.food - hours * OFFLINE_FOOD_DECAY);
      pet.setVitals({ mood: Math.max(40, saved.mood - hours), food });
    })
    .catch(() => {})
    .finally(() => invoke("save_vitals", pet.vitals).catch(() => {}));
}

function loadMemory() {
  return invoke("get_memory")
    .then((memory) => {
      if (memory && Array.isArray(memory.recent)) chatHistory = memory.recent;
    })
    .catch(() => {});
}

// Clawd greets on his first frame, so learn the saved language before starting.
const timeout = new Promise((resolve) => setTimeout(resolve, 500));
const ready = Promise.all([invoke("get_language").catch(() => null), loadVitals(), loadMemory()]);
Promise.race([ready, timeout]).then((values) => {
  const value = values && values[0];
  if (value) lang = value;
  fitCanvas();
  refreshLayout();
  requestAnimationFrame(tick);
});
