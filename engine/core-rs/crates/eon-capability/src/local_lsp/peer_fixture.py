# Trusted test-only stdio fixture. No project imports, subprocesses or discovery.
import json
import os
import sys
import time


def send(packet):
    b = json.dumps(dict(jsonrpc="2.0", **packet)).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(b)}\r\n\r\n".encode() + b)
    sys.stdout.buffer.flush()


def recv():
    length = sys.stdin.buffer.readline()
    assert length.startswith(b"Content-Length: ")
    assert sys.stdin.buffer.readline() == b"\r\n"
    return json.loads(sys.stdin.buffer.read(int(length.split(b":")[1])))


mode, sentinel = sys.argv[1:]
assert os.getcwd() == "/" and "HOME" not in os.environ and "PATH" not in os.environ
try:
    open(sentinel, "rb").close()
except PermissionError:
    pass
else:
    raise AssertionError("workspace was mounted")
init = recv()
assert init["method"] == "initialize"
assert init["params"]["rootUri"] is None and init["params"]["workspaceFolders"] is None
caps = {"diagnosticProvider": {}} if mode == "pull" else {}
if mode == "bad-capabilities":
    caps = []
if mode == "exit-before-ready":
    sys.exit(2)
if mode == "stall-before-ready":
    time.sleep(60)
send({"id": init["id"], "result": {"capabilities": caps}})
assert recv()["method"] == "initialized"
if mode == "blocked-stdin":
    time.sleep(60)

position = {"line": 0, "character": 0}
span = {"start": position, "end": position}
items = [{"message": "fixture message", "range": span}]
docs = []
while True:
    m = recv()
    method = m["method"]
    if method == "textDocument/didOpen":
        doc = m["params"]["textDocument"]
        assert doc["uri"].startswith("file:///eon-lsp-") and doc["version"] == 1
        assert doc["languageId"] == "python"
        docs.append(doc["uri"])
        if mode == "push":
            send({"method": "textDocument/publishDiagnostics", "params": {
                "uri": docs[-1], "version": 1, "diagnostics": items}})
    elif method == "textDocument/definition":
        uri = "file:///outside.py" if mode == "outside" else docs[-1]
        send({"id": m["id"], "result": {"uri": uri, "range": span}})
    elif method == "textDocument/references":
        assert m["params"]["context"]["includeDeclaration"]
        send({"id": m["id"], "result": [{"uri": u, "range": span} for u in docs]})
    elif method == "textDocument/diagnostic":
        send({"id": m["id"], "result": {"kind": "full", "items": items}})
    elif method == "textDocument/hover":
        if mode == "timeout":
            sys.stdout.buffer.write(b"Content-Length: 15\r\n\r\n{}")
            sys.stdout.buffer.flush()
            time.sleep(60)
        if mode == "flood":
            for n in range(50):
                send({"method": "window/logMessage", "params": {"message": "status"}})
        if mode == "mismatch":
            send({"id": m["id"] + 1, "result": None})
        # The client must refuse even a server with its own bidirectional request ID.
        send({"id": "test/edit", "method": "workspace/applyEdit", "params": {"edit": {}}})
        refusal = recv()
        assert refusal["id"] == "test/edit" and refusal["error"]["code"] == -32601
        send({"id": m["id"], "result": {"contents": {"kind": "plaintext", "value": "int"}}})
    elif method == "shutdown":
        assert "params" not in m
        send({"id": m["id"], "result": None})
    elif method == "exit":
        assert "params" not in m
        if mode == "ignore-exit":
            time.sleep(60)
        break
    else:
        raise AssertionError("unexpected client operation")
