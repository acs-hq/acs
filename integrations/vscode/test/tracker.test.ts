import { strict as assert } from "node:assert";
import { test } from "node:test";

import { ClaimTracker, type TrackerOptions } from "../src/tracker";

const options: TrackerOptions = { claimDelayMs: 750, refreshMs: 300_000, releaseAfterIdleMs: 120_000, retryMs: 30_000 };

test("a file is claimed shortly after its first edit, not on every keystroke", () => {
  const t = new ClaimTracker(options);
  t.edited("a.ts", 0);
  t.edited("a.ts", 400);
  assert.deepEqual(t.due(500), []);
  assert.deepEqual(t.due(800), [{ kind: "claim", path: "a.ts" }]);
  // Busy until the result comes back.
  assert.deepEqual(t.due(900), []);
  t.claimed("a.ts", true, 1000);
  t.edited("a.ts", 1500);
  assert.deepEqual(t.due(2000), [], "more edits do not claim again");
  assert.deepEqual(t.held(), ["a.ts"]);
});

test("a held claim is refreshed while the file is being worked on", () => {
  const t = new ClaimTracker(options);
  t.edited("a.ts", 0);
  t.due(1000);
  t.claimed("a.ts", true, 1000);
  t.edited("a.ts", 200_000);
  assert.deepEqual(t.due(301_000), [{ kind: "claim", path: "a.ts" }]);
});

test("a saved file is released after the idle window, and a closed one at once", () => {
  const t = new ClaimTracker(options);
  for (const file of ["a.ts", "b.ts"]) {
    t.edited(file, 0);
  }
  t.due(1000);
  t.claimed("a.ts", true, 1000);
  t.claimed("b.ts", true, 1000);
  t.saved("a.ts", 5000);
  t.saved("b.ts", 5000);
  t.closed("b.ts", false);
  assert.deepEqual(t.due(6000), [{ kind: "release", path: "b.ts" }]);
  t.released("b.ts");
  assert.deepEqual(t.due(124_000), [], "edited or saved too recently");
  assert.deepEqual(t.due(125_000), [{ kind: "release", path: "a.ts" }]);
  t.released("a.ts");
  assert.deepEqual(t.held(), []);
});

test("unsaved changes keep the claim, even when the file is closed", () => {
  const t = new ClaimTracker(options);
  t.edited("a.ts", 0);
  t.due(1000);
  t.claimed("a.ts", true, 1000);
  t.closed("a.ts", true);
  assert.deepEqual(t.due(200_000), [], "dirty: not released");
  assert.deepEqual(t.held(), ["a.ts"]);
});

test("a refused claim is retried while there are changes, and dropped once saved", () => {
  const t = new ClaimTracker(options);
  t.edited("a.ts", 0);
  t.due(1000);
  t.claimed("a.ts", false, 1000);
  assert.deepEqual(t.due(20_000), []);
  assert.deepEqual(t.due(31_000), [{ kind: "claim", path: "a.ts" }]);
  t.claimed("a.ts", false, 31_000);
  t.saved("a.ts", 40_000);
  assert.deepEqual(t.due(70_000), []);
  assert.deepEqual(t.held(), []);
  t.edited("a.ts", 80_000);
  assert.deepEqual(t.due(81_000), [{ kind: "claim", path: "a.ts" }], "a new edit tries again");
});

test("a claim that could not run is retried later", () => {
  const t = new ClaimTracker(options);
  t.edited("a.ts", 0);
  t.due(1000);
  t.failed("a.ts", 1000);
  assert.deepEqual(t.due(2000), []);
  assert.deepEqual(t.due(31_000), [{ kind: "claim", path: "a.ts" }]);
});
