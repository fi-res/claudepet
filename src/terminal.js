// Terminal-style rendering for commands Clawd offers and what they print:
// shell syntax colours like Claude Code's command blocks, and ANSI colours in output.

// Longest first, so "&&" wins over "&".
const OPERATORS = ["2>&1", "&&", "||", ">>", "2>", "|", ";", "&", ">", "<", "(", ")", "{", "}"];
// After these a new command starts.
const STARTS_COMMAND = new Set(["&&", "||", "|", ";", "&", "(", "{", "$("]);
const WORD_END = /[\s;|&<>(){}'"]/;

// Splits a bash or PowerShell command into [type, text] tokens. Types: space, cmd,
// arg, flag, number, string, var, op, comment. Good enough for colouring, not parsing.
export function tokenize(command) {
  const tokens = [];
  let expectCommand = true;
  let i = 0;
  while (i < command.length) {
    const rest = command.slice(i);
    const ch = command[i];
    let m;
    if ((m = rest.match(/^\s+/))) {
      tokens.push(["space", m[0]]);
      if (m[0].includes("\n")) expectCommand = true;
      i += m[0].length;
      continue;
    }
    if (ch === "#" && (i === 0 || /\s/.test(command[i - 1]))) {
      const end = command.indexOf("\n", i);
      const text = end < 0 ? rest : command.slice(i, end);
      tokens.push(["comment", text]);
      i += text.length;
      continue;
    }
    if (ch === "'" || ch === '"') {
      let j = i + 1;
      while (j < command.length && command[j] !== ch) j += command[j] === "\\" && ch === '"' ? 2 : 1;
      const text = command.slice(i, j + 1);
      tokens.push(["string", text]);
      expectCommand = false;
      i += text.length;
      continue;
    }
    if (rest.startsWith("$(")) {
      tokens.push(["op", "$("]);
      expectCommand = true;
      i += 2;
      continue;
    }
    if ((m = rest.match(/^\$(\{[^}]*\}|env:\w+|[\w?_]+)/i))) {
      tokens.push(["var", m[0]]);
      expectCommand = false;
      i += m[0].length;
      continue;
    }
    const op = OPERATORS.find((o) => rest.startsWith(o));
    if (op) {
      tokens.push(["op", op]);
      expectCommand = STARTS_COMMAND.has(op);
      i += op.length;
      continue;
    }
    let j = i;
    while (j < command.length && !WORD_END.test(command[j])) j++;
    const word = command.slice(i, Math.max(j, i + 1));
    let type;
    if (expectCommand && /^\w+=/.test(word)) type = "var"; // FOO=bar cmd
    else if (expectCommand) type = "cmd";
    else if (/^--?[A-Za-z]/.test(word)) type = "flag";
    else if (/^\d+([.,]\d+)?$/.test(word)) type = "number";
    else type = "arg";
    if (type !== "var") expectCommand = false;
    tokens.push([type, word]);
    i += word.length;
  }
  return tokens;
}

const span = (className, text) => {
  const el = document.createElement("span");
  el.className = className;
  el.textContent = text;
  return el;
};

// "$ command" with each token in its colour.
export function highlightCommand(command) {
  const out = document.createDocumentFragment();
  out.append(span("tok-prompt", "$ "));
  for (const [type, text] of tokenize(command)) {
    out.append(type === "space" ? document.createTextNode(text) : span(`tok-${type}`, text));
  }
  return out;
}

const ANSI = {
  30: "#5c6370", 31: "#e06c75", 32: "#98c379", 33: "#e5c07b", 34: "#61afef", 35: "#c678dd", 36: "#56b6c2", 37: "#dcdfe4",
  90: "#7f848e", 91: "#ff7b86", 92: "#b5e890", 93: "#ffd68a", 94: "#82c4ff", 95: "#e09cff", 96: "#7fd8e3", 97: "#ffffff",
};

// Command output with ANSI colour codes turned into coloured spans; other escape
// sequences (cursor movement and such) are dropped.
export function renderAnsi(text) {
  const out = document.createDocumentFragment();
  let color = null;
  let bold = false;
  const parts = text.replace(/\x1b\[[0-9;?]*[A-Za-ln-z]/g, "").split(/\x1b\[([0-9;]*)m/);
  parts.forEach((part, index) => {
    if (index % 2 === 1) {
      for (const code of (part || "0").split(";").map(Number)) {
        if (code === 0) [color, bold] = [null, false];
        else if (code === 1) bold = true;
        else if (code === 22) bold = false;
        else if (code === 39) color = null;
        else if (ANSI[code]) color = ANSI[code];
      }
      return;
    }
    if (!part) return;
    if (!color && !bold) return out.append(document.createTextNode(part));
    const el = span("", part);
    if (color) el.style.color = color;
    if (bold) el.style.fontWeight = "700";
    out.append(el);
  });
  return out;
}
