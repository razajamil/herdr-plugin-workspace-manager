import { readFileSync, existsSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { parseYaml } from "./yaml.mjs";

export class ConfigError extends Error {
  constructor(message) {
    super(message);
    this.name = "ConfigError";
  }
}

export const PLUGIN_ID = "herdr-plugin-workspace-manager";

export function expandHome(p) {
  if (typeof p !== "string") return p;
  if (p === "~") return homedir();
  if (p.startsWith("~/")) return path.join(homedir(), p.slice(2));
  return p;
}

// Candidate config file locations, most-preferred first. The herdr-managed
// config directory (HERDR_PLUGIN_CONFIG_DIR) is canonical; the ~/.herdr path is
// supported as a convenience fallback. HERDR_WSM_CONFIG overrides everything
// (used by tests).
export function configCandidates(env = process.env) {
  const candidates = [];
  if (env.HERDR_WSM_CONFIG) candidates.push(env.HERDR_WSM_CONFIG);
  const configDir = env.HERDR_PLUGIN_CONFIG_DIR;
  if (configDir) {
    candidates.push(path.join(configDir, "config.yml"));
    candidates.push(path.join(configDir, "config.yaml"));
  }
  const fallbackDir = path.join(homedir(), ".herdr", "plugins", PLUGIN_ID);
  candidates.push(path.join(fallbackDir, "config.yml"));
  candidates.push(path.join(fallbackDir, "config.yaml"));
  return candidates;
}

export function resolveConfigPath(env = process.env) {
  for (const candidate of configCandidates(env)) {
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

const SPLIT_ALIASES = {
  vertical: "right",
  horizontal: "down",
  right: "right",
  down: "down",
};

function asString(value, what) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new ConfigError(`${what} must be a non-empty string`);
  }
  return value;
}

// Compile a glob pattern (workspaces[].layoutMatching[].worktreePattern) into an
// anchored RegExp matched against a worktree's branch name. Only `*` (any run of
// characters, including "/") and `?` (a single character) are special; every
// other character is matched literally. The match is full-string, so
// `fix/rwr-*` matches `fix/rwr-123-foo` but not `hotfix/rwr-123-foo`.
export function globToRegExp(glob) {
  let body = "";
  for (const ch of glob) {
    if (ch === "*") body += ".*";
    else if (ch === "?") body += ".";
    else body += ch.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  }
  return new RegExp(`^${body}$`);
}

function normalizeSetup(raw, layoutId) {
  if (raw == null) return null;
  if (typeof raw !== "object" || Array.isArray(raw)) {
    throw new ConfigError(`layout "${layoutId}": setup must be a mapping`);
  }
  const command = asString(raw.command, `layout "${layoutId}": setup.command`);
  const blocking = raw.blocking === undefined ? false : Boolean(raw.blocking);
  return { command, blocking };
}

// Parse a pane `size` (the extent of THIS pane along the split axis: columns
// for a vertical/right split, rows for a horizontal/down split). Accepts three
// forms, normalized to { kind, value }:
//   "30%" (string)      -> { kind: "percent", value: 30 }
//   0.3   (0 < n < 1)   -> { kind: "percent", value: 30 }   (a fraction)
//   40    (integer >=1) -> { kind: "cells",   value: 40 }    (fixed columns/rows)
// The runner turns this into a herdr `--ratio` (a percent needs no lookup; a
// cell count is converted against the pane's live size at creation time).
function normalizeSize(raw, where) {
  if (typeof raw === "string") {
    const s = raw.trim();
    if (s.endsWith("%")) {
      const pct = Number(s.slice(0, -1).trim());
      if (!Number.isFinite(pct) || pct <= 0 || pct >= 100) {
        throw new ConfigError(`${where}: percentage must be between 0 and 100 (got "${raw}")`);
      }
      return { kind: "percent", value: pct };
    }
    const n = Number(s);
    if (s === "" || !Number.isFinite(n)) {
      throw new ConfigError(
        `${where}: must be a number of cells (e.g. 40), a fraction (e.g. 0.3), or a percentage (e.g. "30%")`,
      );
    }
    return numericSize(n, raw, where);
  }
  if (typeof raw === "number") return numericSize(raw, raw, where);
  throw new ConfigError(
    `${where}: must be a number of cells (e.g. 40), a fraction (e.g. 0.3), or a percentage (e.g. "30%")`,
  );
}

function numericSize(n, raw, where) {
  if (!Number.isFinite(n) || n <= 0) {
    throw new ConfigError(`${where}: must be a positive number (got ${JSON.stringify(raw)})`);
  }
  if (n < 1) return { kind: "percent", value: n * 100 }; // fraction of the axis
  if (!Number.isInteger(n)) {
    throw new ConfigError(
      `${where}: a fixed cell count must be a whole number (got ${n}); use a value below 1 ` +
        `or an "N%" string for a proportion`,
    );
  }
  return { kind: "cells", value: n };
}

function normalizePane(raw, layoutId, tabTitle, index) {
  if (typeof raw !== "object" || raw == null || Array.isArray(raw)) {
    throw new ConfigError(
      `layout "${layoutId}", tab "${tabTitle}": pane ${index} must be a mapping`,
    );
  }
  const pane = {
    title: raw.title != null ? asString(raw.title, "pane title") : null,
    command: raw.command != null ? asString(raw.command, "pane command") : null,
    setup: raw.setup === undefined ? false : Boolean(raw.setup),
    split: null,
    ratio: null,
    size: null,
  };
  if (raw.split != null) {
    const mapped = SPLIT_ALIASES[String(raw.split).toLowerCase()];
    if (!mapped) {
      throw new ConfigError(
        `layout "${layoutId}", tab "${tabTitle}": unsupported split "${raw.split}" ` +
          `(use vertical, horizontal, right, or down)`,
      );
    }
    pane.split = mapped;
  }
  if (raw.ratio != null && raw.size != null) {
    throw new ConfigError(
      `layout "${layoutId}", tab "${tabTitle}": set either "ratio" or "size", not both`,
    );
  }
  if (raw.ratio != null) {
    const ratio = Number(raw.ratio);
    if (!Number.isFinite(ratio) || ratio <= 0 || ratio >= 1) {
      throw new ConfigError(
        `layout "${layoutId}", tab "${tabTitle}": ratio must be a number between 0 and 1`,
      );
    }
    pane.ratio = ratio;
  }
  if (raw.size != null) {
    pane.size = normalizeSize(raw.size, `layout "${layoutId}", tab "${tabTitle}": size`);
  }
  return pane;
}

function normalizeTab(raw, layoutId, index) {
  if (typeof raw !== "object" || raw == null || Array.isArray(raw)) {
    throw new ConfigError(`layout "${layoutId}": tab ${index} must be a mapping`);
  }
  const title = raw.title != null ? asString(raw.title, "tab title") : null;
  if (!Array.isArray(raw.panes) || raw.panes.length === 0) {
    throw new ConfigError(
      `layout "${layoutId}", tab "${title ?? index}": needs at least one pane`,
    );
  }
  const panes = raw.panes.map((p, i) => normalizePane(p, layoutId, title ?? index, i));
  return { title, panes };
}

function normalizeLayout(raw, index) {
  if (typeof raw !== "object" || raw == null || Array.isArray(raw)) {
    throw new ConfigError(`layout ${index} must be a mapping`);
  }
  const id = asString(raw.id, `layout ${index}: id`);
  const setup = normalizeSetup(raw.setup, id);
  if (!Array.isArray(raw.tabs) || raw.tabs.length === 0) {
    throw new ConfigError(`layout "${id}": needs at least one tab`);
  }
  const tabs = raw.tabs.map((t, i) => normalizeTab(t, id, i));

  const setupPanes = tabs.flatMap((t) => t.panes).filter((p) => p.setup);
  if (setupPanes.length > 1) {
    throw new ConfigError(
      `layout "${id}": only one pane may set "setup: true" (found ${setupPanes.length})`,
    );
  }
  if (setup && setupPanes.length === 0) {
    throw new ConfigError(
      `layout "${id}": defines a setup command but no pane has "setup: true"`,
    );
  }
  return { id, setup, tabs };
}

function normalizeMatchRule(raw, wsIndex, i) {
  if (typeof raw !== "object" || raw == null || Array.isArray(raw)) {
    throw new ConfigError(`workspace ${wsIndex}: layoutMatching[${i}] must be a mapping`);
  }
  const title =
    raw.title != null
      ? asString(raw.title, `workspace ${wsIndex}: layoutMatching[${i}].title`)
      : null;
  const worktreePattern = asString(
    raw.worktreePattern,
    `workspace ${wsIndex}: layoutMatching[${i}].worktreePattern`,
  );
  const layout = asString(raw.layout, `workspace ${wsIndex}: layoutMatching[${i}].layout`);
  return { title, worktreePattern, layout, regex: globToRegExp(worktreePattern) };
}

function normalizeWorkspace(raw, index) {
  if (typeof raw !== "object" || raw == null || Array.isArray(raw)) {
    throw new ConfigError(`workspace ${index} must be a mapping`);
  }
  const repo = raw.repo != null ? asString(raw.repo, `workspace ${index}: repo`) : null;
  const wsPath = raw.path != null ? asString(raw.path, `workspace ${index}: path`) : null;
  if (!repo && !wsPath) {
    throw new ConfigError(
      `workspace ${index} needs "repo" (repo root/name — recommended) or "path" (worktree dir prefix)`,
    );
  }
  const defaultLayout =
    raw.defaultLayout != null
      ? asString(raw.defaultLayout, `workspace ${index}: defaultLayout`)
      : null;
  const layoutMatchingRaw = raw.layoutMatching ?? [];
  if (!Array.isArray(layoutMatchingRaw)) {
    throw new ConfigError(`workspace ${index}: layoutMatching must be a list`);
  }
  const layoutMatching = layoutMatchingRaw.map((r, i) => normalizeMatchRule(r, index, i));
  return { repo, path: wsPath, defaultLayout, layoutMatching };
}

export function validateConfig(raw) {
  if (raw == null) {
    return { layouts: [], workspaces: [] };
  }
  if (typeof raw !== "object" || Array.isArray(raw)) {
    throw new ConfigError("config root must be a mapping");
  }
  const layoutsRaw = raw.layouts ?? [];
  if (!Array.isArray(layoutsRaw)) throw new ConfigError("layouts must be a list");
  const layouts = layoutsRaw.map(normalizeLayout);

  const seen = new Set();
  for (const layout of layouts) {
    if (seen.has(layout.id)) {
      throw new ConfigError(`duplicate layout id "${layout.id}"`);
    }
    seen.add(layout.id);
  }

  const workspacesRaw = raw.workspaces ?? [];
  if (!Array.isArray(workspacesRaw)) throw new ConfigError("workspaces must be a list");
  const workspaces = workspacesRaw.map(normalizeWorkspace);

  // Cross-check: every layout referenced by a workspace must exist.
  for (const ws of workspaces) {
    const label = ws.repo ?? ws.path;
    if (ws.defaultLayout && !seen.has(ws.defaultLayout)) {
      throw new ConfigError(
        `workspace "${label}" references unknown layout "${ws.defaultLayout}"`,
      );
    }
    for (const rule of ws.layoutMatching) {
      if (!seen.has(rule.layout)) {
        throw new ConfigError(
          `workspace "${label}" layoutMatching references unknown layout "${rule.layout}"`,
        );
      }
    }
  }

  return { layouts, workspaces };
}

export function loadConfig(env = process.env) {
  const file = resolveConfigPath(env);
  if (!file) {
    return { path: null, config: { layouts: [], workspaces: [] } };
  }
  let text;
  try {
    text = readFileSync(file, "utf8");
  } catch (err) {
    throw new ConfigError(`cannot read config file ${file}: ${err.message}`);
  }
  const config = validateConfig(parseYaml(text));
  return { path: file, config };
}

export function findLayout(config, id) {
  return config.layouts.find((l) => l.id === id) ?? null;
}

// Is `checkoutPath` inside (or equal to) the configured workspace `wsPath`?
function isUnder(checkoutPath, wsPath) {
  const a = path.resolve(expandHome(checkoutPath));
  const b = path.resolve(expandHome(wsPath));
  return a === b || a.startsWith(b + path.sep);
}

// Score how specifically a workspace rule matches a target, or null for no
// match. Higher score wins. `repo` matches (by repo root or repo name) are
// preferred over `path` prefix matches, and longer path prefixes win among
// path matches.
function matchScore(ws, target) {
  if (ws.repo) {
    const repoResolved = path.resolve(expandHome(ws.repo));
    if (target.repoRoot && path.resolve(expandHome(target.repoRoot)) === repoResolved) {
      return 1_000_000;
    }
    if (target.repoName && target.repoName === ws.repo) return 900_000;
  }
  if (ws.path && target.checkoutPath && isUnder(target.checkoutPath, ws.path)) {
    return path.resolve(expandHome(ws.path)).length;
  }
  return null;
}

// Which layout does a matched workspace apply to a worktree on `branch`?
// layoutMatching rules are tried in the order the user wrote them; the first
// whose glob matches the branch (and whose layout exists) wins. When no rule
// matches — or there's no branch to match against (e.g. a detached HEAD) — the
// workspace's defaultLayout is used. Returns the layout object, or null if the
// workspace yields no applicable layout.
function resolveLayoutFor(config, ws, branch) {
  if (branch != null) {
    for (const rule of ws.layoutMatching) {
      if (rule.regex.test(branch)) {
        const layout = findLayout(config, rule.layout);
        if (layout) return layout;
      }
    }
  }
  if (ws.defaultLayout) return findLayout(config, ws.defaultLayout);
  return null;
}

// Find the layout to apply for a freshly created worktree. `target` may be a
// plain checkout-path string, or { checkoutPath, repoRoot, repoName, branch }.
// The most specific workspace (by repo/path) that actually yields a layout
// wins; within it, layoutMatching branch patterns are tried before
// defaultLayout. Returns { workspace, layout }, or null.
export function matchWorkspaceLayout(config, target) {
  const t = typeof target === "string" ? { checkoutPath: target } : target ?? {};
  let best = null;
  let bestScore = -1;
  for (const ws of config.workspaces) {
    const score = matchScore(ws, t);
    if (score == null || score <= bestScore) continue;
    const layout = resolveLayoutFor(config, ws, t.branch ?? null);
    if (layout) {
      best = { workspace: ws, layout };
      bestScore = score;
    }
  }
  return best;
}
