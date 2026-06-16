import { spawnSync } from "node:child_process";

export class HerdrError extends Error {
  constructor(message, { args, stderr } = {}) {
    super(message);
    this.name = "HerdrError";
    this.args = args;
    this.stderr = stderr;
  }
}

export function herdrBin(env = process.env) {
  return env.HERDR_BIN_PATH || "herdr";
}

// Run a herdr CLI command. Returns { status, stdout, stderr }.
export function runHerdr(args, { env = process.env } = {}) {
  const bin = herdrBin(env);
  const res = spawnSync(bin, args, {
    encoding: "utf8",
    env,
    maxBuffer: 16 * 1024 * 1024,
  });
  if (res.error) {
    throw new HerdrError(`failed to spawn ${bin}: ${res.error.message}`, { args });
  }
  return { status: res.status ?? 0, stdout: res.stdout ?? "", stderr: res.stderr ?? "" };
}

// Run a herdr command that returns JSON; return its `result` object.
// Throws HerdrError on a non-zero exit or a `{ error: ... }` envelope.
export function runHerdrJson(args, opts = {}) {
  const { status, stdout, stderr } = runHerdr(args, opts);
  let parsed;
  const trimmed = stdout.trim();
  if (trimmed) {
    try {
      parsed = JSON.parse(trimmed);
    } catch {
      parsed = null;
    }
  }
  if (parsed && parsed.error) {
    const { code, message } = parsed.error;
    throw new HerdrError(`herdr ${args.join(" ")} -> ${code}: ${message}`, {
      args,
      stderr,
    });
  }
  if (status !== 0) {
    throw new HerdrError(
      `herdr ${args.join(" ")} exited ${status}: ${stderr.trim() || stdout.trim()}`,
      { args, stderr },
    );
  }
  return parsed ? parsed.result : null;
}

// Extract a pane id from any herdr result shape we care about
// (pane split -> result.pane.pane_id, tab create -> result.root_pane.pane_id).
export function paneIdOf(result) {
  if (!result || typeof result !== "object") return null;
  return (
    result.pane_id ??
    result.pane?.pane_id ??
    result.root_pane?.pane_id ??
    null
  );
}

export function tabIdOf(result) {
  if (!result || typeof result !== "object") return null;
  return result.tab_id ?? result.tab?.tab_id ?? null;
}
