// Claude plan limits: when a window resets, how fast it's being used up, and when
// Clawd should warn. Times are unix seconds. No DOM here, so it runs under node too.

const HOUR = 3600;
const DAY = 24 * HOUR;
export const WINDOWS = { five_hour: 5 * HOUR, seven_day: 7 * DAY };
const MONTHS = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
// "Oct 26, 12:20am", "Oct 30, 11pm", "3pm" — how `/usage` words a reset.
const RESET_TEXT = /^(?:([a-z]{3})[a-z]*\.? (\d{1,2}),?\s+)?(\d{1,2})(?::(\d{2}))?\s*([ap]m)$/i;

// When window `w` resets: its timestamp, or its `/usage` text read in local time.
export function resetTime(w, now) {
  if (w.resets_at) return w.resets_at;
  const m = w.resets && w.resets.trim().match(RESET_TEXT);
  if (!m) return null;
  const [, month, day, hour, minute, ampm] = m;
  const base = new Date(now * 1000);
  const h = (Number(hour) % 12) + (ampm.toLowerCase() === "pm" ? 12 : 0);
  const monthIndex = month ? MONTHS.indexOf(month.toLowerCase()) : base.getMonth();
  if (monthIndex < 0) return null;
  const date = new Date(base.getFullYear(), monthIndex, month ? Number(day) : base.getDate(), h, Number(minute || 0));
  let at = date.getTime() / 1000;
  // A reset is always ahead: "3pm" after 3pm means tomorrow, "Jan 2" in December next year.
  if (at < now - 5 * 60) {
    if (month) date.setFullYear(date.getFullYear() + 1);
    else date.setDate(date.getDate() + 1);
    at = date.getTime() / 1000;
  }
  return at;
}

// How window `kind` is going: share of it elapsed, and when it runs out at this pace.
export function pace(kind, w, now) {
  const length = WINDOWS[kind];
  const resetAt = resetTime(w, now);
  if (!length || !resetAt) return null;
  const left = resetAt - now;
  if (left <= 0 || left > length * 1.05) return null;
  const elapsed = Math.max(length - left, length * 0.01);
  const rate = w.used / elapsed;
  const exhaustAt = w.used >= 100 ? now : rate > 0 ? now + (100 - w.used) / rate : Infinity;
  return { resetAt, elapsed, length, fraction: Math.min(1, elapsed / length), exhaustAt, early: resetAt - exhaustAt };
}

// The rules, most serious first. `pace` warnings need the limit to run out well
// before the reset; `fresh` ones are for a week that has only just started.
const RULES = {
  seven_day: { high: 90, warn: 75, paceFrom: 15, margin: 12 * HOUR, again: 3 * HOUR, worse: 12 * HOUR, fresh: 2 * DAY },
  five_hour: { high: 95, warn: 80, paceFrom: 30, margin: 30 * 60, again: 30 * 60, worse: 30 * 60, fresh: 0 },
};

/**
 * Decides what (if anything) to warn about for the latest `limits`. `memory` is a
 * plain object kept between calls so each warning is said once per reset window
 * (pace warnings again only if things got noticeably worse). Returns null or
 * { phrase, kind, used, resetAt, exhaustAt, elapsed, urgent }.
 */
export function limitWarning(memory, limits, now) {
  const found = [];
  for (const kind of ["seven_day", "five_hour"]) {
    const w = limits?.[kind];
    if (!w) continue;
    const rule = RULES[kind];
    const p = pace(kind, w, now);
    const key = `${kind}:${p ? Math.round(p.resetAt / 600) : w.resets || ""}`;
    const seen = (memory[key] ||= { levels: [], paceAt: 0, exhaustAt: Infinity });
    const base = { kind, used: w.used, resetAt: p?.resetAt ?? null, exhaustAt: p?.exhaustAt ?? null, elapsed: p?.elapsed ?? null };
    const prefix = kind === "seven_day" ? "week" : "five";
    if (w.used >= rule.high && !seen.levels.includes("high")) {
      found.push({ ...base, rank: 0, phrase: `${prefix}_high`, urgent: kind === "seven_day", mark: () => seen.levels.push("high", "warn") });
    } else if (w.used >= rule.warn && !seen.levels.includes("warn")) {
      found.push({ ...base, rank: 3, phrase: `${prefix}_warn`, urgent: false, mark: () => seen.levels.push("warn") });
    }
    const fast = p && w.used >= rule.paceFrom && w.used < rule.high && p.early > rule.margin;
    const due = now - seen.paceAt > rule.again && (!seen.paceAt || p?.exhaustAt < seen.exhaustAt - rule.worse);
    if (fast && due) {
      const fresh = rule.fresh && p.elapsed < rule.fresh;
      const soon = p.exhaustAt - now < DAY;
      found.push({
        ...base,
        rank: fresh ? 1 : 2,
        phrase: fresh ? `${prefix}_pace_fresh` : `${prefix}_pace`,
        urgent: kind === "seven_day" && (fresh || soon),
        mark: () => Object.assign(seen, { paceAt: now, exhaustAt: p.exhaustAt }),
      });
    }
  }
  if (!found.length) return null;
  found.sort((a, b) => a.rank - b.rank || (a.kind === "seven_day" ? -1 : 1));
  const { mark, rank, ...warning } = found[0];
  mark();
  return warning;
}
