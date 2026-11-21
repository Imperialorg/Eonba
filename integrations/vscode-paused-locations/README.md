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
