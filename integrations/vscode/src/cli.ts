// The only way the extension talks to ACS: the `acs` command line with `--json`.
//
// No VS Code imports here, so the tests can drive the real binary from plain Node.

import { execFile } from "node:child_process";

/** A claim as `acs status --json` reports it. */
export interface ClaimView {
  path: string;
  symbol_path?: string;
  pilot: string;
  harness?: string;
  seat_id: string;
  source: "local" | "remote";
  origin: "agent" | "human";
  acquired_at: string;
  heartbeat_at: string;
  expires_at: string;
}

/** The parts of `acs status --json` the extension uses. */
export interface StatusView {
  worktree: string;
  gate_off: boolean;
  state_unavailable?: string;
  seat?: { seat_id: string; pilot: string; harness: string };
  my_claims: ClaimView[];
  other_claims: ClaimView[];
}

/** What `acs claim --json` answers. */
export type ClaimAnswer =
  | { ok: true; granted: true; paths: string[]; claim_ids: string[]; warnings: string[] }
  | { ok: false; granted: false; reason_code: string; message: string };

/** What `acs release --json` answers. */
export interface ReleaseAnswer {
  ok: boolean;
  released: string[];
  not_held: string[];
  outside_repository: string[];
}

/** A failure to run `acs` at all, or an error it reported. */
export class AcsError extends Error {
  constructor(
    message: string,
    /** `not_found` when the program could not be started; otherwise ACS's own error code. */
    readonly code: string,
  ) {
    super(message);
  }
}

/** Runs `acs` commands for one work tree. */
export class AcsCli {
  constructor(
    /** The `acs` program (a bare name is looked up on `PATH`). */
    readonly program: string,
    /** Extra environment, for tests (`ACS_HOME`, git configuration). */
    readonly env: NodeJS.ProcessEnv = process.env,
    readonly timeoutMs = 15_000,
  ) {}

  status(cwd: string): Promise<StatusView> {
    return this.run<StatusView>(["status", "--json"], cwd);
  }

  /** Claims `paths` (relative to `cwd`, or absolute) for the person's own seat. */
  claim(cwd: string, paths: string[]): Promise<ClaimAnswer> {
    return this.run<ClaimAnswer>(["claim", "--json", "--", ...paths], cwd, true);
  }

  release(cwd: string, paths: string[]): Promise<ReleaseAnswer> {
    return this.run<ReleaseAnswer>(["release", "--json", "--", ...paths], cwd, true);
  }

  /**
   * Runs one command and parses its JSON. With `answerOnFailure`, a non-zero exit with a JSON
   * answer (a refused claim) is an answer, not an error.
   */
  run<T>(args: string[], cwd: string, answerOnFailure = false): Promise<T> {
    return new Promise((resolve, reject) => {
      execFile(
        this.program,
        args,
        { cwd, env: this.env, timeout: this.timeoutMs, windowsHide: true, maxBuffer: 16 * 1024 * 1024 },
        (error, stdout, stderr) => {
          if (error && (error as NodeJS.ErrnoException).code === "ENOENT") {
            reject(new AcsError(`cannot run \`${this.program}\`: not found`, "not_found"));
            return;
          }
          let parsed: unknown;
          try {
            parsed = JSON.parse(stdout);
          } catch {
            const detail = (stderr || stdout || String(error ?? "")).trim();
            reject(new AcsError(`\`acs ${args[0]}\` gave no JSON: ${detail}`, "bad_output"));
            return;
          }
          const failure = parsed as { ok?: boolean; error?: { code: string; message: string } };
          if (failure.ok === false && failure.error) {
            reject(new AcsError(failure.error.message, failure.error.code));
            return;
          }
          if (error && !answerOnFailure) {
            reject(new AcsError(`\`acs ${args[0]}\` failed: ${stderr.trim()}`, "failed"));
            return;
          }
          resolve(parsed as T);
        },
      );
    });
  }
}
