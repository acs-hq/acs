// When to claim and release the files a person edits. Pure logic with an injected clock, so the
// tests need no VS Code.
//
// - A file is claimed shortly after its first edit (`claimDelayMs`), not on every keystroke.
// - A held claim is refreshed every `refreshMs` (claims lapse after `claim_ttl_secs` otherwise).
// - It is released once the file is saved and has gone `releaseAfterIdleMs` without an edit or a
//   save, or at once when a saved file is closed.
// - A refused claim (another seat holds the file) is retried after `retryMs` while the file still
//   has changes.

/** Timing, in milliseconds. */
export interface TrackerOptions {
  claimDelayMs: number;
  refreshMs: number;
  releaseAfterIdleMs: number;
  retryMs: number;
}

export const DEFAULT_OPTIONS: TrackerOptions = {
  claimDelayMs: 750,
  refreshMs: 5 * 60_000,
  releaseAfterIdleMs: 120_000,
  retryMs: 30_000,
};

/** Something the extension should do now. */
export interface Action {
  kind: "claim" | "release";
  path: string;
}

interface FileState {
  /** The first edit not yet covered by a claim. */
  pendingSince?: number;
  /** The latest edit or save. */
  lastActivity: number;
  /** Unsaved changes. */
  dirty: boolean;
  /** Closed in the editor. */
  closed: boolean;
  /** This extension holds a claim on it. */
  held: boolean;
  /** When the claim was last taken or refreshed. */
  lastClaim?: number;
  /** A refused claim is not retried before this. */
  retryAt?: number;
  /** A claim or release is running. */
  busy: boolean;
}

export class ClaimTracker {
  private readonly files = new Map<string, FileState>();

  constructor(public options: TrackerOptions = DEFAULT_OPTIONS) {}

  /** The person changed the file's text. */
  edited(path: string, now: number): void {
    const state = this.state(path, now);
    state.dirty = true;
    state.closed = false;
    state.lastActivity = now;
    if (!state.held && state.pendingSince === undefined) {
      state.pendingSince = now;
    }
  }

  /** The file was saved. */
  saved(path: string, now: number): void {
    const state = this.files.get(path);
    if (state) {
      state.dirty = false;
      state.lastActivity = now;
    }
  }

  /** The file was closed; `dirty` says whether unsaved changes remain (a hot exit keeps them). */
  closed(path: string, dirty: boolean): void {
    const state = this.files.get(path);
    if (state) {
      state.closed = true;
      state.dirty = dirty;
    }
  }

  /** What to do now. Marks the returned files busy until their result is reported. */
  due(now: number): Action[] {
    const actions: Action[] = [];
    for (const [path, state] of this.files) {
      if (state.busy) {
        continue;
      }
      const action = this.next(state, now);
      if (action === "forget") {
        this.files.delete(path);
      } else if (action) {
        state.busy = true;
        actions.push({ kind: action, path });
      }
    }
    return actions;
  }

  private next(state: FileState, now: number): "claim" | "release" | "forget" | undefined {
    if (state.held) {
      const idle = now - state.lastActivity >= this.options.releaseAfterIdleMs;
      if (!state.dirty && (state.closed || idle)) {
        return "release";
      }
      if (state.lastClaim === undefined || now - state.lastClaim >= this.options.refreshMs) {
        return "claim";
      }
      return undefined;
    }
    if (state.pendingSince === undefined) {
      return state.closed && !state.dirty ? "forget" : undefined;
    }
    if (state.retryAt !== undefined) {
      // Refused before: retry while there are changes to protect; give up once they are saved.
      if (!state.dirty) {
        return "forget";
      }
      return now >= state.retryAt ? "claim" : undefined;
    }
    return now - state.pendingSince >= this.options.claimDelayMs ? "claim" : undefined;
  }

  /** The result of a claim started by [`due`]. */
  claimed(path: string, granted: boolean, now: number): void {
    const state = this.files.get(path);
    if (!state) {
      return;
    }
    state.busy = false;
    if (granted) {
      state.held = true;
      state.lastClaim = now;
      state.pendingSince = undefined;
      state.retryAt = undefined;
    } else {
      state.held = false;
      state.retryAt = now + this.options.retryMs;
    }
  }

  /** A claim or release could not be run at all (ACS missing, timeout): try again later. */
  failed(path: string, now: number): void {
    const state = this.files.get(path);
    if (state) {
      state.busy = false;
      state.retryAt = now + this.options.retryMs;
    }
  }

  /** The result of a release started by [`due`]. */
  released(path: string): void {
    this.files.delete(path);
  }

  /** The files this extension holds claims on. */
  held(): string[] {
    return [...this.files].filter(([, s]) => s.held).map(([path]) => path);
  }

  private state(path: string, now: number): FileState {
    let state = this.files.get(path);
    if (!state) {
      state = { lastActivity: now, dirty: false, closed: false, held: false, busy: false };
      this.files.set(path, state);
    }
    return state;
  }
}
