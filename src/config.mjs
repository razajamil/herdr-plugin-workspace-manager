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

function normalizeSetup(raw, layoutId) {
  if (raw == null) return null;
  if (typeof raw !== "object" || Array.isArray(raw)) {
    throw new ConfigError(`layout "${layoutId}": setup must be a mapping`);
  }
  const command = asString(raw.command, `layout "${layoutId}": setup.command`);
  const blocking = raw.blocking === undefined ? false : Boolean(raw.blocking);
  return { command, blocking };
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
  if (raw.ratio != null) {
    const ratio = Number(raw.ratio);
    if (!Number.isFinite(ratio) || ratio <= 0 || ratio >= 1) {
      throw new ConfigError(
        `layout "${layoutId}", tab "${tabTitle}": ratio must be a number between 0 and 1`,
      );
    }
    pane.ratio = ratio;
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
  return { repo, path: wsPath, defaultLayout };
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

  // Cross-check: every referenced defaultLayout must exist.
  for (const ws of workspaces) {
    if (ws.defaultLayout && !seen.has(ws.defaultLayout)) {
      throw new ConfigError(
        `workspace "${ws.path}" references unknown layout "${ws.defaultLayout}"`,
      );
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

// Find the layout to apply for a freshly created worktree. `target` may be a
// plain checkout-path string, or { checkoutPath, repoRoot, repoName }.
// Returns { workspace, layout } for the most specific match, or null.
export function matchWorkspaceLayout(config, target) {
  const t = typeof target === "string" ? { checkoutPath: target } : target ?? {};
  let best = null;
  let bestScore = -1;
  for (const ws of config.workspaces) {
    if (!ws.defaultLayout) continue;
    const score = matchScore(ws, t);
    if (score != null && score > bestScore) {
      const layout = findLayout(config, ws.defaultLayout);
      if (layout) {
        best = { workspace: ws, layout };
        bestScore = score;
      }
    }
  }
  return best;
}
