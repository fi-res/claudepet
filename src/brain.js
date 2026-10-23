// Clawd's "brain": what's going on around him, and a stash of lines from Claude Haiku.

const CACHE_TTL = 180;
const REFILL_EVERY = 60;
const WINDOW_COMMENT_EVERY = 60;
const APP_FORGET_AFTER = 15 * 60;

const STATE_NAMES = {
  idle: "просто стоит",
  walk: "гуляет по панели задач",
  read: "читает книжку",
  code: "программирует за ноутбуком",
  think: "о чём-то думает",
  dance: "танцует",
  sleep: "спит",
  fly: "летит, его бросили",
  hang: "висит на краю экрана",
  drag: "его тащат мышкой",
  pet: "его гладят",
};

const DAY_PARTS = [
  [5, "утро"],
  [12, "день"],
  [18, "вечер"],
  [23, "ночь"],
];

export class Brain {
  constructor(invoke) {
    this.invoke = invoke;
    this.cache = [];
    this.lastRequest = -Infinity;
    this.pending = false;
    this.available = true;
    this.window = null;
    this.kpm = 0;
    this.appSeen = new Map();
    this.lastWindowComment = -Infinity;
  }

  // Returns an app name worth commenting on when the user switches to a new app.
  onContext(event, now) {
    if (event.kind === "typing") {
      this.kpm = event.kpm;
      return null;
    }
    if (!event.app && !event.title) {
      this.window = null;
      return null;
    }
    this.window = { app: event.app, title: event.title };
    const key = event.app || event.title;
    const seen = this.appSeen.get(key);
    this.appSeen.set(key, now);
    const fresh = seen === undefined || now - seen > APP_FORGET_AFTER;
    if (!fresh || now - this.lastWindowComment < WINDOW_COMMENT_EVERY) return null;
    this.lastWindowComment = now;
    return key;
  }

  describe(pet, now, extra = "") {
    const date = new Date();
    const hour = date.getHours();
    const part = [...DAY_PARTS].reverse().find(([h]) => hour >= h)?.[1] ?? "ночь";
    const time = date.toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" });
    const weekday = date.toLocaleDateString("ru-RU", { weekday: "long" });
    const lines = [`- Сейчас ${time}, ${part}, ${weekday}.`];
    if (this.window) {
      const app = this.window.app ? ` (${this.window.app})` : "";
      lines.push(`- У пользователя открыто окно: «${this.window.title}»${app}.`);
    }
    if (this.kpm > 0) lines.push(`- Пользователь печатает, примерно ${this.kpm} нажатий в минуту.`);
    if (pet.music) lines.push("- Играет музыка.");
    lines.push(`- Clawd сейчас ${STATE_NAMES[pet.state] || "занят своими делами"}.`);
    const projects = [...pet.claudeSessions.values()].map((s) => s.project).filter(Boolean);
    if (projects.length) lines.push(`- Claude Code сейчас работает над задачей в проекте ${projects.join(", ")}.`);
    if (extra) lines.push(`- ${extra}`);
    return lines.join("\n");
  }

  ask(context, count) {
    this.pending = true;
    this.lastRequest = performance.now() / 1000;
    return this.invoke("claude_phrases", { context, count })
      .then((lines) => {
        this.available = true;
        return lines;
      })
      .catch((e) => {
        // Not installed or switched off: stop asking for a while.
        if (String(e).includes("not found") || String(e).includes("disabled")) this.available = false;
        return [];
      })
      .finally(() => {
        this.pending = false;
      });
  }

  maybeRefill(pet, now) {
    this.cache = this.cache.filter((p) => now - p.at < CACHE_TTL);
    const interval = this.available ? REFILL_EVERY : REFILL_EVERY * 5;
    if (this.pending || this.cache.length >= 3 || now - this.lastRequest < interval) return;
    this.ask(this.describe(pet, now), 8).then((lines) => {
      const at = performance.now() / 1000;
      this.cache.push(...lines.map((text) => ({ text, at })));
    });
  }

  take() {
    return this.cache.shift()?.text ?? null;
  }

  // One line about the window the user just switched to.
  comment(pet, now, app) {
    if (this.pending || !this.available) return Promise.resolve(null);
    const context = this.describe(pet, now, `Пользователь только что переключился на ${app}.`);
    return this.ask(context, 1).then((lines) => lines[0] ?? null);
  }
}
