// Turning `acs status --json` into what the editor shows. No VS Code imports.

import * as path from "node:path";

import type { ClaimView, StatusView } from "./cli";

/** A key for an absolute file path that matches however the editor spells it. */
export function fileKey(file: string, platform: NodeJS.Platform = process.platform): string {
  const normal = path.normalize(file).replace(/\\/g, "/").replace(/\/+$/, "");
  return platform === "win32" || platform === "darwin" ? normal.toLowerCase() : normal;
}

/** The absolute path of a claim's file. */
export function claimFile(status: StatusView, claim: ClaimView): string {
  return path.join(status.worktree, ...claim.path.split("/"));
}

/** Other seats' claims, by [`fileKey`] of the file. */
export function othersByFile(
  status: StatusView,
  platform: NodeJS.Platform = process.platform,
): Map<string, ClaimView[]> {
  const map = new Map<string, ClaimView[]>();
  for (const claim of status.other_claims) {
    const key = fileKey(claimFile(status, claim), platform);
    const list = map.get(key) ?? [];
    list.push(claim);
    map.set(key, list);
  }
  return map;
}

/** "4 min", "2 h": how long ago `iso` was, in the coarsest unit. */
export function age(nowMs: number, iso: string): string {
  const secs = Math.max(0, Math.round((nowMs - Date.parse(iso)) / 1000));
  if (secs < 60) {
    return `${secs} s`;
  }
  if (secs < 3600) {
    return `${Math.floor(secs / 60)} min`;
  }
  if (secs < 86_400) {
    return `${Math.floor(secs / 3600)} h`;
  }
  return `${Math.floor(secs / 86_400)} d`;
}

/** Who holds a claim: "sara@example.com (Claude Code)", "lina@example.com (a person)". */
export function holder(claim: ClaimView): string {
  let what: string;
  if (claim.origin === "human") {
    what = "a person";
  } else if (claim.harness) {
    what = harnessName(claim.harness);
  } else {
    what = "an agent";
  }
  const where = claim.source === "remote" ? ", another machine" : "";
  return `${claim.pilot} (${what}${where})`;
}

/** One line per holder: "Claimed by sara@example.com (Claude Code) for 4 min." */
export function describe(claims: ClaimView[], nowMs: number): string {
  return claims
    .map((claim) => {
      const symbol = claim.symbol_path ? ` (${claim.symbol_path})` : "";
      return `Claimed${symbol} by ${holder(claim)} for ${age(nowMs, claim.acquired_at)}.`;
    })
    .join("\n");
}

/** The status bar text: claims held by others, and by you. */
export function statusBarText(others: number, mine: number): string {
  const parts = [`$(lock) ${others}`];
  if (mine > 0) {
    parts.push(`$(person) ${mine}`);
  }
  return `ACS ${parts.join("  ")}`;
}

function harnessName(harness: string): string {
  switch (harness) {
    case "claude-code":
      return "Claude Code";
    case "antigravity":
      return "Antigravity";
    case "human":
      return "a person";
    default:
      return harness;
  }
}
