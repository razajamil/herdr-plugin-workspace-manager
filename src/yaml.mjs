// Minimal, dependency-free YAML parser.
//
// This intentionally supports only the subset of YAML the plugin config needs:
//   - block mappings            key: value
//   - block sequences           - item
//   - nesting via indentation   (spaces only; tabs are rejected)
//   - scalars                   plain / 'single' / "double" quoted strings,
//                               booleans, null, ints, floats
//   - comments                  # ... to end of line (ignored unless quoted)
//
// It does NOT support flow collections ({}, []), anchors/aliases, multi-line
// block scalars (|, >), or multiple documents. Those are not used by the config
// schema, and keeping the parser small keeps the plugin a single `node` away
// from working (no install step, so `herdr plugin link` works instantly).

export class YamlError extends Error {
  constructor(message, line) {
    super(line != null ? `${message} (line ${line + 1})` : message);
    this.name = "YamlError";
    this.line = line;
  }
}

// Strip a trailing `#` comment from a line, respecting quotes. A `#` only
// starts a comment when it is at the start of the (content) or preceded by
// whitespace — `a#b` is not a comment.
function stripComment(line) {
  let quote = null;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (quote) {
      if (ch === quote) quote = null;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (ch === "#" && (i === 0 || /\s/.test(line[i - 1]))) {
      return line.slice(0, i);
    }
  }
  return line;
}

// Tokenize into significant lines: { indent, content, line }.
function tokenize(text) {
  const nodes = [];
  const rawLines = text.split(/\r?\n/);
  for (let line = 0; line < rawLines.length; line++) {
    const raw = rawLines[line];
    if (raw.includes("\t")) {
      // Only reject tabs used for indentation; a tab inside a value is fine.
      const leading = raw.match(/^[ \t]*/)[0];
      if (leading.includes("\t")) {
        throw new YamlError("tabs are not allowed for indentation", line);
      }
    }
    const noComment = stripComment(raw);
    const trimmedEnd = noComment.replace(/\s+$/, "");
    if (trimmedEnd.trim() === "") continue; // blank / comment-only line
    if (trimmedEnd.trim() === "---") continue; // tolerate a document marker
    const indent = trimmedEnd.match(/^ */)[0].length;
    nodes.push({ indent, content: trimmedEnd.slice(indent), line });
  }
  return nodes;
}

function unescapeDouble(inner) {
  return inner.replace(/\\(["\\/nrt])/g, (_, c) => {
    switch (c) {
      case "n":
        return "\n";
      case "r":
        return "\r";
      case "t":
        return "\t";
      default:
        return c; // " \ /
    }
  });
}

function parseScalar(text, line) {
  const s = text.trim();
  if (s === "") return null;
  if (s[0] === '"') {
    if (s.length < 2 || s[s.length - 1] !== '"') {
      throw new YamlError("unterminated double-quoted string", line);
    }
    return unescapeDouble(s.slice(1, -1));
  }
  if (s[0] === "'") {
    if (s.length < 2 || s[s.length - 1] !== "'") {
      throw new YamlError("unterminated single-quoted string", line);
    }
    return s.slice(1, -1).replace(/''/g, "'");
  }
  const lower = s.toLowerCase();
  if (lower === "null" || s === "~") return null;
  if (lower === "true") return true;
  if (lower === "false") return false;
  if (/^[-+]?\d+$/.test(s)) return Number.parseInt(s, 10);
  if (/^[-+]?(\d+\.\d*|\.\d+|\d+)([eE][-+]?\d+)?$/.test(s)) {
    return Number.parseFloat(s);
  }
  return s;
}

// Find the index of the colon that separates a mapping key from its value:
// the first `:` that is at end-of-content or followed by whitespace, ignoring
// colons inside quotes. Returns -1 when the line is not a mapping entry.
function findKeyColon(content) {
  let quote = null;
  for (let i = 0; i < content.length; i++) {
    const ch = content[i];
    if (quote) {
      if (ch === quote) quote = null;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (ch === ":" && (i === content.length - 1 || /\s/.test(content[i + 1]))) {
      return i;
    }
  }
  return -1;
}

function parseKey(text, line) {
  const s = text.trim();
  if (s === "") throw new YamlError("empty mapping key", line);
  if (s[0] === '"' || s[0] === "'") return parseScalar(s, line);
  return s;
}

// Recursive descent over the token list. Each parse* returns [value, nextIndex].
function parseBlock(nodes, i, indent) {
  if (i >= nodes.length || nodes[i].indent < indent) return [null, i];
  if (isSeqItem(nodes[i].content)) return parseSeq(nodes, i, nodes[i].indent);
  return parseMap(nodes, i, nodes[i].indent);
}

function isSeqItem(content) {
  return content === "-" || content.startsWith("- ");
}

function parseMap(nodes, i, indent) {
  const obj = {};
  while (i < nodes.length && nodes[i].indent === indent && !isSeqItem(nodes[i].content)) {
    const { content, line } = nodes[i];
    const colon = findKeyColon(content);
    if (colon === -1) {
      throw new YamlError(`expected "key: value", got "${content}"`, line);
    }
    const key = parseKey(content.slice(0, colon), line);
    const after = content.slice(colon + 1).trim();
    if (after !== "") {
      obj[key] = parseScalar(after, line);
      i++;
    } else {
      // Value is a nested block on following lines. It may be indented deeper
      // (a mapping) or a sequence at the same indent as the key.
      i++;
      const next = nodes[i];
      if (next && (next.indent > indent || (next.indent === indent && isSeqItem(next.content)))) {
        const [child, ni] = parseBlock(nodes, i, next.indent);
        obj[key] = child;
        i = ni;
      } else {
        obj[key] = null;
      }
    }
  }
  return [obj, i];
}

function parseSeq(nodes, i, indent) {
  const arr = [];
  while (i < nodes.length && nodes[i].indent === indent && isSeqItem(nodes[i].content)) {
    const { content, line } = nodes[i];
    if (content === "-") {
      // Nested block begins on the following deeper lines.
      i++;
      const next = nodes[i];
      if (next && next.indent > indent) {
        const [child, ni] = parseBlock(nodes, i, next.indent);
        arr.push(child);
        i = ni;
      } else {
        arr.push(null);
      }
      continue;
    }
    const rest = content.slice(2); // after "- "
    const offset = content.length - rest.length; // column of `rest` within content
    if (findKeyColon(rest) !== -1) {
      // Inline mapping item: rewrite the dash line as a normal mapping line at
      // the deeper indent, then let parseMap consume it plus its continuation
      // lines. The next dash sits at `indent` (< mapIndent) so parseMap stops.
      const mapIndent = indent + offset;
      const view = nodes.slice();
      view[i] = { indent: mapIndent, content: rest, line };
      const [child, ni] = parseMap(view, i, mapIndent);
      arr.push(child);
      i = ni;
    } else {
      arr.push(parseScalar(rest, line));
      i++;
    }
  }
  return [arr, i];
}

export function parseYaml(text) {
  const nodes = tokenize(text);
  if (nodes.length === 0) return null;
  const [value, end] = parseBlock(nodes, 0, nodes[0].indent);
  if (end !== nodes.length) {
    throw new YamlError(
      `unexpected indentation`,
      nodes[end] ? nodes[end].line : undefined,
    );
  }
  return value;
}
