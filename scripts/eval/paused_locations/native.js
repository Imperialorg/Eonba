'use strict';

// Explicit Linux x86-64 evidence for one reviewed, side-effect-free call in two
// production eon-voice files. No project/program/adapter/breakpoint arguments.
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { createHash } = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events');
const assert = require('node:assert/strict');
const { repo, workDirectory, editor } = require('./host');

const project = path.join(repo, 'engine/core-rs/crates/eon-voice');
const llvm = '/usr/lib/llvm-18/bin';
const inputs = [
  ['src/endpoint.rs', '24c839b1f80898e6bb29e1abe2a81c27b588768b65974815b8e7558c6b6e8a22', '-> Option<Self> {\n        let text = text.trim().trim_end_matches('],
  ['src/pre_cognition.rs', '3fb0812fd2487460faa3c4abc7404c3e20d016e997573ad016d9bae9cf190a92', '&& let Some(hint) = ReflexHint::from_partial(text)'],
];

function checked(file, digest) {
  const st = fs.lstatSync(file);
  assert.ok(st.isFile() && st.size < 64 * 1024 && !(st.mode & 0o022));
  const bytes = fs.readFileSync(file);
  assert.equal(createHash('sha256').update(bytes).digest('hex'), digest);
  return bytes.toString('utf8');
}

// Bounded, private stdio peer for this one scripted interaction. No reverse requests.
async function* messages(stdout) {
  let buf = Buffer.alloc(0);
  let count = 0;
  for await (const chunk of stdout) {
    buf = Buffer.concat([buf, chunk]);
    while (buf.length) {
      const end = buf.indexOf('\r\n\r\n');
      if (end < 0) { assert.ok(buf.length < 128); break; }
      assert.ok(end < 128);
      const m = /^Content-Length: ([0-9]{1,5})$/i.exec(buf.toString('ascii', 0, end));
      assert.ok(m);
      const length = Number(m[1]);
      assert.ok(length > 0 && length <= 65536);
      if (buf.length < end + 4 + length) break;
      const msg = JSON.parse(buf.toString('utf8', end + 4, end + 4 + length));
      buf = buf.subarray(end + 4 + length);
      assert.ok(++count <= 96 && (msg.type === 'response' || msg.type === 'event'));
      if (msg.type === 'response' || ['initialized', 'stopped', 'exited', 'terminated'].includes(msg.event)) yield msg;
    }
  }
}

async function verify(work) {
  const env = { HOME: work, TMPDIR: work, XDG_CACHE_HOME: work, PATH: '/usr/bin:/bin',
    LLDB_DEBUGSERVER_PATH: `${llvm}/lldb-server`, PYTHONNOUSERSITE: '1' };
  const files = [];
  const lines = [];
  fs.mkdirSync(path.join(work, 'src'), { mode: 0o700 });
  for (const [rel, digest, marker] of inputs) {
    const file = path.join(project, rel);
    const text = checked(file, digest);
    const offset = text.indexOf(marker);
    assert.ok(offset >= 0 && offset === text.lastIndexOf(marker));
    files.push([file, text]);
    lines.push(text.slice(0, offset + marker.length).split('\n').length);
    fs.writeFileSync(path.join(work, rel), text);
  }
  fs.writeFileSync(path.join(work, 'main.rs'), `
#[path = "src/endpoint.rs"] mod endpoint;
#[path = "src/pre_cognition.rs"] mod pre_cognition;
fn main() {
    use pre_cognition::IntentPreview;
    assert_eq!(IntentPreview::from_partial("how long"), Some(IntentPreview::Question));
}
`);
  const guard = path.join(repo, 'engine/core-rs/crates/eon-cyber/fixtures/owned_dap/launch.c');
  fs.writeFileSync(path.join(work, 'guard.c'), checked(guard, '5381158e94b80aa431059c32dd963432c0f545c36b0519cc06e825fcd6ad2128'));
  for (const [tool, args] of [
    [`${llvm}/clang`, ['--no-default-config', '--ld-path=/usr/bin/ld', '-O2', 'guard.c', '-o', 'guard']],
    [path.join(os.homedir(), '.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc'),
      ['--edition=2024', '-g', '-C', 'opt-level=0', '-C', 'linker=/usr/bin/cc',
        '--remap-path-prefix', `${path.join(work, 'src')}=${path.join(project, 'src')}`, path.join(work, 'main.rs'), '-o', 'preview']],
  ]) {
    assert.equal(spawnSync(tool, args, { cwd: work, env, stdio: 'ignore', timeout: 30_000 }).status, 0);
  }

  const guardProcess = spawn(path.join(work, 'guard'), [], { cwd: work, env, stdio: ['pipe', 'pipe', 'ignore'] });
  const closed = once(guardProcess, 'close');
  guardProcess.stdin.on('error', () => {}); // An early close is detected on stdout.
  const stream = messages(guardProcess.stdout);
  const inbox = [];
  let seq = 0;
  function send(command, args) {
    const body = JSON.stringify({ seq: ++seq, type: 'request', command, arguments: args });
    guardProcess.stdin.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
    return seq;
  }
  async function take(predicate) {
    for (;;) {
      const i = inbox.findIndex(predicate);
      if (i >= 0) return inbox.splice(i, 1)[0];
      const next = await stream.next();
      assert.ok(!next.done);
      inbox.push(next.value);
    }
  }
  const event = name => take(m => m.type === 'event' && m.event === name);
  async function reply(id) {
    const m = await take(m => m.type === 'response' && m.request_seq === id);
    assert.ok(m.success);
    return m.body;
  }
  const call = (command, args) => reply(send(command, args));
  try {
    await call('initialize', { adapterID: 'lldb', pathFormat: 'path', linesStartAt1: true, columnsStartAt1: true,
      supportsRunInTerminalRequest: false, supportsStartDebuggingRequest: false });
    const launch = send('launch', { program: path.join(work, 'preview'), cwd: work, disableASLR: false });
    await event('initialized');
    const bp = (await call('setBreakpoints', { source: { path: files[0][0] }, breakpoints: [{ line: lines[0] }] })).breakpoints[0];
    assert.ok(bp.verified && bp.line === lines[0]);
    await call('configurationDone', {});
    await reply(launch);
    const { reason, threadId, hitBreakpointIds } = (await event('stopped')).body;
    assert.ok(reason === 'breakpoint' && hitBreakpointIds.includes(bp.id));
    // The editor normally fetches this frame to populate its Call Stack view.
    const top = (await call('stackTrace', { threadId, startFrame: 0, levels: 1 })).stackFrames[0];
    assert.ok(Number.isSafeInteger(top.id) && top.source.path === files[0][0] && top.line === lines[0]);

    // The product command receives only the already-paused editor API, not the setup/control peer.
    let requests = 0;
    const h = editor(project, files, async (command, args) => {
      assert.equal(command, 'stackTrace');
      assert.deepEqual(args, { threadId, startFrame: 0, levels: 12 });
      requests++;
      return call('stackTrace', args);
    }, threadId, top.id);
    const expected = inputs.map(([rel], i) => `${rel}:${lines[i]}`);
    h.pick = () => inputs.map(([rel]) => rel);
    await h.command();
    assert.deepEqual(h.messages, [expected.join('\n')]);
    h.pick = () => [inputs[1][0]];
    await h.command();
    assert.equal(h.messages[1], expected[1]);
    assert.equal(requests, 2);

    await call('continue', { threadId });
    assert.equal((await event('exited')).body.exitCode, 0);
    await event('terminated');
    await call('disconnect', { terminateDebuggee: false });
    return expected;
  } finally {
    guardProcess.kill('SIGTERM'); // Existing subreaper also cleans up a still-paused test on failure.
    assert.deepEqual(await closed, [0, null]);
  }
}

async function main() {
  assert.ok(process.platform === 'linux' && process.arch === 'x64');
  assert.deepEqual(process.argv.slice(2), ['--verify-owned-sources']);
  const work = workDirectory();
  try {
    const expected = await verify(work);
    console.log(`LLVM 18 stdio; both: ${expected.join(', ')}; caller only: ${expected[1]}; exit 0.`);
  } finally {
    fs.rmSync(work, { recursive: true, force: true });
  }
}
main().catch(err => {
  // Report the failed check's location, never the adapter's text or raw reply.
  const line = err.stack?.match(/native\.js:(\d+):\d+/)?.[1];
  console.error(`Native paused-project check failed${line ? ` at native.js:${line}` : ''}.`);
  process.exitCode = 1;
});
