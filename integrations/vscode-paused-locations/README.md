# EON: Show Paused Project Locations

A read-only command for **local Linux desktop VS Code 1.93+**. It answers “which lines of my open files are on this paused thread’s stack?” without taking control of the debugger. No build, npm install, Rust service, or extra adapter is required. Use your normal trusted debugger.

## Use it

1. Load the unpacked extension. From this checkout, for example:
   ```sh
   code --new-window --extensionDevelopmentPath=/home/runner/work/Eonba/Eonba/integrations/vscode-paused-locations /absolute/path/to/your/project
   ```
   The last path is the workspace to open in VS Code, **not** a program to run. To use it in an existing Linux installation, copy the extension directory into `~/.vscode/extensions/eon.eon-paused-locations-0.1.0` and reload the window.
2. Open and save the source tabs you want to inspect. In your own local debug session, stop at a normal source breakpoint and select a **frame** in **Run and Debug → Call Stack**. A selected thread alone is not proof of a pause.
3. Run **EON: Show Paused Project Locations** from the Command Palette. Check the files you own and press Enter. Nothing is selected in advance. Escape cancels without sending a request.
4. The dialog shows matching `relative/path:line` rows, innermost first. Missing, generated or unselected sources are omitted. It neither opens files nor resumes the program. Run the command again for another snapshot; consent is not remembered.

## What it can see

- One existing session in a trusted, non-remote workspace, scoped to that session’s own folder (also in multi-root windows). The command takes **no arguments** and returns no data to other commands. Activation only registers the command.
- At most eight ordinary, saved text tabs. The picker considers up to 256 open tabs/documents and 128 candidates. Supported source suffixes are listed in `sources.js` (including C/C++, Go, Java/Kotlin, JavaScript/TypeScript, Python and Rust). Display paths are bounded, plain ASCII; hidden components, traversal, URIs from adapters, ambiguous aliases and unsafe display characters are not accepted.
- Only the selected editor paths are opened. Every path component must be free of symlinks and group/other write access, owned by the operator or root. The workspace directory and each regular, singly linked source must be operator-owned. Sources must be UTF-8 (optional BOM), no NUL, at most 2 MiB and 100,000 lines. Saved editor text must match the file bytes. On an unexpected refusal, check ownership, permissions, path spelling and external changes; do not relax permissions to make the command work.
- Exactly one `stackTrace` with the focused frame’s thread ID, `startFrame: 0`, `levels: 12`. Three seconds to receive the reply; no polling, pagination, retry, cancellation request, or overlapping requests. Overlong frame lists are rejected; file paths are exact matches against the selection and lines must exist in the open document. Focus, workspace trust, document identity/version and file metadata are rechecked. Adapter-supplied paths are **never** opened.
- A matching row can show ` [conditional breakpoint configured]`. This reports **editor configuration only**, not the cause of the pause. It matches enabled, non-logpoint source breakpoints with a condition or hit count at that exact file and line. Above 256 editor breakpoints, no rows are annotated. Condition text is neither displayed nor sent to the adapter.
- No debug-session configuration, launch, attach, pause, stepping, breakpoint changes, evaluation, variables, heap or memory reads. No transport, listener, logs, telemetry, clipboard, source cache or persisted choices. The observer drops function names, values, frame IDs, instruction references, raw replies and adapter diagnostics from the display.

Use a local filesystem, a trusted editor/adapter and an operator-controlled project. The command does not authenticate the debuggee, inspect the adapter’s transport, or prove the paused binary was built from the current source. Source maps that do not resolve to exact selected local paths produce no rows. A compromised editor, same-user process or kernel is outside its trust model. The VS Code host owns DAP message decoding; the extension bounds its request, processing and output, not the host’s input buffer. The existing Rust debugger’s typed heap counts remain limited to its fixed fixtures.

## Reproduce the checks

From the repository checkout (absolute paths shown for this runner):

```sh
node --test /home/runner/work/Eonba/Eonba/scripts/eval/paused_locations/command.test.js
node /home/runner/work/Eonba/Eonba/scripts/eval/paused_locations/native.js --verify-owned-sources
```

The second check needs Linux x86-64 child tracing and seccomp, system LLVM 18 in `/usr/lib/llvm-18/bin`, `/usr/bin/cc`, and a stable Rust toolchain supporting edition 2024 and `str::floor_char_boundary` at `~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu`. Neither check downloads anything. Tests create private scratch directories under the checkout’s `target/` and remove them.

The explicit native test compiles **verified snapshots of two existing production `eon-voice` files**, with debug paths mapped to the originals, and a fixed driver that classifies the words “how long”. There is no test-program selector. The existing no-network subreaper confines `lldb-dap` and the test child. The harness establishes and verifies a real breakpoint stop over stdio before invoking the same command registered by the extension. The VS Code tab/selection/UI API is simulated in this check; the adapter, stopped program, stack, files, and source-line oracle are real.

On LLVM 18.1.3 the two-file choice produced `src/endpoint.rs:28` and `src/pre_cognition.rs:22`. Repeating on the same stop with only the caller selected produced just `src/pre_cognition.rs:22`. The driver exited 0. Neither this check nor the command adds a general project launcher or extends heap inspection.

### Disposable desktop check (Ubuntu 24.04 x86-64)

The separate desktop harness uses Microsoft VS Code **1.93.1**, the stock LLVM extension **0.7.20260710** in stdio mode, and the same two digest-checked Rust files. It requires working rootless `podman unshare`, nested user/mount/PID/IPC/network namespaces, `unzip`, and a headless Sway compositor, Grim, wlrctl and Tesseract with English data. The Linux desktop and its local IPC, source copies, profile and extension directories are isolated from the real user. Electron's nested sandbox is disabled **inside** this unprivileged, offline filesystem/PID namespace. There is no remote debugging port or test adapter.

Before running, provision the editor and stock VSIX into the ignored tools directory. These downloads are separate from the offline test:

```sh
set -eu
T=/home/runner/work/Eonba/Eonba/target/paused-ui-tools
mkdir -p "$T/unpacked"
cd "$T"
curl -fL https://update.code.visualstudio.com/1.93.1/linux-x64/stable -o code.tar.gz
echo '4a8598a8016630229f26f243b653bc14b0cc28795038f55ea224543f149fe510  code.tar.gz' | sha256sum -c -
tar -xzf code.tar.gz
curl -fL https://open-vsx.org/api/llvm-vs-code-extensions/lldb-dap/0.7.20260710/file/llvm-vs-code-extensions.lldb-dap-0.7.20260710.vsix -o lldb.vsix
echo 'cd6bec2f77813c6bb010002044c6b231d1ec3615d2584c6fddd09988ade8b863  lldb.vsix' | sha256sum -c -
```

Provide Ubuntu packages `sway`, `grim`, `wlrctl`, `tesseract-ocr` and `tesseract-ocr-eng` and their libraries. The test sees system `/usr` read-only and also searches `$T/unpacked/usr` for locally extracted packages. Provision `eng.traineddata` at `$T/unpacked/usr/share/tesseract-ocr/5/tessdata/eng.traineddata` (copy it from `/usr/share/tesseract-ocr/5/tessdata/` if installed system-wide). Then run:

```sh
cd /home/runner/work/Eonba/Eonba
node /home/runner/work/Eonba/Eonba/scripts/eval/paused_locations/desktop.js --verify-disposable-desktop
```

The test driver—not the product—opens a fixed workspace, sets a first-hit source breakpoint, starts that one test, and checks `activeStackItem instanceof vscode.DebugStackFrame` for the active session. It drives stock Command Palette and Quick Pick actions. An independent process checks pixels on the private display. Only a zero exit from the desktop command is a pass for both rendered result dialogs.

**Observed on this runner: partial desktop verification.** The active session had a real `DebugStackFrame`, and the display check saw the registered command and the real two-file picker. After the workbench selection and accept actions, the visual check timed out waiting for `src/pre_cognition.rs:22` in the first result dialog. This does not distinguish a selection, rendering or OCR failure. Neither the rendered conditional annotation nor the second, caller-only desktop dialog is verified. The native stdio result above and the editor-model annotation tests are separate, passing checks.
