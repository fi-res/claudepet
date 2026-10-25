// Clawd's behaviour: state machine and physics. Units are CSS pixels and seconds.
import { GRID_W, GRID_H, SPINNER_LENGTH } from "./sprite.js";

const GRAVITY = 2600;
const WALK_SPEED = 42;
const SLEEP_AFTER = 180;
// How often Clawd says something of his own while idle.
const CHATTER_EVERY = [18, 35];
// Dozing off on his own lasts this long (minutes) at most.
const DEEP_SLEEP = [20, 40];
// Coming back to the computer after this long wakes him up.
const AWAY_AFTER = 300;
const HOVER_WAKE = 1.5;
const ATTENTION_RADIUS = 360;
const ATTENTION_TIME = 2.5;

// Calm activities Clawd picks by himself: [state, min seconds, max seconds].
// Some only make sense at certain times; `activityAllowed` decides.
const ACTIVITIES = [
  ["read", 25, 60],
  ["code", 25, 60],
  ["think", 8, 15],
  ["sleep", 25, 50],
  ["draw", 25, 45],
  ["coffee", 15, 25],
  ["pizza", 14, 20],
  ["chase", 10, 15],
];
const ACTIVITY_STATES = new Set(["read", "code", "think", "draw", "coffee", "pizza"]);
const CALM_STATES = ["idle", "walk", "read", "code", "think", "dance", "draw", "coffee", "pizza", "chase"];
const SNACK_EVERY = 2 * 3600;
const CHASE_SPEED = 150;
const PIZZA_BITES = 4;
const PAINT_DOTS = 16;
// Tamagotchi needs, in points per hour.
const FOOD_DECAY = 10;
const LONELY_AFTER = 20 * 60;
const LONELY_DECAY = 6;
const HUNGRY = 25;
const SAD = 30;
// Nudge to take a break after this long at the computer without a 5-minute pause.
const BREAK_AFTER = 60 * 60;
const BREAK_GAP = 5 * 60;
const BREAK_NAG_EVERY = 20 * 60;
// A reminder rings right away and then every REMIND_REPEAT seconds, REMIND_TIMES
// times in all; Clawd keeps jumping with his alarm clock for REMIND_LOUD seconds.
const REMIND_REPEAT = 30;
const REMIND_TIMES = 5;
const REMIND_LOUD = 180;
const SESSION_TIMEOUT = 10 * 60;
const TOOL_SAY_EVERY = 8;
const FAST_TYPING_KPM = 250;

const clamp = (v, lo, hi) => (hi < lo ? lo : v < lo ? lo : v > hi ? hi : v);
const clamp100 = (v) => clamp(v, 0, 100);
const rand = (a, b) => a + Math.random() * (b - a);
const pick = (items) => items[Math.floor(Math.random() * items.length)];

// Seasonal outfit for a date: a Santa hat around New Year, a pumpkin around Halloween.
export function season(date = new Date()) {
  const forced = typeof location !== "undefined" && new URLSearchParams(location.search).get("season");
  if (forced) return forced === "none" ? null : forced;
  const [month, day] = [date.getMonth() + 1, date.getDate()];
  if ((month === 12 && day >= 15) || (month === 1 && day <= 10)) return "santa";
  if ((month === 10 && day >= 20) || (month === 11 && day <= 2)) return "pumpkin";
  return null;
}

export class Pet {
  // `fx` receives side effects: say(category, force, vars), sayText(text, force, hold, kind)
  // and spawn(kind); `kind` ranks the line so chatter can't cover the conversation.
  constructor(unit, fx) {
    this.fx = fx;
    this.setUnit(unit);
    const now = performance.now() / 1000;
    this.state = "idle";
    this.x = 0;
    this.y = 0;
    this.vx = 0;
    this.vy = 0;
    this.ready = false;
    this.lastTick = now;
    this.lastInteraction = now;
    this.nextAction = now + 4;
    this.nextBlink = now + 2;
    this.blinkUntil = 0;
    this.look = 0;
    this.expr = null;
    this.exprUntil = 0;
    this.squashUntil = 0;
    this.waveUntil = 0;
    this.hopActive = false;
    this.hopOff = 0;
    this.hopVy = 0;
    this.walkTarget = 0;
    this.walkIn = false;
    this.touch = null;
    this.lastTap = 0;
    this.recentTaps = [];
    this.nextHeart = 0;
    this.nextZ = 0;
    this.hangStart = 0;
    this.hangUntil = 0;
    this.activityStart = 0;
    this.activityUntil = 0;
    this.nextActivity = now + 15;
    this.nextActivityEvent = 0;
    this.flipUntil = 0;
    this.typingPause = 0;
    this.napUntil = 0;
    this.lastUserInput = now;
    this.chatting = false;
    this.awaitingReply = false;
    this.nextChatter = now + rand(CHATTER_EVERY[0], CHATTER_EVERY[1]);
    this.music = false;
    this.musicReacted = false;
    this.phones = false;
    this.nextMusicSwitch = 0;
    this.beatAt = -10;
    this.beatCount = 0;
    this.nextNote = 0;
    this.lastStartle = 0;
    this.claudeSessions = new Map();
    this.claudeWork = false;
    this.nextToolSay = 0;
    this.pendingReport = null;
    this.reportAt = 0;
    this.nextSessionSweep = 0;
    this.fastTypingSince = 0;
    this.lastTypingComment = -Infinity;
    this.cursor = null;
    this.hoverSince = 0;
    this.bounds = { minX: 0, maxX: 0, minY: 0, maxY: 0, width: 0, height: 0 };
    // Other monitors beside this one, and a pending move to one of them.
    this.edges = { left: false, right: false };
    this.crossing = null;
    this.walkCross = false;
    // The focused window's top edge, and whether Clawd stands on it.
    this.platform = null;
    this.onPlatform = false;
    this.leaping = false;
    // Activities with props.
    this.lastSnack = { coffee: -Infinity, pizza: -Infinity };
    this.fed = false;
    this.sipUntil = 0;
    this.nextSteam = 0;
    this.bites = 0;
    this.biteUntil = 0;
    this.paint = 0;
    this.chaseIdle = 0;
    this.stretchUntil = 0;
    // Mood and fullness, 0..100.
    this.vitals = { mood: 70, food: 70 };
    this.nextVitals = now + 1;
    this.workSince = 0;
    this.lastBreakNag = -Infinity;
    this.reminder = null;
    this.limitWarned = new Map();
    // The cursor is over a box above Clawd (chat, reminder card).
    this.uiHover = false;
    // Claude plan usage from the status line, when Claude Code has sent it.
    this.limits = null;
  }

  setUnit(unit) {
    const oldW = this.w || 0;
    const oldH = this.h || 0;
    this.unit = unit;
    this.w = GRID_W * unit;
    this.h = GRID_H * unit;
    if (oldW) {
      this.x += (oldW - this.w) / 2;
      this.y += oldH - this.h;
    }
  }

  setArea(width, height) {
    this.bounds = {
      minX: 0,
      maxX: Math.max(0, width - this.w),
      minY: 0,
      maxY: Math.max(0, height - this.h),
      width,
      height,
    };
  }

  // Where Clawd's feet go: the window he sits on, or the taskbar.
  floorY() {
    return this.onPlatform && this.platform ? this.platform.y - this.h : this.bounds.maxY;
  }

  // How far left and right he can walk where he stands.
  walkRange() {
    const { minX, maxX } = this.bounds;
    const p = this.platform;
    if (!this.onPlatform || !p) return [minX, maxX];
    return [Math.max(minX, p.x - this.w * 0.25), Math.min(maxX, p.x + p.w - this.w * 0.75)];
  }

  // The focused window moved, resized or changed. Rides along when it's the same
  // window being dragged; falls off when it's gone or another window took focus.
  setPlatform(p, now) {
    const b = this.bounds;
    if (p && (p.y < this.h + 4 || p.y > b.maxY - 8 || p.x + p.w < 40 || p.x > b.width - 40)) p = null;
    const old = this.platform;
    this.platform = p;
    if (!this.onPlatform || this.held) return;
    const sameWindow = p && old && (Math.abs(p.w - old.w) < 2 || Math.abs(p.x - old.x) < 2);
    if (!sameWindow) {
      this.onPlatform = false;
      if (Math.random() < 0.6) this.fx.say("perch_fall");
      this.setExpr("surprised", 0.8, now);
      return;
    }
    this.x += p.x - old.x;
    this.y = p.y - this.h;
  }

  setEdges(edges) {
    this.edges = { left: !!edges.left, right: !!edges.right };
  }

  // Asks to move to the monitor on `side`; the page calls arrive() or crossFailed().
  cross(side, now) {
    if (this.crossing) return;
    this.crossing = { side, at: now };
    this.fx.cross(side, this.y);
  }

  arrive(side, layout, now) {
    this.crossing = null;
    this.setArea(layout.width, layout.height);
    this.setEdges(layout);
    this.platform = null;
    this.onPlatform = false;
    this.lastInteraction = now;
    if (this.held) return;
    const { minX, maxX, maxY } = this.bounds;
    if (this.state === "fly") {
      this.x = side === "right" ? minX : maxX;
      this.y = clamp(this.y, 0, maxY);
      return;
    }
    // Step in from the edge he came through.
    this.x = side === "right" ? minX - this.w * 0.6 : maxX + this.w * 0.6;
    this.y = maxY;
    this.state = "walk";
    this.walkCross = false;
    this.walkTarget = side === "right" ? minX + rand(60, 160) : maxX - rand(60, 160);
    if (Math.random() < 0.5) this.fx.say("monitor_cross");
  }

  crossFailed(side) {
    this.crossing = null;
    this.edges[side] = false;
    this.walkCross = false;
  }

  setVitals(vitals) {
    this.vitals = { mood: clamp100(vitals.mood), food: clamp100(vitals.food) };
  }

  cheer(amount) {
    this.vitals.mood = clamp100(this.vitals.mood + amount);
  }

  stepVitals(now) {
    if (now < this.nextVitals) return;
    const dt = now - this.nextVitals + 1;
    this.nextVitals = now + 1;
    const v = this.vitals;
    v.food = clamp100(v.food - (FOOD_DECAY * dt) / 3600);
    const lonely = now - this.lastInteraction > LONELY_AFTER && !this.chatting;
    const drain = (lonely ? LONELY_DECAY : 0) + (v.food < HUNGRY ? 4 : 0);
    v.mood = clamp100(v.mood - (drain * dt) / 3600);
  }

  get hungry() {
    return this.vitals.food < HUNGRY;
  }

  get sad() {
    return this.vitals.mood < SAD;
  }

  // Tray "Feed Clawd": a slice of pizza, unless he's full.
  feed(now) {
    if (this.state === "sleep") this.wakeUp(now, "wake");
    if (this.vitals.food > 90) {
      this.setExpr("happy", 1.2, now);
      this.fx.say("feed_full", true);
      return;
    }
    if (this.held || ["fly", "hang"].includes(this.state) || this.claudeWork) {
      this.fx.say("feed_later", true);
      return;
    }
    this.fed = true;
    this.beginActivity("pizza", 12, 16, now);
    this.fx.say("feed_thanks", true);
  }

  // A reminder went off: Clawd drops what he's doing and rings an alarm clock
  // until the user dismisses the reminder card (or clicks him).
  remind(text, now) {
    if (this.state === "sleep") this.wakeUp(now, "wake");
    const busy = ACTIVITY_STATES.has(this.state) && !this.claudeWork;
    if (busy || this.state === "dance" || this.state === "chase") this.state = "idle";
    this.lastInteraction = now;
    this.reminder = { text, since: now, count: 0, next: now, nextHop: now + 0.3 };
    this.fx.say("remind_ring", true);
  }

  ackReminder(now) {
    if (!this.reminder) return;
    this.reminder = null;
    this.setExpr("happy", 1.2, now);
  }

  stepReminder(now) {
    const r = this.reminder;
    if (!r) return;
    this.lastInteraction = now;
    if (now >= r.next && r.count < REMIND_TIMES) {
      r.count++;
      r.next = now + REMIND_REPEAT;
      this.fx.ring();
    }
    const free = this.state === "idle" || this.state === "walk";
    if (now >= r.nextHop && now - r.since < REMIND_LOUD && free && !this.uiHover) {
      r.nextHop = now + rand(1, 1.5);
      this.hop(440);
      this.fx.spawn("alert");
    }
  }

  // A long stretch at the computer without a pause earns a nudge to stretch.
  stepBreak(now) {
    if (now - this.lastUserInput > BREAK_GAP) {
      this.workSince = 0;
      return;
    }
    if (!this.workSince) this.workSince = this.lastUserInput;
    const due = now - this.workSince > BREAK_AFTER && now - this.lastBreakNag > BREAK_NAG_EVERY;
    if (!due || !this.canChat() || this.claudeWork || this.chatting) return;
    this.lastBreakNag = now;
    this.stretchUntil = now + 2.5;
    this.fx.say("break", true, { minutes: Math.round((now - this.workSince) / 60) });
  }

  onLimits(event, now) {
    const five = event.five_hour;
    if (!five) return false;
    const reset = five.resets_at ? new Date(five.resets_at * 1000) : null;
    const key = reset ? reset.getTime() : 0;
    const level = five.used >= 95 ? 95 : five.used >= 80 ? 80 : 0;
    if (!level || (this.limitWarned.get(key) || 0) >= level) return false;
    this.limitWarned.set(key, level);
    const time = reset ? reset.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : "";
    this.setExpr("surprised", 1, now);
    this.fx.say(level >= 95 ? "limit_high" : "limit_warn", true, { pct: Math.round(five.used), time });
    return true;
  }

  get held() {
    return this.state === "press" || this.state === "drag" || this.state === "pet";
  }

  hitTest(px, py) {
    const u = this.unit;
    const top = this.y + this.hopOff;
    return px >= this.x + u && px <= this.x + this.w - u && py >= top + u * 2 && py <= top + this.h;
  }

  setCursor(x, y, now) {
    const c = this.cursor;
    if (c && Math.abs(c.x - x) < 0.5 && Math.abs(c.y - y) < 0.5) return;
    this.cursor = { x, y, t: now };
    this.noteUserInput(now);
  }

  // The user moved the mouse or typed; after a long absence that wakes Clawd up.
  noteUserInput(now) {
    if (this.state === "sleep" && now - this.lastUserInput > AWAY_AFTER) this.wakeUp(now, "welcome_back");
    this.lastUserInput = now;
  }

  startChat(now) {
    this.chatting = true;
    this.lastInteraction = now;
    if (this.state === "sleep") {
      this.wakeUp(now, "chat_hello");
      return;
    }
    // Put the book or laptop away to talk, unless he's busy helping Claude.
    if (ACTIVITY_STATES.has(this.state) && !this.claudeWork) this.state = "idle";
    this.look = 0;
    this.fx.say("chat_hello", true);
  }

  endChat() {
    this.chatting = false;
    this.awaitingReply = false;
  }

  wakeUp(now, phrase) {
    this.napUntil = 0;
    this.state = "idle";
    this.lastInteraction = now;
    this.nextAction = now + rand(2, 4);
    this.hoverSince = 0;
    this.hop(300);
    this.fx.say(phrase, true);
  }

  step(now) {
    const dt = clamp(now - this.lastTick, 0, 0.05);
    this.lastTick = now;
    const b = this.bounds;

    if (!this.ready) {
      this.x = b.minX + (b.maxX - b.minX) * 0.8;
      this.y = b.maxY;
      this.ready = true;
      this.hop(420);
      this.fx.say(season() === "pumpkin" ? "hello_halloween" : season() === "santa" ? "hello_newyear" : "hello");
    }

    // Waiting for the overlay to move to the next monitor.
    if (this.crossing) {
      if (now - this.crossing.at > 1.5) this.crossFailed(this.crossing.side);
      else return;
    }
    this.stepVitals(now);
    this.stepReminder(now);
    this.stepBreak(now);
    if (this.onPlatform && !this.held) {
      const p = this.platform;
      const cx = this.x + this.w / 2;
      if (!p || cx < p.x - 2 || cx > p.x + p.w + 2) this.onPlatform = false;
    }

    if (this.state === "press" && this.touch && now - this.touch.t0 >= 0.4) {
      this.startPet(now);
    }
    if (this.pendingReport && now >= this.reportAt) {
      this.fx.sayText(this.pendingReport, true);
      this.pendingReport = null;
    }
    if (now >= this.nextSessionSweep) {
      this.nextSessionSweep = now + 5;
      for (const [id, session] of this.claudeSessions) {
        if (now - session.last > SESSION_TIMEOUT) this.claudeSessions.delete(id);
      }
      if (!this.claudeSessions.size && this.claudeWork && this.state === "code") this.finishActivity(now);
    }

    switch (this.state) {
      case "pet":
        if (now >= this.nextHeart) {
          this.nextHeart = now + 0.45;
          this.fx.spawn("heart");
        }
        break;
      case "fly":
        this.stepFly(dt, now);
        break;
      case "hang":
        this.stepHang(now);
        break;
      case "idle":
      case "walk":
      case "sleep":
        this.stepGround(dt, now);
        break;
      case "read":
      case "code":
      case "think":
      case "draw":
      case "coffee":
      case "pizza":
        this.stepActivity(now);
        break;
      case "chase":
        this.stepChase(dt, now);
        break;
      case "eat":
        this.stepEat(now);
        break;
      case "dance":
        this.stepDance(now);
        break;
    }

    if (this.hopActive) {
      this.hopOff += this.hopVy * dt;
      this.hopVy += GRAVITY * dt;
      if (this.hopOff >= 0) {
        this.hopOff = 0;
        this.hopVy = 0;
        this.hopActive = false;
        this.squashUntil = now + 0.1;
      }
    }
  }

  stepGround(dt, now) {
    const [minX, maxX] = this.walkRange();
    const maxY = this.floorY();
    this.walkIn = false;
    if (this.state !== "walk") {
      // Walk back on screen if the work area shrank under us.
      if (this.x < minX - 0.5 || this.x > maxX + 0.5) {
        const step = 60 * dt;
        if (this.x < minX) {
          this.x = Math.min(minX, this.x + step);
          this.look = 1;
        } else {
          this.x = Math.max(maxX, this.x - step);
          this.look = -1;
        }
        this.walkIn = true;
      } else {
        this.x = clamp(this.x, minX, maxX);
      }
    }

    if (this.y < maxY - 0.5 && !this.hopActive) {
      this.fall(this.state === "walk" ? WALK_SPEED * this.look : 0);
      return;
    }
    this.y = Math.min(this.y, maxY);

    if (this.state === "walk") {
      const dir = this.walkTarget > this.x ? 1 : -1;
      this.look = dir;
      this.x += dir * WALK_SPEED * (this.hungry ? 0.7 : 1) * dt;
      const reached = (dir > 0 && this.x >= this.walkTarget) || (dir < 0 && this.x <= this.walkTarget);
      const hitEdge = (dir < 0 && this.x <= minX) || (dir > 0 && this.x >= maxX);
      const side = dir > 0 ? "right" : "left";
      // Walking off a side with another monitor there takes him over to it.
      if (hitEdge && this.walkCross && !this.onPlatform && this.edges[side]) {
        if ((dir > 0 && this.x >= maxX + this.w * 0.5) || (dir < 0 && this.x <= minX - this.w * 0.5)) {
          this.cross(side, now);
        }
        return;
      }
      if (reached || hitEdge) {
        this.x = clamp(this.x, minX, maxX);
        this.state = "idle";
        this.nextAction = now + rand(2.5, 6);
      }
      return;
    }

    if (this.state === "sleep") {
      if (this.napUntil && now >= this.napUntil) {
        this.wakeUp(now, "stretch");
        return;
      }
      if (this.music) {
        this.wakeUp(now, "music_wake");
        return;
      }
      const c = this.cursor;
      if (c && this.hitTest(c.x, c.y)) {
        if (!this.hoverSince) this.hoverSince = now;
        if (now - this.hoverSince > HOVER_WAKE) {
          this.wakeUp(now, "wake");
          return;
        }
      } else {
        this.hoverSince = 0;
      }
      if (now >= this.nextZ) {
        this.nextZ = now + 1.6;
        this.fx.spawn("zzz");
      }
      return;
    }

    if (this.updateMusic(now)) return;
    const settled = now - this.lastInteraction > 3 && !this.hopActive && !this.walkIn;
    if (this.claudeSessions.size && this.state === "idle" && settled) {
      this.startWork(now, false);
      return;
    }
    if (this.music && this.phones && now >= this.nextNote) {
      this.nextNote = now + rand(1.2, 2);
      this.fx.spawn("note");
    }

    if (now >= this.nextBlink) {
      this.blinkUntil = now + 0.13;
      this.nextBlink = now + (Math.random() < 0.15 ? 0.35 : rand(2, 5.5));
    }

    if (this.updateHover(now)) return;
    // Ringing a reminder: no wandering off or starting something else.
    if (this.reminder) return;

    const attention = this.attention(now);
    if (attention !== null) this.look = attention;

    if (this.chatting) {
      this.lastInteraction = now;
      this.nextChatter = now + 10;
      this.nextAction = Math.max(this.nextAction, now + 1);
      return;
    }

    if (now >= this.nextChatter && now - this.lastInteraction > 5 && !this.hopActive && !this.walkIn) {
      this.nextChatter = now + rand(CHATTER_EVERY[0], CHATTER_EVERY[1]);
      if (this.hungry && Math.random() < 0.4) this.fx.say("hungry");
      else if (this.sad && Math.random() < 0.4) {
        this.setExpr("sad", 2.5, now);
        this.fx.say("sad");
      } else this.fx.say(season() && Math.random() < 0.15 ? `idle_${season()}` : "idle");
    }

    const busy = this.music || this.claudeSessions.size > 0;
    if (now - this.lastInteraction > SLEEP_AFTER && !busy && !this.hopActive && !this.walkIn) {
      this.goToSleep(now, now + rand(DEEP_SLEEP[0], DEEP_SLEEP[1]) * 60);
      this.fx.hideBubble();
      return;
    }

    if (now < this.nextAction || this.hopActive || this.walkIn) return;
    this.nextAction = now + rand(3.5, 9);
    if (now >= this.nextActivity && now - this.lastInteraction > 8 && Math.random() < 0.3) {
      this.startActivity(now);
      return;
    }
    // Hop up onto the focused window, or back down from it.
    if (this.platform && !this.onPlatform && Math.random() < 0.12 && this.leapTo(this.platform, now)) return;
    if (this.onPlatform && Math.random() < 0.1) {
      this.jumpDown(now);
      return;
    }
    const roll = Math.random();
    if (roll < 0.38) {
      // Near a side with another monitor: sometimes go and see what's there.
      const nearSide = this.x - minX < 220 ? "left" : maxX - this.x < 220 ? "right" : null;
      if (nearSide && this.edges[nearSide] && !this.onPlatform && Math.random() < 0.35) {
        this.walkCross = true;
        this.walkTarget = nearSide === "left" ? minX - 400 : maxX + 400;
        this.state = "walk";
        return;
      }
      this.walkCross = false;
      const distance = rand(40, 160) * pick([-1, 1]);
      const target = clamp(this.x + distance, minX, maxX);
      if (Math.abs(target - this.x) > 12) {
        this.walkTarget = target;
        this.state = "walk";
      }
    } else if (roll < 0.58) {
      if (attention === null) this.look = pick([-1, 0, 1]);
    } else if (roll < 0.72) {
      this.fx.say("idle");
    } else if (roll < 0.84) {
      this.hop(this.vitals.mood > 85 ? 480 : 360);
    } else {
      this.waveUntil = now + 1.6;
    }
  }

  activityAllowed(name, now) {
    const hour = new Date().getHours();
    switch (name) {
      case "sleep":
        return !this.music; // No naps while music is playing.
      case "coffee":
        return hour >= 6 && hour < 12 && now - this.lastSnack.coffee > SNACK_EVERY;
      case "pizza":
        return hour >= 17 && hour < 24 && this.vitals.food < 80 && now - this.lastSnack.pizza > SNACK_EVERY;
      case "chase": {
        // Only when the cursor is moving around near him on the floor.
        const c = this.cursor;
        return !!c && !this.onPlatform && now - c.t < 1.5 && c.y > this.bounds.height * 0.5 && this.attention(now) !== null;
      }
      default:
        return true;
    }
  }

  startActivity(now) {
    const [state, min, max] = pick(ACTIVITIES.filter(([name]) => this.activityAllowed(name, now)));
    this.look = 0;
    if (state === "sleep") {
      this.goToSleep(now, now + rand(min, max));
      this.fx.say("nap");
      return;
    }
    this.fed = false;
    this.beginActivity(state, min, max, now);
    this.fx.say(`${state}_start`);
  }

  beginActivity(state, min, max, now) {
    this.state = state;
    this.look = 0;
    this.activityStart = now;
    this.activityUntil = now + rand(min, max);
    this.nextActivityEvent = now + (state === "pizza" || state === "coffee" ? rand(1.5, 2.5) : rand(3, 6));
    this.typingPause = 0;
    this.bites = 0;
    this.paint = 0;
    this.sipUntil = 0;
    this.biteUntil = 0;
    this.chaseIdle = now;
    if (state === "coffee" || state === "pizza") this.lastSnack[state] = now;
  }

  goToSleep(now, napUntil) {
    this.state = "sleep";
    this.phones = false;
    this.musicReacted = false;
    this.look = 0;
    this.napUntil = napUntil;
    this.nextZ = now + 0.6;
  }

  stepActivity(now) {
    const maxY = this.floorY();
    if (this.y < maxY - 0.5) {
      this.fall(0);
      return;
    }
    this.y = Math.min(this.y, maxY);
    this.updateHover(now);
    this.updateMusic(now);

    if (now >= this.nextBlink) {
      this.blinkUntil = now + 0.13;
      this.nextBlink = now + rand(2.5, 6);
    }

    if (this.state === "coffee" && now >= this.nextSteam && now >= this.sipUntil) {
      this.nextSteam = now + rand(0.9, 1.6);
      this.fx.spawn("steam");
    }
    if (now >= this.activityUntil) {
      this.finishActivity(now);
      return;
    }
    if (now < this.nextActivityEvent) return;

    switch (this.state) {
      case "read":
        this.nextActivityEvent = now + rand(4, 8);
        this.flipUntil = now + 0.35;
        if (Math.random() < 0.3) this.fx.say("read");
        break;
      case "code":
        this.nextActivityEvent = now + rand(1.2, 2.6);
        if (Math.random() < 0.25) this.typingPause = now + rand(1, 2.5);
        if (this.claudeWork) {
          if (now >= this.typingPause) this.fx.spawn("code");
        } else if (Math.random() < 0.12) {
          if (Math.random() < 0.35) {
            this.setExpr("angry", 1.6, now);
            this.fx.say("bug");
          } else {
            this.fx.say("code");
          }
        } else if (now >= this.typingPause) {
          this.fx.spawn("code");
        }
        break;
      case "think":
        this.nextActivityEvent = now + rand(3, 5);
        if (Math.random() < 0.4) this.fx.say("think");
        break;
      case "coffee":
        this.nextActivityEvent = now + rand(3, 5);
        this.sipUntil = now + 1.1;
        if (Math.random() < 0.3) this.fx.say("coffee");
        break;
      case "pizza":
        this.nextActivityEvent = now + rand(2, 3.2);
        this.bites = Math.min(PIZZA_BITES, this.bites + 1);
        this.biteUntil = now + 0.45;
        this.fx.spawn("crumb");
        if (Math.random() < 0.3) this.fx.say("pizza");
        if (this.bites >= PIZZA_BITES) this.activityUntil = Math.min(this.activityUntil, now + 1.6);
        break;
      case "draw":
        this.nextActivityEvent = now + rand(1.4, 2.8);
        this.paint = Math.min(PAINT_DOTS, this.paint + 1);
        if (Math.random() < 0.15) this.fx.say("draw");
        break;
    }
  }

  finishActivity(now) {
    const state = this.state;
    const quiet = this.claudeWork;
    this.claudeWork = false;
    this.state = "idle";
    this.nextAction = now + rand(2, 5);
    this.nextActivity = now + rand(20, 45);
    if (state === "think") {
      this.hop(420);
      this.setExpr("happy", 1.5, now);
      for (let i = 0; i < 3; i++) this.fx.spawn("spark");
      this.fx.say("idea");
    } else if (!quiet) {
      this.fx.say(`${state}_end`);
    }
    const v = this.vitals;
    if (state === "pizza") {
      v.food = clamp100(v.food + (this.fed ? 40 : 30));
      v.mood = clamp100(v.mood + 6);
      this.setExpr("happy", 1.5, now);
    } else if (state === "coffee") {
      v.food = clamp100(v.food + 8);
      v.mood = clamp100(v.mood + 4);
    } else if (state === "draw") {
      v.mood = clamp100(v.mood + 4);
      this.setExpr("happy", 1.5, now);
      this.fx.spawn("spark");
    }
    this.fed = false;
  }

  // A file is being dragged over Clawd: he looks up expectantly.
  expectFood(now) {
    if (this.state === "sleep") this.wakeUp(now, "wake");
    this.setExpr("surprised", 0.4, now);
    this.look = 0;
    this.lastInteraction = now;
  }

  // A file was dropped on him: a few bites of a paper sheet.
  eatFile(now) {
    if (this.state === "sleep") this.napUntil = 0;
    if (this.held || this.claudeWork) return;
    this.state = "eat";
    this.look = 0;
    this.bites = 0;
    this.activityStart = now;
    this.nextActivityEvent = now + 0.35;
    this.vitals.food = clamp100(this.vitals.food + 4);
  }

  stepEat(now) {
    const maxY = this.floorY();
    if (this.y < maxY - 0.5) this.y = Math.min(maxY, this.y + 6);
    if (now < this.nextActivityEvent) return;
    this.nextActivityEvent = now + 0.4;
    this.bites++;
    this.biteUntil = now + 0.2;
    this.fx.spawn("crumb");
    if (this.bites <= 3) return;
    this.state = "idle";
    this.nextAction = now + rand(2, 4);
    this.setExpr("happy", 1.4, now);
    this.hop(380);
    this.fx.spawn("spark");
  }

  // Tag: Clawd runs after the cursor along the floor and jumps when it's above him.
  stepChase(dt, now) {
    const [minX, maxX] = this.walkRange();
    const maxY = this.floorY();
    if (this.y < maxY - 0.5 && !this.hopActive) {
      this.fall(0);
      return;
    }
    this.y = Math.min(this.y, maxY);
    const c = this.cursor;
    if (c && this.hitTest(c.x, c.y)) return this.endChase(true, now);
    if (c && now - c.t < 0.5) this.chaseIdle = now;
    if (now >= this.activityUntil || now - this.chaseIdle > 3) return this.endChase(false, now);
    if (!c) return;
    const dx = c.x - (this.x + this.w / 2);
    if (Math.abs(dx) > 6) {
      const dir = dx > 0 ? 1 : -1;
      this.look = dir;
      this.x = clamp(this.x + dir * CHASE_SPEED * (this.hungry ? 0.7 : 1) * dt, minX, maxX);
    }
    const above = this.y - c.y;
    if (Math.abs(dx) < this.w && above > 0 && above < 170 && !this.hopActive) this.hop(640);
  }

  endChase(caught, now) {
    this.state = "idle";
    this.nextAction = now + rand(2, 4);
    this.nextActivity = now + rand(40, 80);
    if (caught) {
      this.cheer(6);
      this.setExpr("happy", 1.8, now);
      this.hop(520);
      for (let i = 0; i < 3; i++) this.fx.spawn("spark");
      this.fx.say("chase_win", true);
    } else {
      this.cheer(2);
      this.fx.say("chase_lose");
    }
  }

  // A jump that lands on the window's title bar.
  leapTo(p, now) {
    const top = p.y - this.h;
    const rise = this.y - top;
    if (rise < 30 || this.held) return false;
    const tx = clamp(this.x, p.x, p.x + p.w - this.w);
    const extra = Math.min(40, Math.max(0, top));
    const vy = -Math.sqrt(2 * GRAVITY * (rise + extra));
    const t = -vy / GRAVITY + Math.sqrt((2 * Math.max(extra, 1)) / GRAVITY);
    // The air drag in stepFly slows him down a little on the way; make up for it.
    const drag = (1 - Math.exp(-0.35 * t)) / (0.35 * t);
    this.state = "fly";
    this.onPlatform = false;
    this.leaping = true;
    this.vx = (tx - this.x) / t / drag;
    this.vy = vy;
    this.look = this.vx > 0 ? 1 : -1;
    if (Math.random() < 0.4) this.fx.say("perch_jump");
    return true;
  }

  jumpDown(now) {
    this.onPlatform = false;
    this.state = "fly";
    this.leaping = true;
    const p = this.platform;
    const toRight = p ? this.x + this.w / 2 > p.x + p.w / 2 : Math.random() < 0.5;
    this.vx = (toRight ? 1 : -1) * rand(90, 160);
    this.vy = -380;
    this.lastInteraction = now - 4;
  }

  // Clawd is free to chat when he isn't held, flying or asleep.
  canChat() {
    return CALM_STATES.includes(this.state);
  }

  startWork(now, announce) {
    if (this.state === "sleep") {
      this.napUntil = 0;
      this.setExpr("surprised", 0.8, now);
    }
    if (this.held || ["fly", "hang"].includes(this.state)) return;
    this.state = "code";
    this.claudeWork = true;
    this.look = 0;
    this.activityStart = now;
    this.activityUntil = Infinity;
    this.nextActivityEvent = now + 0.5;
    this.typingPause = 0;
    if (this.music) this.phones = true;
    if (announce) this.fx.say("claude_start", true);
  }

  onClaude(event, now) {
    const sessions = this.claudeSessions;
    const id = event.session;
    const touch = () => {
      const session = sessions.get(id) || { project: event.project };
      session.last = now;
      if (event.project) session.project = event.project;
      sessions.set(id, session);
    };
    switch (event.kind) {
      case "start":
        touch();
        this.lastInteraction = now;
        this.startWork(now, true);
        break;
      case "tool": {
        touch();
        if (!this.claudeWork) this.startWork(now, false);
        // Shell commands are always shown; other tools at most every few seconds.
        const shell = event.tool === "Bash" || event.tool === "PowerShell";
        if (!shell && now < this.nextToolSay) break;
        this.nextToolSay = now + TOOL_SAY_EVERY;
        this.fx.say(`tool:${event.tool}`, true, { detail: event.detail, tool: event.tool, background: event.background });
        break;
      }
      case "thought":
        if (sessions.has(id)) touch();
        this.nextToolSay = now + TOOL_SAY_EVERY;
        // Play-by-play: fine to skip while the user is talking to Clawd.
        this.fx.sayText(event.text, true, 0, "ephemeral");
        break;
      case "attention":
        touch();
        this.waveUntil = now + 3;
        if (this.state === "sleep") this.startle(now);
        this.hop(400);
        this.fx.say("claude_wait", true, { project: event.project });
        break;
      case "stop":
        sessions.delete(id);
        if (sessions.size) break;
        if (this.claudeWork && this.state === "code") this.finishActivity(now);
        this.claudeWork = false;
        this.hop(520);
        this.waveUntil = now + 2.5;
        for (let i = 0; i < 3; i++) this.fx.spawn("spark");
        this.fx.say("claude_done", true, { project: event.project });
        this.reportAt = now + 3;
        break;
      case "report":
        // Haiku's summary follows the "done!" line instead of replacing it.
        if (now < this.reportAt) this.pendingReport = event.text;
        else this.fx.sayText(event.text, true);
        break;
      case "end":
        sessions.delete(id);
        if (!sessions.size && this.claudeWork && this.state === "code") this.finishActivity(now);
        break;
      case "subagent":
        if (sessions.has(id)) touch();
        this.nextToolSay = now + TOOL_SAY_EVERY;
        this.fx.spawn("spark");
        this.fx.say("subagent_done", true, { agent: event.agent });
        break;
      case "failure": {
        sessions.delete(id);
        if (!sessions.size && this.claudeWork && this.state === "code") this.finishActivity(now);
        this.claudeWork = false;
        this.setExpr("sad", 3, now);
        const error = event.error || "";
        const phrase = /rate|limit|quota/i.test(error)
          ? "claude_fail_limit"
          : /auth|login|billing|credential/i.test(error)
            ? "claude_fail_auth"
            : /server|overload|api_error|unavailable|5\d\d/i.test(error)
              ? "claude_fail_server"
              : "claude_fail";
        this.fx.say(phrase, true, { error, project: event.project });
        break;
      }
      case "limits":
        this.onLimits(event, now);
        break;
    }
  }

  onTyping(kpm, now) {
    if (kpm === null) return;
    if (kpm > 0) this.noteUserInput(now);
    if (kpm < FAST_TYPING_KPM) {
      this.fastTypingSince = 0;
      return;
    }
    if (!this.fastTypingSince) this.fastTypingSince = now;
    const sustained = now - this.fastTypingSince > 6;
    if (sustained && now - this.lastTypingComment > 180 && this.canChat()) {
      this.lastTypingComment = now;
      this.fx.say("typing_fast");
    }
  }

  onAudio(event, now) {
    if (event.kind === "music") {
      this.music = event.playing;
    } else if (event.kind === "beat") {
      this.beatAt = now;
      this.beatCount++;
      if (this.state === "dance" && event.strength > 4 && Math.random() < 0.2) this.hop(300);
    } else if (event.kind === "loud") {
      this.startle(now);
    }
  }

  startle(now) {
    if (this.state === "sleep") {
      this.napUntil = 0;
      this.state = "idle";
      this.lastInteraction = now;
      this.nextAction = now + rand(2, 4);
      this.setExpr("surprised", 1.4, now);
      this.hop(520);
      this.fx.say("loud_wake", true);
      return;
    }
    const calm = CALM_STATES.includes(this.state);
    if (!calm || now - this.lastStartle < 8) return;
    this.lastStartle = now;
    this.setExpr("surprised", 0.8, now);
    this.squashUntil = now + 0.15;
    this.hop(300);
    if (Math.random() < 0.5) this.fx.say("loud");
  }

  // Reacts to music starting, switching between dancing and listening, and stopping.
  // Returns true when it changed what Clawd is doing this tick.
  updateMusic(now) {
    if (!this.music) {
      if (!this.musicReacted) return false;
      this.musicReacted = false;
      const wasDancing = this.state === "dance";
      if (wasDancing) {
        this.state = "idle";
        this.nextAction = now + rand(2, 4);
      }
      if (this.phones || wasDancing) this.fx.say("music_end");
      this.phones = false;
      return wasDancing;
    }
    if (!this.musicReacted) {
      this.musicReacted = true;
      this.nextMusicSwitch = now + rand(25, 50);
      if (ACTIVITY_STATES.has(this.state)) {
        this.phones = true;
        this.fx.say("music_phones");
      } else if (Math.random() < 0.5) {
        this.startDance();
      } else {
        this.phones = true;
        this.fx.say("music");
      }
      return true;
    }
    if (now >= this.nextMusicSwitch && (this.state === "idle" || this.state === "dance")) {
      this.nextMusicSwitch = now + rand(25, 50);
      if (this.state === "dance") {
        this.state = "idle";
        this.phones = true;
        this.fx.say("music");
      } else {
        this.startDance();
      }
      return true;
    }
    return false;
  }

  startDance() {
    this.state = "dance";
    this.look = 0;
    this.fx.say("music_dance");
  }

  stepDance(now) {
    const maxY = this.floorY();
    if (this.y < maxY - 0.5 && !this.hopActive) {
      this.fall(0);
      return;
    }
    this.y = Math.min(this.y, maxY);
    this.updateHover(now);
    if (this.updateMusic(now)) return;
    if (now >= this.nextNote) {
      this.nextNote = now + rand(0.6, 1.1);
      this.fx.spawn("note");
    }
  }

  // Real beats while the analyzer finds them, otherwise a steady 120 BPM.
  beat(now) {
    if (now - this.beatAt < 1.5) return { count: this.beatCount, since: now - this.beatAt };
    const t = now * 2;
    return { count: Math.floor(t), since: (t % 1) / 2 };
  }

  // Cursor resting on Clawd makes him happy and keeps him in place.
  updateHover(now) {
    const c = this.cursor;
    if (!c || !this.hitTest(c.x, c.y)) {
      this.hoverSince = 0;
      return false;
    }
    if (!this.hoverSince) this.hoverSince = now;
    this.lastInteraction = now;
    this.look = 0;
    this.nextAction = Math.max(this.nextAction, now + 1);
    if (this.state !== "idle") return true;
    const busy = this.expr && this.expr !== "happy" && now < this.exprUntil;
    if (now - this.hoverSince > 0.6 && !busy) this.setExpr("happy", 0.3, now);
    return true;
  }

  // Eyes follow a recently moved cursor: -1 left, 0 straight, 1 right.
  attention(now) {
    const c = this.cursor;
    if (!c || now - c.t > ATTENTION_TIME) return null;
    const cx = this.x + this.w / 2;
    const cy = this.y + this.h / 2;
    const dx = c.x - cx;
    if (Math.hypot(dx, c.y - cy) > ATTENTION_RADIUS) return null;
    if (Math.abs(dx) < this.w / 2) return 0;
    return dx > 0 ? 1 : -1;
  }

  fall(vx) {
    this.state = "fly";
    this.onPlatform = false;
    this.vx = vx;
    this.vy = 0;
  }

  stepFly(dt, now) {
    const { minX, maxX, minY, maxY } = this.bounds;
    const prevY = this.y;
    this.x += this.vx * dt;
    this.y += this.vy * dt;
    this.vy += GRAVITY * dt;
    this.vx *= Math.exp(-0.35 * dt);

    // Flying off a side with another monitor there carries him over to it.
    if (this.x < minX - this.w * 0.4 && this.edges.left) return this.cross("left", now);
    if (this.x > maxX + this.w * 0.4 && this.edges.right) return this.cross("right", now);

    const p = this.platform;
    if (p && this.vy > 0) {
      const top = p.y - this.h;
      const cx = this.x + this.w / 2;
      if (prevY <= top + 1 && this.y >= top && cx >= p.x && cx <= p.x + p.w) {
        this.y = top;
        this.onPlatform = true;
        this.land(now);
        if (Math.random() < 0.35) this.fx.say("perch");
        return;
      }
    }

    let impact = 0;
    if (this.x < minX && !this.edges.left) {
      this.x = minX;
      impact = Math.max(impact, Math.abs(this.vx));
      this.vx = Math.abs(this.vx) * 0.55;
    } else if (this.x > maxX && !this.edges.right) {
      this.x = maxX;
      impact = Math.max(impact, Math.abs(this.vx));
      this.vx = -Math.abs(this.vx) * 0.55;
    }

    if (this.y < minY) {
      if (this.vy < -250) {
        this.startHang(now);
        return;
      }
      this.y = minY;
      impact = Math.max(impact, Math.abs(this.vy));
      this.vy = Math.abs(this.vy) * 0.55;
    } else if (this.y > maxY) {
      this.y = maxY;
      impact = Math.max(impact, Math.abs(this.vy));
      if (this.vy < 380) {
        this.land(now);
      } else {
        this.vy = -Math.abs(this.vy) * 0.42;
        this.vx *= 0.75;
        this.squashUntil = now + 0.1;
      }
    }

    if (impact > 2300) {
      this.setExpr("dizzy", 2.6, now);
      this.cheer(-4);
      this.fx.say("dizzy");
    }
  }

  land(now) {
    this.leaping = false;
    this.state = "idle";
    this.vx = 0;
    this.vy = 0;
    this.squashUntil = now + 0.14;
    this.nextAction = now + rand(2, 5);
  }

  startHang(now) {
    this.state = "hang";
    this.vx = 0;
    this.vy = 0;
    this.y = this.bounds.minY - this.unit * 2;
    this.hangStart = now;
    this.hangUntil = now + rand(2, 3.5);
    if (Math.random() < 0.7) this.fx.say("hang");
  }

  stepHang(now) {
    const { minX, maxX, minY } = this.bounds;
    this.x = clamp(this.x, minX, maxX);
    this.y = minY - this.unit * 2;
    if (now < this.hangUntil) return;
    this.state = "fly";
    this.vx = 0;
    this.vy = 150;
    this.setExpr("surprised", 0.8, now);
    if (Math.random() < 0.6) this.fx.say("hang_fall");
  }

  recall(now) {
    const { minX, maxX, minY, maxY } = this.bounds;
    this.x = (minX + maxX) / 2;
    this.y = (minY + maxY) / 2;
    this.crossing = null;
    this.onPlatform = false;
    this.touch = null;
    this.hopActive = false;
    this.hopOff = 0;
    this.lastInteraction = now;
    this.fall(0);
    for (let i = 0; i < 3; i++) this.fx.spawn("spark");
    this.fx.say("hello");
  }

  frame(now) {
    let eyes = "open";
    let look = this.look;
    let arms = "normal";
    let legs = "stand";
    let squash = 0;
    let blush = false;
    let prop = null;
    let spinner = -1;
    let flip = false;
    let bites = 0;
    let paint = 0;
    let sip = false;
    const state = this.state;
    const walkLegs = () => (Math.floor(now * 7) % 2 ? "walk1" : "walk2");

    if (state === "fly" && this.leaping) {
      [eyes, arms, legs] = ["happy", "up", "dangle"];
    } else if (state === "drag" || state === "fly") {
      [eyes, arms, legs] = ["surprised", "up", "dangle"];
    } else if (state === "pet") {
      eyes = "happy";
      blush = true;
      squash = Math.floor(now * 3) % 2 ? 1 : 0;
      look = 0;
    } else if (state === "sleep") {
      [eyes, arms, legs, look] = ["sleep", "tuck", "tuck", 0];
    } else if (state === "read") {
      // Eyes sweep along the lines, then jump back to the start of the next one.
      [eyes, arms, prop] = ["down", "hold", "book"];
      look = [-1, 0, 1][Math.floor(((now - this.activityStart) % 2.4) / 0.8)];
      flip = now < this.flipUntil;
    } else if (state === "code") {
      prop = "laptop";
      const typing = now >= this.typingPause;
      arms = typing && Math.floor(now * 8) % 2 ? "type1" : "type2";
      if (!typing) eyes = "up";
      look = typing ? 0 : 1;
    } else if (state === "dance") {
      const { count, since } = this.beat(now);
      const odd = count % 2 === 1;
      eyes = "happy";
      arms = odd ? "wave1" : "wave2";
      legs = odd ? "walk1" : "walk2";
      squash = since < 0.12 ? 1 : 0;
      look = odd ? 1 : -1;
    } else if (state === "think") {
      eyes = "up";
      look = Math.floor(now * 0.8) % 2 ? 1 : -1;
      spinner = Math.floor(now * 8) % SPINNER_LENGTH;
    } else if (state === "coffee") {
      [arms, prop] = ["hold", "mug"];
      sip = now < this.sipUntil;
      eyes = sip ? "happy" : "open";
      look = 0;
    } else if (state === "pizza") {
      [arms, prop, bites] = ["hold", "pizza", this.bites];
      eyes = now < this.biteUntil ? "happy" : "open";
      squash = now < this.biteUntil ? 1 : 0;
      look = 0;
    } else if (state === "draw") {
      prop = "easel";
      paint = this.paint;
      arms = Math.floor(now * 3) % 2 ? "paint1" : "paint2";
      eyes = "down";
      look = 1;
    } else if (state === "eat") {
      [arms, prop, bites] = ["hold", "doc", this.bites];
      eyes = now < this.biteUntil ? "happy" : "open";
      squash = now < this.biteUntil ? 1 : 0;
      look = 0;
    } else if (state === "chase") {
      legs = Math.floor(now * 12) % 2 ? "walk1" : "walk2";
      arms = Math.floor(now * 6) % 2 ? "wave1" : "wave2";
      eyes = "happy";
    } else if (state === "walk") {
      legs = walkLegs();
    } else if (state === "press") {
      squash = 1;
    } else if (state === "hang") {
      eyes = now - this.hangStart < 0.5 ? "surprised" : "open";
      [arms, legs] = ["up", "dangle"];
      look = Math.floor(now * 1.5) % 2 ? -1 : 1;
    }

    if (this.walkIn && state === "idle") legs = walkLegs();
    if (state === "idle" && this.phones && this.music) {
      // Nodding along, eyes closed every few bars.
      const { count, since } = this.beat(now);
      if (since < 0.1) squash = 1;
      if (count % 8 >= 6) eyes = "happy";
    }
    if ((state === "idle" || state === "walk") && now < this.waveUntil) {
      arms = Math.floor(now * 5) % 2 ? "wave1" : "wave2";
    }
    if (state === "idle" && now < this.stretchUntil) {
      [arms, eyes] = ["up", "happy"];
      squash = -1;
    }
    if (this.hopActive && (state === "idle" || state === "walk")) {
      legs = "dangle";
      if (this.hopVy < 0) {
        squash = -1;
        arms = "up";
      }
    }
    if (now < this.exprUntil && this.expr && state !== "sleep") {
      eyes = this.expr;
      if (this.expr === "happy") blush = true;
    }
    if (now < this.squashUntil && squash === 0) squash = 1;
    if (this.awaitingReply) {
      eyes = "up";
      spinner = Math.floor(now * 8) % SPINNER_LENGTH;
    }
    let ring = 0;
    if (this.reminder && (state === "idle" || state === "walk")) {
      [prop, arms] = ["clock", "hold"];
      const loud = now - this.reminder.since < REMIND_LOUD;
      ring = loud ? [-1, 0, 1, 0][Math.floor(now * 16) % 4] : 0;
      eyes = loud && Math.floor(now * 2) % 2 ? "surprised" : "open";
      look = 0;
    }
    const canBlink = ["idle", "walk", "hang", "code", "think", "coffee", "pizza"].includes(state);
    if (now < this.blinkUntil && (eyes === "open" || eyes === "up") && canBlink) eyes = "blink";
    if (legs === "tuck" && squash) squash = 0;
    const cap = state === "sleep";
    const phones = this.phones && !cap;
    // The costume stays out of the way of headphones, the nightcap and props.
    const outfit = season();
    const hat = outfit === "santa" && !cap && !phones ? "santa" : null;
    const pumpkin = outfit === "pumpkin" && !prop && (state === "idle" || state === "walk") && arms !== "wave1";
    return { eyes, look, arms, legs, squash, blush, prop, cap, phones, spinner, flip, bites, paint, sip, hat, pumpkin, ring };
  }

  rotation(now) {
    if (this.state === "fly") return clamp(this.vx / 70, -25, 25);
    if (this.state === "dance") {
      const { count, since } = this.beat(now);
      return (count % 2 ? 1 : -1) * (4 + 4 * Math.max(0, 1 - since * 4));
    }
    if (this.state === "hang") return Math.sin((now - this.hangStart) * 3) * 6;
    if (this.expr === "dizzy" && now < this.exprUntil) return Math.sin(now * 10) * 7;
    if (this.state === "drag" && this.touch) {
      const last = this.touch.hist[this.touch.hist.length - 1];
      if (now - last[0] < 0.08) return clamp(this.touch.vx / 90, -18, 18);
    }
    return 0;
  }

  pointerDown(x, y, now) {
    if (this.state === "chase") {
      this.endChase(true, now);
    }
    if (this.reminder) this.fx.dismissReminder();
    const wasSleeping = this.state === "sleep";
    const interrupted = ACTIVITY_STATES.has(this.state) || this.state === "dance" ? this.state : null;
    this.lastInteraction = now;
    this.y += this.hopOff;
    this.hopActive = false;
    this.hopOff = 0;
    this.hopVy = 0;
    this.vx = 0;
    this.vy = 0;
    this.touch = {
      x0: x,
      y0: y,
      t0: now,
      ox: x - this.x,
      oy: y - this.y,
      hist: [[now, x, y]],
      anchor: x,
      dir: 0,
      vx: 0,
    };
    this.state = "press";
    this.napUntil = 0;
    if (interrupted) this.nextActivity = now + rand(20, 45);
    if (interrupted === "dance") this.nextMusicSwitch = now + rand(3, 6);
    if (wasSleeping) {
      this.setExpr("surprised", 0.9, now);
      this.fx.say("wake");
    } else if (interrupted) {
      this.setExpr("surprised", 0.7, now);
      this.fx.say(this.claudeWork ? "work_stop" : `${interrupted}_stop`, true);
    }
    this.claudeWork = false;
  }

  pointerMove(x, y, now) {
    const touch = this.touch;
    if (!touch) return;
    const hist = touch.hist;
    hist.push([now, x, y]);
    while (hist.length > 2 && now - hist[0][0] > 0.1) hist.shift();

    if (this.state === "press" && Math.hypot(x - touch.x0, y - touch.y0) > 8) {
      this.state = "drag";
      if (Math.random() < 0.25) this.fx.say("drag");
    }
    if (this.state === "drag") {
      const { width, height } = this.bounds;
      // Dragging him to the edge of the screen carries him to the next monitor.
      if (x <= 1 && this.edges.left) this.cross("left", now);
      else if (x >= width - 1 && this.edges.right) this.cross("right", now);
      this.onPlatform = false;
      this.x = clamp(x - touch.ox, -this.w * 0.3, width - this.w * 0.7);
      this.y = clamp(y - touch.oy, 0, height - this.h * 0.5);
      const [t0, x0] = hist[0];
      if (now - t0 > 0.005) touch.vx = (x - x0) / (now - t0);
    } else if (this.state === "pet") {
      // Rubbing back and forth: every change of direction is a stroke.
      const dx = x - touch.anchor;
      if (Math.abs(dx) > 10) {
        const dir = dx > 0 ? 1 : -1;
        if (touch.dir && dir !== touch.dir) {
          this.fx.spawn("heart");
          this.cheer(1.5);
        }
        touch.dir = dir;
        touch.anchor = x;
      }
    }
  }

  pointerUp(now, cancelled = false) {
    const touch = this.touch;
    if (!touch) return;
    this.touch = null;
    this.lastInteraction = now;
    if (this.state === "drag") {
      this.releaseDrag(now, touch, cancelled);
    } else if (this.state === "pet") {
      this.state = "idle";
      this.cheer(3);
      this.setExpr("happy", 2.2, now);
      this.nextAction = now + rand(3, 6);
      if (Math.random() < 0.6) this.fx.say("pet");
    } else if (this.state === "press") {
      this.state = "idle";
      if (!cancelled) this.tap(now);
    }
  }

  releaseDrag(now, touch, cancelled) {
    let vx = 0;
    let vy = 0;
    const hist = touch.hist;
    if (!cancelled && hist.length >= 2) {
      const [t0, x0, y0] = hist[0];
      const [t1, x1, y1] = hist[hist.length - 1];
      if (t1 - t0 > 0.005 && now - t1 < 0.08) {
        vx = (x1 - x0) / (t1 - t0);
        vy = (y1 - y0) / (t1 - t0);
      }
    }
    vx = clamp(vx, -6000, 6000);
    vy = clamp(vy, -6000, 6000);
    const speed = Math.hypot(vx, vy);
    if (speed <= 700 && this.y <= this.bounds.minY + this.unit * 2) {
      this.startHang(now);
      return;
    }
    this.state = "fly";
    this.vx = vx;
    this.vy = vy;
    if (speed > 2600) this.fx.say("throw");
  }

  tap(now) {
    if (now - this.lastTap < 0.32) {
      this.lastTap = 0;
      this.hop(720);
      this.setExpr("happy", 1.3, now);
      for (let i = 0; i < 3; i++) this.fx.spawn("spark");
      if (Math.random() < 0.4) this.fx.say("jump");
      return;
    }
    this.lastTap = now;
    this.recentTaps = this.recentTaps.filter((t) => now - t < 2.5);
    this.recentTaps.push(now);
    if (this.recentTaps.length >= 6) {
      this.recentTaps = [];
      this.cheer(-6);
      this.setExpr("angry", 2.5, now);
      this.fx.say("annoyed", true);
      return;
    }
    this.squashUntil = now + 0.12;
    this.setExpr("surprised", 0.45, now);
    this.hop(330);
    if (Math.random() < 0.35) this.fx.say("tap");
  }

  startPet(now) {
    this.state = "pet";
    this.nextHeart = now + 0.1;
  }

  setExpr(expr, duration, now) {
    this.expr = expr;
    this.exprUntil = now + duration;
  }

  hop(strength) {
    if (!["idle", "walk", "dance", "chase"].includes(this.state)) return;
    this.hopActive = true;
    this.hopVy = -strength;
    if (this.hopOff > 0) this.hopOff = 0;
  }
}
