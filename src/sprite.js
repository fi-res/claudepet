// Pixel-art Clawd. Coordinates are in grid units; the idle frame matches the Clawd logo
// from the Claude Code CLI banner (quadrant blocks mapped to 1x2 grid cells).
export const GRID_W = 18;
export const GRID_H = 13;

export const BODY_COLOR = "#d77757";
export const EYE_COLOR = "#000000";

const BOOK_COVER = "#34558f";
const BOOK_PAGE = "#f4efe3";
const BOOK_FOLD = "#dcd5c4";
const BOOK_TEXT = "#9a9282";
const LAPTOP_LID = "#c3c9d0";
const LAPTOP_EDGE = "#8d959e";
const LAPTOP_LOGO = "#eef1f4";
const CAP_COLOR = "#5b74c9";
const POMPOM_COLOR = "#ffffff";
const SPINNER_COLOR = "#eb8f6e";

// Thinking spinner beside the head, a nod to Claude Code's "✻ Thinking…".
const SPINNER_FRAMES = [
  ["...", ".X.", "..."],
  [".X.", "XXX", ".X."],
  ["X.X", ".X.", "X.X"],
  ["XXX", "X.X", "XXX"],
  ["X.X", ".X.", "X.X"],
  [".X.", "XXX", ".X."],
];
export const SPINNER_LENGTH = SPINNER_FRAMES.length;

const PARTICLE_COLORS = {
  heart: "#ff5c7a",
  zzz: "#8fa3bf",
  spark: "#ffc94d",
  code: "#7fd38a",
};

const PATTERNS = {
  heart: [".XX.XX.", "XXXXXXX", "XXXXXXX", ".XXXXX.", "..XXX..", "...X..."],
  zzz: ["XXXXX", "...X.", "..X..", ".X...", "XXXXX"],
  spark: ["..X..", "..X..", "XX.XX", "..X..", "..X.."],
  code: ["..X...X.X..", ".X....X..X.", "X....X....X", ".X..X....X.", "..X.X...X.."],
};

function mix(a, b, t) {
  const pa = parseInt(a.slice(1), 16);
  const pb = parseInt(b.slice(1), 16);
  let out = 0;
  for (const shift of [16, 8, 0]) {
    const ca = (pa >> shift) & 0xff;
    const cb = (pb >> shift) & 0xff;
    out |= Math.round(ca + (cb - ca) * t) << shift;
  }
  return `#${out.toString(16).padStart(6, "0")}`;
}

const BLUSH_COLOR = mix(BODY_COLOR, "#ff4f79", 0.5);

const LEG_HEIGHTS = {
  stand: [2, 2, 2, 2],
  walk1: [1, 2, 1, 2],
  walk2: [2, 1, 2, 1],
  dangle: [3, 3, 3, 3],
  tuck: [1, 1, 1, 1],
};

// `s` is the size of one grid cell in device pixels.
export function drawPet(ctx, frame, s) {
  const { eyes, look, arms, legs, squash, blush, prop = null, cap = false, spinner = -1, flip = false } = frame;
  ctx.clearRect(0, 0, GRID_W * s, GRID_H * s);
  const px = (x0, y0, x1, y1, color) => {
    ctx.fillStyle = color;
    ctx.fillRect(x0 * s, y0 * s, (x1 - x0) * s, (y1 - y0) * s);
  };

  let [left, right, top, bottom] = [3, 15, 3, 11];
  if (squash > 0) [left, right, top] = [2, 16, 5];
  else if (squash < 0) [left, right, top] = [4, 14, 2];
  if (legs === "tuck") {
    top += 1;
    bottom += 1;
  }

  const heights = LEG_HEIGHTS[legs] || LEG_HEIGHTS.stand;
  [left + 1, left + 3, right - 4, right - 2].forEach((col, i) => {
    px(col, bottom, col + 1, bottom + heights[i], BODY_COLOR);
  });

  const arm = (side, style) => {
    if (style === "up") {
      if (side < 0) {
        px(left - 2, top - 1, left - 1, top + 3, BODY_COLOR);
        px(left - 1, top + 2, left, top + 3, BODY_COLOR);
      } else {
        px(right + 1, top - 1, right + 2, top + 3, BODY_COLOR);
        px(right, top + 2, right + 1, top + 3, BODY_COLOR);
      }
    } else if (style === "type1" || style === "type2") {
      // Hands bob up and down on the keyboard, alternating sides.
      const up = (style === "type1") === side < 0 ? 1 : 0;
      if (side < 0) px(left - 1, bottom - 3 - up, left, bottom - 1 - up, BODY_COLOR);
      else px(right, bottom - 3 - up, right + 1, bottom - 1 - up, BODY_COLOR);
    } else if (style === "hold") {
      // Hands are drawn on top of the book instead.
    } else if (style === "tuck") {
      if (side < 0) px(left - 1, bottom - 4, left, bottom - 2, BODY_COLOR);
      else px(right, bottom - 4, right + 1, bottom - 2, BODY_COLOR);
    } else if (side < 0) {
      px(left - 2, bottom - 4, left, bottom - 2, BODY_COLOR);
    } else {
      px(right, bottom - 4, right + 2, bottom - 2, BODY_COLOR);
    }
  };

  let leftArm = arms;
  let rightArm = arms;
  if (arms === "wave1") [leftArm, rightArm] = ["up", "normal"];
  else if (arms === "wave2") [leftArm, rightArm] = ["normal", "up"];
  arm(-1, leftArm);
  arm(1, rightArm);

  px(left, top, right, bottom, BODY_COLOR);

  const ey = top + 2;
  for (const [side, cx] of [[-1, left + 2 + look], [1, right - 3 + look]]) {
    switch (eyes) {
      case "blink":
        px(cx, ey + 1, cx + 1, ey + 2, EYE_COLOR);
        break;
      case "happy":
        px(cx - 1, ey + 1, cx, ey + 2, EYE_COLOR);
        px(cx, ey, cx + 1, ey + 1, EYE_COLOR);
        px(cx + 1, ey + 1, cx + 2, ey + 2, EYE_COLOR);
        break;
      case "surprised":
        px(cx, ey - 1, cx + 1, ey + 2, EYE_COLOR);
        break;
      case "dizzy":
        for (const [dx, dy] of [[-1, -1], [1, -1], [0, 0], [-1, 1], [1, 1]]) {
          px(cx + dx, ey + dy, cx + dx + 1, ey + dy + 1, EYE_COLOR);
        }
        break;
      case "sleep":
        px(cx - 1, ey + 1, cx + 2, ey + 2, EYE_COLOR);
        break;
      case "down":
        px(cx, ey + 1, cx + 1, ey + 3, EYE_COLOR);
        break;
      case "up":
        px(cx, ey - 1, cx + 1, ey + 1, EYE_COLOR);
        break;
      case "angry": {
        px(cx, ey, cx + 1, ey + 2, EYE_COLOR);
        const brow = side < 0 ? cx - 1 : cx + 1;
        px(brow, ey - 1, brow + 1, ey, EYE_COLOR);
        break;
      }
      default:
        px(cx, ey, cx + 1, ey + 2, EYE_COLOR);
    }
    if (blush) {
      if (side < 0) px(cx - 2, ey + 2, cx, ey + 3, BLUSH_COLOR);
      else px(cx + 1, ey + 2, cx + 3, ey + 3, BLUSH_COLOR);
    }
  }

  if (prop === "book") drawBook(px, left, right, bottom, flip);
  else if (prop === "laptop") drawLaptop(px, left, right, bottom);
  if (cap) drawCap(px, left, right, top);
  if (spinner >= 0) {
    SPINNER_FRAMES[spinner % SPINNER_FRAMES.length].forEach((row, y) => {
      [...row].forEach((ch, x) => {
        if (ch === "X") px(GRID_W - 3 + x, y, GRID_W - 2 + x, y + 1, SPINNER_COLOR);
      });
    });
  }
}

// Open book held up in front of the body, pages towards the viewer.
function drawBook(px, left, right, bottom, flip) {
  const x0 = left + 1;
  const x1 = right - 1;
  const mid = (x0 + x1) / 2;
  const y0 = bottom - 4;
  px(x0, y0, x1, bottom, BOOK_COVER);
  px(x0 + 1, y0, x1 - 1, bottom - 1, BOOK_PAGE);
  px(mid - 1, y0 + 1, mid + 1, bottom - 1, BOOK_FOLD);
  px(mid - 1, y0, mid + 1, y0 + 1, BODY_COLOR);
  px(x0 + 1, y0 + 1, mid - 1, y0 + 2, BOOK_TEXT);
  px(mid + 1, y0 + 1, x1 - 1, y0 + 2, BOOK_TEXT);
  if (flip) px(mid + 1, y0 - 1, x1 - 2, y0, BOOK_PAGE);
  px(x0, y0 + 1, x0 + 1, y0 + 3, BODY_COLOR);
  px(x1 - 1, y0 + 1, x1, y0 + 3, BODY_COLOR);
}

// Laptop on the ground in front of Clawd; we see the back of the lid.
function drawLaptop(px, left, right, bottom) {
  px(left + 1, bottom - 4, right - 1, bottom + 1, LAPTOP_LID);
  px(left + 1, bottom - 4, right - 1, bottom - 3, LAPTOP_EDGE);
  const mid = (left + right) / 2;
  px(mid - 1, bottom - 2, mid + 1, bottom - 1, LAPTOP_LOGO);
  px(left, bottom + 1, right, bottom + 2, LAPTOP_EDGE);
}

// Nightcap leaning to the right, with a pompom.
function drawCap(px, left, right, top) {
  px(left + 1, top - 1, right - 1, top, CAP_COLOR);
  px(left + 3, top - 2, right - 2, top - 1, CAP_COLOR);
  px(right - 4, top - 3, right - 1, top - 2, CAP_COLOR);
  px(right - 1, top - 3, right + 1, top - 1, POMPOM_COLOR);
}

export function particleCanvas(kind, s) {
  const rows = PATTERNS[kind];
  const canvas = document.createElement("canvas");
  canvas.width = rows[0].length * s;
  canvas.height = rows.length * s;
  const ctx = canvas.getContext("2d");
  ctx.fillStyle = PARTICLE_COLORS[kind];
  rows.forEach((row, y) => {
    [...row].forEach((ch, x) => {
      if (ch === "X") ctx.fillRect(x * s, y * s, s, s);
    });
  });
  return canvas;
}
