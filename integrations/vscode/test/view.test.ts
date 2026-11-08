import { strict as assert } from "node:assert";
import { test } from "node:test";

import type { ClaimView, StatusView } from "../src/cli";
import { age, describe, fileKey, holder, othersByFile, statusBarText } from "../src/view";

function claim(path: string, more: Partial<ClaimView> = {}): ClaimView {
  return {
    path,
    pilot: "sara@example.com",
    harness: "claude-code",
    seat_id: "seat_1",
    source: "local",
    origin: "agent",
    acquired_at: "2026-11-09T10:00:00Z",
    heartbeat_at: "2026-11-09T10:00:00Z",
    expires_at: "2026-11-09T10:15:00Z",
    ...more,
  };
}

test("file keys ignore case and separators on Windows only", () => {
  assert.equal(fileKey("C:\\Repo\\Src\\A.ts", "win32"), fileKey("c:/repo/src/a.ts", "win32"));
  assert.notEqual(fileKey("/repo/Src/A.ts", "linux"), fileKey("/repo/src/a.ts", "linux"));
});

test("other seats' claims are found by the editor's path", () => {
  const status: StatusView = {
    worktree: "/repo",
    gate_off: false,
    my_claims: [],
    other_claims: [claim("src/a.ts"), claim("src/a.ts", { seat_id: "seat_2" }), claim("b.ts")],
  };
  const map = othersByFile(status, "linux");
  assert.equal(map.get(fileKey("/repo/src/a.ts", "linux"))?.length, 2);
  assert.equal(map.get(fileKey("/repo/b.ts", "linux"))?.length, 1);
});

test("holders and ages read naturally", () => {
  const now = Date.parse("2026-11-09T10:04:30Z");
  assert.equal(age(now, "2026-11-09T10:04:00Z"), "30 s");
  assert.equal(age(now, "2026-11-09T10:00:00Z"), "4 min");
  assert.equal(age(now, "2026-11-09T07:00:00Z"), "3 h");
  assert.equal(holder(claim("a")), "sara@example.com (Claude Code)");
  assert.equal(
    holder(claim("a", { origin: "human", harness: "human", pilot: "lina@example.com" })),
    "lina@example.com (a person)",
  );
  assert.equal(holder(claim("a", { harness: undefined, source: "remote" })), "sara@example.com (an agent, another machine)");
  assert.equal(describe([claim("a")], now), "Claimed by sara@example.com (Claude Code) for 4 min.");
});

test("the status bar counts others' claims and yours", () => {
  assert.equal(statusBarText(2, 0), "ACS $(lock) 2");
  assert.equal(statusBarText(0, 1), "ACS $(lock) 0  $(person) 1");
});
