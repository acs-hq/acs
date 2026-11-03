// Judges a run of three_sessions.sh from the harnesses' own logs (Claude Code's stream, Antigravity's
// conversation transcript), not from ACS's records:
//
//   node check.mjs <repo> <logs>
//
// - No silent overwrite: every line that an Edit or Write added, by Claude Code's own report of
//   success, is still in the file. Lines, because an edit repeats its neighbours as an anchor and a
//   later session may rightly insert between them. Only each session's last write to a file counts:
//   its earlier ones are its own to change.
// - Every collision is visible: each file a blocked call names has that many in `acs status --json`.
// - No lost work: each task's function reached the shared file in HEAD.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const [repo, logs] = process.argv.slice(2);
const tasks = { discount: 'apply_discount', tax: 'apply_tax', rounding: 'round_to_nearest' };

// Claude Code reports Windows paths with either slash.
const relative = (file) =>
  path.relative(repo, file).split(path.sep).join('/');

// The lines of a text that carry anything, without their line endings.
const lines = (text) =>
  text.split('\n').map((line) => line.trimEnd()).filter((line) => line !== '');

function readSession(file) {
  const calls = new Map();
  const results = [];
  for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
    if (!line.trim()) continue;
    let event;
    try {
      event = JSON.parse(line);
    } catch {
      continue;
    }
    for (const block of event.message?.content ?? []) {
      if (block.type === 'tool_use') calls.set(block.id, block);
      if (block.type === 'tool_result') results.push(block);
    }
  }
  return results.flatMap((result) => {
    const call = calls.get(result.tool_use_id);
    if (!call) return [];
    const text = Array.isArray(result.content)
      ? result.content.map((part) => part.text ?? '').join('')
      : String(result.content ?? '');
    return [{ name: call.name, input: call.input, error: result.is_error === true, text }];
  });
}

// Antigravity's transcript: a step with `tool_calls` is followed by one result step per call, whose
// `status` is `ERROR` (with `error`) when the call failed or a hook denied it. Its file tools are
// mapped onto Claude Code's names so one check covers both.
function readAgySession(file) {
  const pending = [];
  const calls = [];
  for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
    let step;
    try {
      step = JSON.parse(line);
    } catch {
      continue;
    }
    if (Array.isArray(step.tool_calls)) {
      pending.push(...step.tool_calls);
      continue;
    }
    const call = step.source === 'MODEL' && step.type === 'GENERIC' ? pending.shift() : undefined;
    if (!call) continue;
    const args = call.args ?? {};
    const error = step.status === 'ERROR';
    const text = String(step.error ?? step.content ?? '');
    if (call.name === 'edit_file' || call.name === 'replace_file_content') {
      calls.push({
        name: 'Edit',
        input: {
          file_path: args.Path ?? args.TargetFile,
          old_string: args.Target ?? args.TargetContent,
          new_string: args.Replacement ?? args.ReplacementContent,
        },
        error,
        text,
      });
    } else if (call.name === 'write_file' || call.name === 'write_to_file') {
      calls.push({
        name: 'Write',
        input: { file_path: args.Path ?? args.TargetFile, content: args.Content ?? args.CodeContent },
        error,
        text,
      });
    } else if (call.name === 'run_command') {
      calls.push({ name: 'Bash', input: { command: args.CommandLine }, error, text });
    } else {
      calls.push({ name: call.name, input: args, error, text });
    }
  }
  return calls;
}

const sessions = fs
  .readdirSync(logs)
  .filter((name) => name.startsWith('round'))
  .sort()
  .flatMap((name) => {
    const file = path.join(logs, name);
    if (name.endsWith('.agy-transcript.ndjson')) {
      return [{ name: name.replace('.agy-transcript.ndjson', ' (antigravity)'), calls: readAgySession(file) }];
    }
    return name.endsWith('.jsonl') ? [{ name: name.replace('.jsonl', ''), calls: readSession(file) }] : [];
  });

const problems = [];
const writes = [];
const blocked = [];
const otherShellWrites = [];

for (const session of sessions) {
  for (const call of session.calls) {
    if (call.name === 'Edit' || call.name === 'Write') {
      const file = relative(call.input.file_path ?? '');
      const before = lines(call.input.old_string ?? '');
      const added = lines(call.input.new_string ?? call.input.content ?? '').filter(
        (line) => !before.includes(line),
      );
      if (!call.error) writes.push({ session: session.name, file, added });
    }
    const refusedClaim = call.name.endsWith('acs_claim') && /is claimed by/.test(call.text);
    if (refusedClaim || (call.error && /is claimed by/.test(call.text))) {
      const files = [...call.text.matchAll(/`([^`]+)` is claimed by/g)].map((match) => match[1]);
      blocked.push({ session: session.name, tool: call.name, files, text: call.text.slice(0, 200) });
    }
    if (call.name === 'Bash' && !call.error && />|sed -i|tee /.test(call.input.command ?? '')) {
      otherShellWrites.push({ session: session.name, command: call.input.command });
    }
  }
}

const last = new Map();
for (const write of writes) last.set(`${write.session} ${write.file}`, write);
let checked = 0;
for (const write of last.values()) {
  const file = path.join(repo, write.file);
  const text = fs.existsSync(file) ? lines(fs.readFileSync(file, 'utf8')) : [];
  checked += 1;
  for (const line of write.added.filter((added) => !text.includes(added))) {
    problems.push(`SILENT OVERWRITE: ${write.session} wrote ${write.file}, line gone: ${line}`);
  }
}

const view = JSON.parse(fs.readFileSync(path.join(logs, 'status-after.json'), 'utf8'));
const collisions = view.collisions ?? [];
const shown = new Map();
for (const collision of collisions) {
  for (const file of collision.paths) shown.set(file, (shown.get(file) ?? 0) + 1);
}
const wanted = new Map();
for (const item of blocked) {
  for (const file of item.files) wanted.set(file, (wanted.get(file) ?? 0) + 1);
}
for (const [file, count] of wanted) {
  if ((shown.get(file) ?? 0) < count) {
    problems.push(`UNSEEN COLLISION: ${count} blocked on ${file}, ${shown.get(file) ?? 0} in acs status`);
  }
}

const head = (file) => execFileSync('git', ['-C', repo, 'show', `HEAD:${file}`], { encoding: 'utf8' });
for (const [task, name] of Object.entries(tasks)) {
  if (!head('src/pricing.py').includes(`def ${name}`)) {
    problems.push(`LOST WORK: ${name} is not in src/pricing.py in HEAD (${task})`);
  }
}

console.log(`sessions: ${sessions.length}`);
console.log(`successful file writes: ${writes.length}, last per session and file checked: ${checked}`);
console.log(`blocked calls in the logs: ${blocked.length}`);
console.log(`collisions in acs status: ${view.collisions_total ?? collisions.length}`);
for (const item of blocked) console.log(`  ${item.session} ${item.tool}: ${item.text}`);
for (const item of otherShellWrites) console.log(`shell write, ${item.session}: ${item.command}`);
for (const problem of problems) console.log(problem);
console.log(problems.length === 0 ? 'PASS' : 'FAIL');
process.exit(problems.length === 0 ? 0 : 1);
