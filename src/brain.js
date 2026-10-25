// Clawd's "brain": what's going on around him, and a stash of lines from Claude Haiku.
// The situation is described to Haiku in English; Rust tells it which language to answer in.

const CACHE_TTL = 150;
const REFILL_EVERY = 30;
const CACHE_LOW = 4;
const WINDOW_COMMENT_EVERY = 45;
const APP_FORGET_AFTER = 15 * 60;

const STATE_NAMES = {
  idle: "just standing around",
  walk: "strolling along the taskbar",
  read: "reading a book",
  code: "coding on a laptop",
  think: "thinking about something",
  dance: "dancing",
  sleep: "sleeping",
  fly: "flying through the air, someone threw him",
  hang: "hanging from the edge of the screen",
  drag: "being dragged by the mouse",
  pet: "being petted",
};

const DAY_PARTS = [
  [5, "morning"],
  [12, "afternoon"],
  [18, "evening"],
  [23, "night"],
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

  // Drops lines in the old language and asks again right away.
  forget() {
    this.cache = [];
    this.available = true;
    this.lastRequest = -Infinity;
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
    const part = [...DAY_PARTS].reverse().find(([h]) => hour >= h)?.[1] ?? "night";
    const time = date.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
    const weekday = date.toLocaleDateString("en-GB", { weekday: "long" });
    const lines = [`- It is ${time}, ${part}, ${weekday}.`];
    if (this.window) {
      const app = this.window.app ? ` (${this.window.app})` : "";
      lines.push(`- The user has this window open: "${this.window.title}"${app}.`);
    }
    if (this.kpm > 0) {
      lines.push(`- The user is typing, about ${this.kpm} key presses a minute.`);
    } else {
      const idle = Math.round((now - pet.lastUserInput) / 60);
      if (idle >= 3) {
        lines.push(`- The user hasn't touched the mouse or keyboard for about ${idle} min; they seem away or idle.`);
      }
    }
    if (pet.music) lines.push("- Music is playing.");
    lines.push(`- Clawd is ${STATE_NAMES[pet.state] || "busy with his own things"}.`);
    const projects = [...pet.claudeSessions.values()].map((s) => s.project).filter(Boolean);
    if (projects.length) lines.push(`- Claude Code is working on a task in ${projects.join(", ")}.`);
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
    const interval = this.available ? REFILL_EVERY : REFILL_EVERY * 6;
    if (this.pending || this.cache.length >= CACHE_LOW || now - this.lastRequest < interval) return;
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
    const context = this.describe(pet, now, `The user just switched to ${app}.`);
    return this.ask(context, 1).then((lines) => lines[0] ?? null);
  }
}
