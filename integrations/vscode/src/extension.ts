// The VS Code side: claims the files the person edits, and shows the files others hold.
//
// Everything goes through the `acs` command line (`--json`): `acs status` every few seconds,
// `acs claim` shortly after the first edit of a file, `acs release` once it is saved and idle.

import { execFile } from "node:child_process";
import * as path from "node:path";

import * as vscode from "vscode";

import { AcsCli, AcsError, type ClaimView, type StatusView } from "./cli";
import { ClaimTracker, DEFAULT_OPTIONS, type TrackerOptions } from "./tracker";
import { claimFile, describe, fileKey, holder, othersByFile, statusBarText, age } from "./view";

/** A note about the same file is repeated at most this often. */
const NOTE_EVERY_MS = 10 * 60_000;
/** Files ACS refused for a reason that will not change soon are not retried before this. */
const SKIP_FOR_MS = 5 * 60_000;

/** One work tree with ACS, as last reported by `acs status`. */
interface Repo {
  /** The work tree root. */
  root: string;
  status: StatusView;
  others: Map<string, ClaimView[]>;
}

let controller: Controller | undefined;

export function activate(context: vscode.ExtensionContext): void {
  controller = new Controller(context);
  context.subscriptions.push(controller);
  void controller.start();
}

export async function deactivate(): Promise<void> {
  await controller?.releaseAll();
}

class Controller implements vscode.Disposable {
  private cli = new AcsCli(setting("path", "acs"));
  private readonly tracker = new ClaimTracker(trackerOptions());
  private repos = new Map<string, Repo>();
  private readonly ignored = new Map<string, boolean>();
  /** Files refused for a lasting reason (sealed), with when to try again. */
  private readonly skip = new Map<string, number>();
  private readonly notes = new Map<string, number>();
  private missingShown = false;
  private refreshing = false;
  private readonly disposables: vscode.Disposable[] = [];
  private readonly decorationChanged = new vscode.EventEmitter<vscode.Uri[]>();
  private readonly statusItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);
  private readonly gutter: vscode.TextEditorDecorationType;
  private refreshTimer: NodeJS.Timeout | undefined;
  private tickTimer: NodeJS.Timeout | undefined;

  constructor(context: vscode.ExtensionContext) {
    this.gutter = vscode.window.createTextEditorDecorationType({
      gutterIconPath: context.asAbsolutePath(path.join("media", "claimed.svg")),
      gutterIconSize: "contain",
      overviewRulerColor: new vscode.ThemeColor("acs.claimedByOthers"),
      overviewRulerLane: vscode.OverviewRulerLane.Right,
    });
    this.statusItem.command = "acs.showClaims";
    this.disposables.push(
      this.gutter,
      this.statusItem,
      this.decorationChanged,
      vscode.window.registerFileDecorationProvider({
        onDidChangeFileDecorations: this.decorationChanged.event,
        provideFileDecoration: (uri) => this.fileDecoration(uri),
      }),
      vscode.workspace.onDidChangeTextDocument((e) => this.onEdit(e)),
      vscode.workspace.onDidSaveTextDocument((doc) => {
        if (doc.uri.scheme === "file") {
          this.tracker.saved(doc.uri.fsPath, Date.now());
        }
      }),
      vscode.workspace.onDidCloseTextDocument((doc) => {
        if (doc.uri.scheme === "file") {
          this.tracker.closed(doc.uri.fsPath, doc.isDirty);
        }
      }),
      vscode.window.onDidChangeVisibleTextEditors(() => this.paintEditors()),
      vscode.workspace.onDidChangeWorkspaceFolders(() => void this.refresh()),
      vscode.workspace.onDidChangeConfiguration((e) => {
        if (e.affectsConfiguration("acs")) {
          this.cli = new AcsCli(setting("path", "acs"));
          this.tracker.options = trackerOptions();
          this.missingShown = false;
          this.schedule();
          void this.refresh();
        }
      }),
      vscode.commands.registerCommand("acs.showClaims", () => this.showClaims()),
      vscode.commands.registerCommand("acs.releaseMine", () => this.releaseMine()),
      vscode.commands.registerCommand("acs.refresh", () => this.refresh()),
    );
  }

  async start(): Promise<void> {
    this.schedule();
    await this.refresh();
  }

  dispose(): void {
    clearInterval(this.refreshTimer);
    clearInterval(this.tickTimer);
    for (const d of this.disposables) {
      d.dispose();
    }
  }

  private schedule(): void {
    clearInterval(this.refreshTimer);
    clearInterval(this.tickTimer);
    const every = Math.max(1, setting("refreshIntervalSeconds", 5)) * 1000;
    this.refreshTimer = setInterval(() => void this.refresh(), every);
    this.tickTimer = setInterval(() => void this.tick(), 1000);
  }

  // -------------------------------------------------------------------------------------------
  // Claiming
  // -------------------------------------------------------------------------------------------

  private onEdit(e: vscode.TextDocumentChangeEvent): void {
    // A reload from disk (an agent changed the file) also fires this, but leaves the document
    // clean: only edits that leave unsaved changes are the person's.
    if (!setting("claimOnEdit", true) || e.contentChanges.length === 0 || !e.document.isDirty) {
      return;
    }
    const uri = e.document.uri;
    if (uri.scheme !== "file" || !this.repoFor(uri.fsPath)) {
      return;
    }
    this.tracker.edited(uri.fsPath, Date.now());
  }

  private async tick(): Promise<void> {
    const now = Date.now();
    for (const action of this.tracker.due(now)) {
      const repo = this.repoFor(action.path);
      if (!repo) {
        this.tracker.released(action.path);
        continue;
      }
      if (action.kind === "release") {
        await this.release(repo, action.path);
      } else {
        await this.claim(repo, action.path);
      }
    }
  }

  private async claim(repo: Repo, file: string): Promise<void> {
    const key = fileKey(file);
    const skipUntil = this.skip.get(key);
    if ((skipUntil !== undefined && Date.now() < skipUntil) || (await this.isIgnored(repo, file))) {
      this.tracker.released(file);
      return;
    }
    try {
      const answer = await this.cli.claim(repo.root, [file]);
      if (answer.granted) {
        this.tracker.claimed(file, true, Date.now());
        return;
      }
      if (answer.reason_code === "sealed_path") {
        // Sealed paths bind agents, not people: editing them is normal, there is nothing to claim.
        this.skip.set(key, Date.now() + SKIP_FOR_MS);
        this.tracker.released(file);
        return;
      }
      this.tracker.claimed(file, false, Date.now());
      this.note(key, `ACS could not claim ${path.basename(file)} for you. ${answer.message}`);
    } catch (err) {
      this.tracker.failed(file, Date.now());
      this.problem(err);
    }
  }

  private async release(repo: Repo, file: string): Promise<void> {
    try {
      await this.cli.release(repo.root, [file]);
    } catch (err) {
      this.problem(err);
    }
    this.tracker.released(file);
  }

  /** Releases every claim this window holds (deactivation). */
  async releaseAll(): Promise<void> {
    const work = this.tracker.held().map(async (file) => {
      const repo = this.repoFor(file);
      if (repo) {
        await this.release(repo, file);
      }
    });
    await Promise.allSettled(work);
  }

  private async releaseMine(): Promise<void> {
    for (const repo of this.repos.values()) {
      try {
        await this.cli.run(["release", "--json", "--all"], repo.root, true);
      } catch (err) {
        this.problem(err);
      }
    }
    for (const file of this.tracker.held()) {
      this.tracker.released(file);
    }
    await this.refresh();
  }

  private isIgnored(repo: Repo, file: string): Promise<boolean> {
    const key = fileKey(file);
    const known = this.ignored.get(key);
    if (known !== undefined) {
      return Promise.resolve(known);
    }
    const rel = path.relative(repo.root, file);
    if (rel.split(path.sep)[0] === ".git") {
      return Promise.resolve(true);
    }
    return new Promise((resolve) => {
      execFile("git", ["check-ignore", "-q", "--", rel], { cwd: repo.root, windowsHide: true }, (error) => {
        // Exit 0: ignored. 1: not ignored. Anything else (no git): not ignored.
        const ignored = error === null;
        this.ignored.set(key, ignored);
        resolve(ignored);
      });
    });
  }

  // -------------------------------------------------------------------------------------------
  // Showing
  // -------------------------------------------------------------------------------------------

  async refresh(): Promise<void> {
    if (this.refreshing) {
      return;
    }
    this.refreshing = true;
    try {
      const files = new Map<string, string>();
      const remember = (repo: Repo): void => {
        for (const claim of repo.status.other_claims) {
          const file = claimFile(repo.status, claim);
          files.set(fileKey(file), file);
        }
      };
      this.repos.forEach(remember);
      const repos = new Map<string, Repo>();
      for (const folder of vscode.workspace.workspaceFolders ?? []) {
        if (folder.uri.scheme !== "file") {
          continue;
        }
        try {
          const status = await this.cli.status(folder.uri.fsPath);
          repos.set(fileKey(status.worktree), { root: status.worktree, status, others: othersByFile(status) });
        } catch (err) {
          if (!(err instanceof AcsError && err.code === "not_a_repo")) {
            this.problem(err);
          }
        }
      }
      this.repos = repos;
      repos.forEach(remember);
      this.decorationChanged.fire([...files.values()].map((file) => vscode.Uri.file(file)));
      this.paintEditors();
      this.paintStatus();
    } finally {
      this.refreshing = false;
    }
  }

  private fileDecoration(uri: vscode.Uri): vscode.FileDecoration | undefined {
    const claims = this.othersFor(uri.fsPath);
    if (!claims) {
      return undefined;
    }
    return {
      badge: "\u{1F512}",
      tooltip: describe(claims, Date.now()),
      color: new vscode.ThemeColor("acs.claimedByOthers"),
    };
  }

  private paintEditors(): void {
    for (const editor of vscode.window.visibleTextEditors) {
      const claims = editor.document.uri.scheme === "file" ? this.othersFor(editor.document.uri.fsPath) : undefined;
      if (!claims) {
        editor.setDecorations(this.gutter, []);
        continue;
      }
      const hover = new vscode.MarkdownString();
      hover.appendText(describe(claims, Date.now()));
      hover.appendMarkdown("\n\nEdits you make here may collide with theirs; ACS does not block your editor.");
      editor.setDecorations(this.gutter, [{ range: new vscode.Range(0, 0, 0, 0), hoverMessage: hover }]);
    }
  }

  private paintStatus(): void {
    if (this.repos.size === 0) {
      this.statusItem.hide();
      return;
    }
    let others = 0;
    let mine = 0;
    let off = false;
    let unavailable: string | undefined;
    for (const repo of this.repos.values()) {
      others += repo.status.other_claims.length;
      mine += repo.status.my_claims.length;
      off ||= repo.status.gate_off;
      unavailable ??= repo.status.state_unavailable;
    }
    this.statusItem.text = statusBarText(others, mine) + (off ? " (off)" : "");
    const lines = [
      `${others} file(s) claimed by agents or other people`,
      `${mine} file(s) claimed by you`,
    ];
    if (off) {
      lines.push("`acs off` is active: claims are not enforced.");
    }
    if (unavailable) {
      lines.push(`State unavailable: ${unavailable}`);
    }
    lines.push("Click to list claims.");
    this.statusItem.tooltip = lines.join("\n");
    this.statusItem.show();
  }

  private async showClaims(): Promise<void> {
    await this.refresh();
    type Item = vscode.QuickPickItem & { file?: string };
    const items: Item[] = [];
    const now = Date.now();
    for (const repo of this.repos.values()) {
      if (this.repos.size > 1) {
        items.push({ label: repo.root, kind: vscode.QuickPickItemKind.Separator });
      }
      for (const claim of repo.status.other_claims) {
        items.push({
          label: `$(lock) ${claim.path}`,
          description: holder(claim),
          detail: `for ${age(now, claim.acquired_at)}`,
          file: claimFile(repo.status, claim),
        });
      }
      for (const claim of repo.status.my_claims) {
        items.push({
          label: `$(person) ${claim.path}`,
          description: "you",
          detail: `for ${age(now, claim.acquired_at)}`,
          file: claimFile(repo.status, claim),
        });
      }
    }
    if (items.every((item) => !item.file)) {
      void vscode.window.showInformationMessage("ACS: no file is claimed right now.");
      return;
    }
    const picked = await vscode.window.showQuickPick(items, { placeHolder: "Claimed files", matchOnDescription: true });
    if (picked?.file) {
      await vscode.window.showTextDocument(vscode.Uri.file(picked.file));
    }
  }

  // -------------------------------------------------------------------------------------------
  // Helpers
  // -------------------------------------------------------------------------------------------

  private repoFor(file: string): Repo | undefined {
    const key = fileKey(file);
    let best: Repo | undefined;
    for (const [root, repo] of this.repos) {
      if ((key === root || key.startsWith(`${root}/`)) && (!best || root.length > fileKey(best.root).length)) {
        best = repo;
      }
    }
    return best;
  }

  private othersFor(file: string): ClaimView[] | undefined {
    return this.repoFor(file)?.others.get(fileKey(file));
  }

  private note(key: string, message: string): void {
    const last = this.notes.get(key);
    if (last !== undefined && Date.now() - last < NOTE_EVERY_MS) {
      return;
    }
    this.notes.set(key, Date.now());
    void vscode.window.showWarningMessage(message);
  }

  private problem(err: unknown): void {
    if (err instanceof AcsError && err.code === "not_found") {
      if (!this.missingShown) {
        this.missingShown = true;
        void vscode.window
          .showErrorMessage("ACS: cannot run `acs`. Install it, or set `acs.path`.", "Open Settings")
          .then((choice) => {
            if (choice) {
              void vscode.commands.executeCommand("workbench.action.openSettings", "acs.path");
            }
          });
      }
      return;
    }
    const message = err instanceof Error ? err.message : String(err);
    this.note(`problem:${message}`, `ACS: ${message}`);
  }
}

function setting<T>(name: string, fallback: T): T {
  return vscode.workspace.getConfiguration("acs").get<T>(name, fallback);
}

function trackerOptions(): TrackerOptions {
  return {
    ...DEFAULT_OPTIONS,
    releaseAfterIdleMs: Math.max(5, setting("releaseAfterIdleSeconds", 120)) * 1000,
  };
}
