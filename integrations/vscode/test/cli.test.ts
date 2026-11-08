// The extension's CLI client against the real `acs` binary: a claim the extension takes for the
// person blocks an agent's edit, and the agent's denial names the person (WP7.3's acceptance).
//
// Uses `ACS_BIN`, or the workspace's `target/debug/acs`; skipped when neither exists.

import { strict as assert } from "node:assert";
import { execFileSync } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { test } from "node:test";

import { AcsCli } from "../src/cli";

const exe = process.platform === "win32" ? "acs.exe" : "acs";
const bin = process.env.ACS_BIN ?? path.resolve(__dirname, "..", "..", "..", "..", "target", "debug", exe);
const skip = fs.existsSync(bin) ? false : `no acs binary at ${bin} (build it, or set ACS_BIN)`;

interface World {
  repo: string;
  env: NodeJS.ProcessEnv;
}

function world(): World {
  const base = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "acs-vscode-")));
  const repo = path.join(base, "repo");
  fs.mkdirSync(path.join(repo, "src"), { recursive: true });
  const gitconfig = path.join(base, "gitconfig");
  fs.writeFileSync(gitconfig, "[user]\n\temail = sara@example.com\n\tname = Sara\n");
  const env = {
    ...process.env,
    ACS_HOME: path.join(base, "acs-home"),
    GIT_CONFIG_GLOBAL: gitconfig,
    GIT_CONFIG_NOSYSTEM: "1",
  };
  const git = (...args: string[]) => execFileSync("git", args, { cwd: repo, env, stdio: "ignore" });
  git("init", "-q", "-b", "main", ".");
  fs.writeFileSync(path.join(repo, "src", "a.ts"), "export const a = 1;\n");
  fs.writeFileSync(path.join(repo, "src", "b.ts"), "export const b = 1;\n");
  git("add", "-A");
  git("commit", "-q", "-m", "init");
  return { repo, env };
}

interface Decision {
  outcome: string;
  reason_code: string;
  message: string;
}

function agentEdit(w: World, file: string): Decision {
  const event = {
    spec: "acs/0.1",
    harness: "claude-code",
    harness_session_id: "s1",
    phase: "pre_tool",
    cwd: w.repo,
    tool: { name: "Edit", kind: "edit" },
    paths: [file],
  };
  const out = execFileSync(bin, ["hook", "--canonical", "pre_tool"], {
    cwd: w.repo,
    env: w.env,
    input: JSON.stringify(event),
  });
  return JSON.parse(out.toString()) as Decision;
}

test("a claim taken by the extension blocks an agent, and the denial names the person", { skip }, async () => {
  const w = world();
  const cli = new AcsCli(bin, w.env);
  const answer = await cli.claim(w.repo, [path.join(w.repo, "src", "a.ts")]);
  assert.equal(answer.granted, true, JSON.stringify(answer));

  const status = await cli.status(w.repo);
  assert.deepEqual(
    status.my_claims.map((c) => [c.path, c.origin]),
    [["src/a.ts", "human"]],
  );

  const denied = agentEdit(w, "src/a.ts");
  assert.equal(denied.outcome, "deny");
  assert.equal(denied.reason_code, "claim_conflict");
  assert.match(denied.message, /sara@example\.com \(human/);

  const released = await cli.release(w.repo, ["src/a.ts"]);
  assert.deepEqual(released.released, ["src/a.ts"]);
  assert.equal(agentEdit(w, "src/a.ts").outcome, "allow");
});

test("a file an agent holds is refused, with the agent named", { skip }, async () => {
  const w = world();
  assert.equal(agentEdit(w, "src/b.ts").outcome, "allow");
  const cli = new AcsCli(bin, w.env);
  const answer = await cli.claim(w.repo, ["src/b.ts"]);
  assert.equal(answer.granted, false);
  if (!answer.granted) {
    assert.equal(answer.reason_code, "claim_conflict");
    assert.match(answer.message, /claude-code/);
  }
  const others = (await cli.status(w.repo)).other_claims;
  assert.deepEqual(
    others.map((c) => [c.path, c.harness, c.origin]),
    [["src/b.ts", "claude-code", "agent"]],
  );
});

test("a missing acs is reported as not found", async () => {
  const cli = new AcsCli(path.join(os.tmpdir(), "no-such-acs-binary"));
  await assert.rejects(cli.status(os.tmpdir()), (err: Error & { code?: string }) => err.code === "not_found");
});
