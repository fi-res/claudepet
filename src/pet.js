// Clawd's behaviour: state machine and physics. Units are CSS pixels and seconds.
import { GRID_W, GRID_H, SPINNER_LENGTH } from "./sprite.js";

const GRAVITY = 2600;
const WALK_SPEED = 42;
const SLEEP_AFTER = 180;
const ATTENTION_RADIUS = 360;
const ATTENTION_TIME = 2.5;

// Calm activities Clawd picks by himself: [state, min seconds, max seconds].
const ACTIVITIES = [
  ["read", 25, 60],
  ["code", 25, 60],
  ["think", 8, 15],
  ["sleep", 25, 50],
];
const ACTIVITY_STATES = new Set(["read", "code", "think"]);

const clamp = (v, lo, hi) => (hi < lo ? lo : v < lo ? lo : v > hi ? hi : v);
const rand = (a, b) => a + Math.random() * (b - a);
const pick = (items) => items[Math.floor(Math.random() * items.length)];

export class Pet {
  // `fx` receives side effects: say(category, force) and spawn(kind).
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
    this.cursor = null;
    this.hoverSince = 0;
    this.bounds = { minX: 0, maxX: 0, minY: 0, maxY: 0, width: 0, height: 0 };
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
      this.fx.say("hello");
    }

    if (this.state === "press" && this.touch && now - this.touch.t0 >= 0.4) {
      this.startPet(now);
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
        this.stepActivity(now);
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
    const { minX, maxX, maxY } = this.bounds;
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
      this.x += dir * WALK_SPEED * dt;
      const reached = (dir > 0 && this.x >= this.walkTarget) || (dir < 0 && this.x <= this.walkTarget);
      const hitEdge = (dir < 0 && this.x <= minX) || (dir > 0 && this.x >= maxX);
      if (reached || hitEdge) {
        this.x = clamp(this.x, minX, maxX);
        this.state = "idle";
        this.nextAction = now + rand(2.5, 6);
      }
      return;
    }

    if (this.state === "sleep") {
      if (this.napUntil && now >= this.napUntil) {
        this.napUntil = 0;
        this.state = "idle";
        this.nextAction = now + rand(2, 4);
        this.hop(300);
        this.fx.say("stretch");
        return;
      }
      if (now >= this.nextZ) {
        this.nextZ = now + 1.6;
        this.fx.spawn("zzz");
      }
      return;
    }

    if (now >= this.nextBlink) {
      this.blinkUntil = now + 0.13;
      this.nextBlink = now + (Math.random() < 0.15 ? 0.35 : rand(2, 5.5));
    }

    if (this.updateHover(now)) return;

    const attention = this.attention(now);
    if (attention !== null) this.look = attention;

    if (now - this.lastInteraction > SLEEP_AFTER && !this.hopActive && !this.walkIn) {
      this.goToSleep(now, 0);
      this.fx.hideBubble();
      return;
    }

    if (now < this.nextAction || this.hopActive || this.walkIn) return;
    this.nextAction = now + rand(3.5, 9);
    if (now >= this.nextActivity && now - this.lastInteraction > 8 && Math.random() < 0.3) {
      this.startActivity(now);
      return;
    }
    const roll = Math.random();
    if (roll < 0.38) {
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
      this.hop(360);
    } else {
      this.waveUntil = now + 1.6;
    }
  }

  startActivity(now) {
    const [state, min, max] = pick(ACTIVITIES);
    this.look = 0;
    if (state === "sleep") {
      this.goToSleep(now, now + rand(min, max));
      this.fx.say("nap");
      return;
    }
    this.state = state;
    this.activityStart = now;
    this.activityUntil = now + rand(min, max);
    this.nextActivityEvent = now + rand(3, 6);
    this.typingPause = 0;
    this.fx.say(`${state}_start`);
  }

  // napUntil = 0 means sleep until someone wakes him up.
  goToSleep(now, napUntil) {
    this.state = "sleep";
    this.look = 0;
    this.napUntil = napUntil;
    this.nextZ = now + 0.6;
  }

  stepActivity(now) {
    const { maxY } = this.bounds;
    if (this.y < maxY - 0.5) {
      this.fall(0);
      return;
    }
    this.y = Math.min(this.y, maxY);
    this.updateHover(now);

    if (now >= this.nextBlink) {
      this.blinkUntil = now + 0.13;
      this.nextBlink = now + rand(2.5, 6);
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
        if (Math.random() < 0.12) {
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
    }
  }

  finishActivity(now) {
    const state = this.state;
    this.state = "idle";
    this.nextAction = now + rand(2, 5);
    this.nextActivity = now + rand(20, 45);
    if (state === "think") {
      this.hop(420);
      this.setExpr("happy", 1.5, now);
      for (let i = 0; i < 3; i++) this.fx.spawn("spark");
      this.fx.say("idea");
    } else {
      this.fx.say(`${state}_end`);
    }
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
    this.vx = vx;
    this.vy = 0;
  }

  stepFly(dt, now) {
    const { minX, maxX, minY, maxY } = this.bounds;
    this.x += this.vx * dt;
    this.y += this.vy * dt;
    this.vy += GRAVITY * dt;
    this.vx *= Math.exp(-0.35 * dt);

    let impact = 0;
    if (this.x < minX) {
      this.x = minX;
      impact = Math.max(impact, Math.abs(this.vx));
      this.vx = Math.abs(this.vx) * 0.55;
    } else if (this.x > maxX) {
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
      this.fx.say("dizzy");
    }
  }

  land(now) {
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
    const state = this.state;
    const walkLegs = () => (Math.floor(now * 7) % 2 ? "walk1" : "walk2");

    if (state === "drag" || state === "fly") {
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
    } else if (state === "think") {
      eyes = "up";
      look = Math.floor(now * 0.8) % 2 ? 1 : -1;
      spinner = Math.floor(now * 8) % SPINNER_LENGTH;
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
    if ((state === "idle" || state === "walk") && now < this.waveUntil) {
      arms = Math.floor(now * 5) % 2 ? "wave1" : "wave2";
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
    const canBlink = ["idle", "walk", "hang", "code", "think"].includes(state);
    if (now < this.blinkUntil && (eyes === "open" || eyes === "up") && canBlink) eyes = "blink";
    if (legs === "tuck" && squash) squash = 0;
    const cap = state === "sleep";
    return { eyes, look, arms, legs, squash, blush, prop, cap, spinner, flip };
  }

  rotation(now) {
    if (this.state === "fly") return clamp(this.vx / 70, -25, 25);
    if (this.state === "hang") return Math.sin((now - this.hangStart) * 3) * 6;
    if (this.expr === "dizzy" && now < this.exprUntil) return Math.sin(now * 10) * 7;
    if (this.state === "drag" && this.touch) {
      const last = this.touch.hist[this.touch.hist.length - 1];
      if (now - last[0] < 0.08) return clamp(this.touch.vx / 90, -18, 18);
    }
    return 0;
  }

  pointerDown(x, y, now) {
    const wasSleeping = this.state === "sleep";
    const interrupted = ACTIVITY_STATES.has(this.state) ? this.state : null;
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
    if (wasSleeping) {
      this.setExpr("surprised", 0.9, now);
      this.fx.say("wake");
    } else if (interrupted) {
      this.setExpr("surprised", 0.7, now);
      this.fx.say(`${interrupted}_stop`, true);
    }
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
      this.x = clamp(x - touch.ox, -this.w * 0.3, width - this.w * 0.7);
      this.y = clamp(y - touch.oy, 0, height - this.h * 0.5);
      const [t0, x0] = hist[0];
      if (now - t0 > 0.005) touch.vx = (x - x0) / (now - t0);
    } else if (this.state === "pet") {
      // Rubbing back and forth: every change of direction is a stroke.
      const dx = x - touch.anchor;
      if (Math.abs(dx) > 10) {
        const dir = dx > 0 ? 1 : -1;
        if (touch.dir && dir !== touch.dir) this.fx.spawn("heart");
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
    if (this.state !== "idle" && this.state !== "walk") return;
    this.hopActive = true;
    this.hopVy = -strength;
    if (this.hopOff > 0) this.hopOff = 0;
  }
}
