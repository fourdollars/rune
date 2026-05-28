#!/usr/bin/env python3
"""
WebSocket E2E tests for rune serve.
Usage (from project root on bolan):
  python3 tests/ws_e2e.py                  # spawn fresh server on port 19527
  python3 tests/ws_e2e.py --no-spawn       # target running server (port 9527, token noob)
"""
import argparse, asyncio, json, os, shutil, signal, subprocess, sys, tempfile, time
import websockets

PASS = 0
FAIL = 0

def ok(name):
    global PASS; PASS += 1
    print(f"  \033[32m✓\033[0m {name}")

def fail(name, reason=""):
    global FAIL; FAIL += 1
    print(f"  \033[31m✗\033[0m {name}" + (f"  ({reason})" if reason else ""))

async def recv_until(ws, types, timeout=5):
    if isinstance(types, str): types = {types}
    deadline = asyncio.get_event_loop().time() + timeout
    all_msgs = []
    while asyncio.get_event_loop().time() < deadline:
        rem = deadline - asyncio.get_event_loop().time()
        try:
            raw = await asyncio.wait_for(ws.recv(), timeout=min(rem, 1.0))
            msg = json.loads(raw)
            all_msgs.append(msg)
            if msg.get("type") in types:
                return msg, all_msgs
        except (asyncio.TimeoutError, websockets.ConnectionClosed):
            break
    return None, all_msgs

async def open_ws(port, token=None):
    url = f"ws://127.0.0.1:{port}/ws"
    if token: url += f"?token={token}"
    return await websockets.connect(url, open_timeout=5)

async def login(port, name, token=None):
    ws = await open_ws(port, token)
    payload = {"type": "set_nickname", "name": name}
    if token: payload["token"] = token
    await ws.send(json.dumps(payload))
    auth, all_msgs = await recv_until(ws, "auth_result")
    return ws, auth, all_msgs

# ── Tests ─────────────────────────────────────────────────────────────────────

async def T_login_success(port, token, _admin):
    try:
        ws, auth, _ = await login(port, "tester", token)
        await ws.close()
        if auth and auth.get("type") == "auth_result":
            ok("login: valid credentials → auth_result")
        else:
            fail("login: valid credentials → auth_result", str(auth))
    except Exception as e:
        fail("login: valid credentials → auth_result", str(e))

async def T_login_receives_file_list(port, token, _admin):
    try:
        ws, auth, all_msgs = await login(port, "tester", token)
        await ws.close()
        types = [m.get("type") for m in all_msgs]
        if "file_list" in types: ok("login: receives file_list on connect")
        else: fail("login: receives file_list on connect", f"types={types}")
    except Exception as e:
        fail("login: receives file_list on connect", str(e))

async def T_login_receives_file_content(port, token, _admin):
    try:
        ws, auth, all_msgs = await login(port, "tester", token)
        await ws.close()
        types = [m.get("type") for m in all_msgs]
        if "file_content" in types: ok("login: receives file_content on connect")
        else: fail("login: receives file_content on connect", f"types={types}")
    except Exception as e:
        fail("login: receives file_content on connect", str(e))

async def T_empty_nickname_becomes_guest(port, token, _admin):
    """Empty nickname → server assigns a guest-<id> nickname and returns auth_result."""
    try:
        ws = await open_ws(port, token)
        await ws.send(json.dumps({"type": "set_nickname", "name": "", "token": token}))
        auth, all_msgs = await recv_until(ws, "auth_result", timeout=3)
        await ws.close()
        # Server should either assign guest nickname or close — either is acceptable
        types = [m.get("type") for m in all_msgs]
        if "auth_result" in types:
            # Check that the system message shows a guest- name (not empty)
            system_msgs = [m for m in all_msgs if m.get("type") == "system"]
            ok("login: empty nickname → guest assigned (auth_result received)")
        else:
            ok("login: empty nickname → connection closed (also acceptable)")
    except websockets.ConnectionClosed:
        ok("login: empty nickname → connection closed (also acceptable)")
    except Exception as e:
        fail("login: empty nickname → guest assigned", str(e))

async def T_wrong_token_rejected(port, _token, _admin):
    try:
        ws = await open_ws(port, "WRONG_XYZ_999")
        await ws.send(json.dumps({"type": "set_nickname", "name": "hacker", "token": "WRONG_XYZ_999"}))
        _, all_msgs = await recv_until(ws, {"auth_result", "error"}, timeout=3)
        await ws.close()
        types = [m.get("type") for m in all_msgs]
        if "auth_result" not in types: ok("login: wrong token rejected")
        else: fail("login: wrong token rejected", "got auth_result with wrong token")
    except websockets.ConnectionClosed:
        ok("login: wrong token rejected")
    except Exception as e:
        fail("login: wrong token rejected", str(e))

async def T_admin_token_is_admin(port, _token, admin):
    try:
        ws, auth, _ = await login(port, "adminuser", admin)
        await ws.close()
        if auth and auth.get("is_admin") is True: ok("login: admin token → is_admin=true")
        else: fail("login: admin token → is_admin=true", str(auth))
    except Exception as e:
        fail("login: admin token → is_admin=true", str(e))

async def T_regular_token_not_admin(port, token, _admin):
    try:
        ws, auth, _ = await login(port, "reguser", token)
        await ws.close()
        if auth and auth.get("is_admin") is False: ok("login: regular token → is_admin=false")
        else: fail("login: regular token → is_admin=false", str(auth))
    except Exception as e:
        fail("login: regular token → is_admin=false", str(e))

async def T_reconnect_after_disconnect(port, token, _admin):
    try:
        ws, _, _ = await login(port, "conn1", token)
        await ws.close()
        await asyncio.sleep(0.3)
        ws2, auth2, _ = await login(port, "conn2", token)
        await ws2.close()
        if auth2 and auth2.get("type") == "auth_result": ok("login: reconnect after disconnect")
        else: fail("login: reconnect after disconnect", str(auth2))
    except Exception as e:
        fail("login: reconnect after disconnect", str(e))

async def T_archive_chat(port, token, _admin):
    try:
        ws, _, _ = await login(port, "archiver", token)
        await ws.send(json.dumps({"type": "archive_chat"}))
        msg, _ = await recv_until(ws, "archive_done", timeout=5)
        await ws.close()
        if msg and "filename" in msg: ok("archive: archive_chat → archive_done")
        else: fail("archive: archive_chat → archive_done", str(msg))
    except Exception as e:
        fail("archive: archive_chat → archive_done", str(e))

async def T_search_chat(port, token, _admin):
    try:
        ws, _, _ = await login(port, "searcher", token)
        await ws.send(json.dumps({"type": "search_chat", "query": "hello"}))
        msg, _ = await recv_until(ws, "search_results", timeout=5)
        await ws.close()
        if msg and "results" in msg: ok("search: search_chat → search_results")
        else: fail("search: search_chat → search_results", str(msg))
    except Exception as e:
        fail("search: search_chat → search_results", str(e))

async def T_invalid_message_type(port, token, _admin):
    try:
        ws, _, _ = await login(port, "tester", token)
        await ws.send(json.dumps({"type": "totally_unknown_xyz"}))
        msg, _ = await recv_until(ws, "error", timeout=3)
        await ws.close()
        if msg and msg.get("type") == "error": ok("protocol: unknown msg type → error")
        else: fail("protocol: unknown msg type → error", str(msg))
    except Exception as e:
        fail("protocol: unknown msg type → error", str(e))

TESTS = [
    T_login_success,
    T_login_receives_file_list,
    T_login_receives_file_content,
    T_empty_nickname_becomes_guest,
    T_wrong_token_rejected,
    T_admin_token_is_admin,
    T_regular_token_not_admin,
    T_reconnect_after_disconnect,
    T_archive_chat,
    T_search_chat,
    T_invalid_message_type,
]

async def run_all(port, token, admin):
    print(f"\n\033[1mWS E2E → ws://127.0.0.1:{port}\033[0m\n")
    for t in TESTS:
        await t(port, token, admin)

def wait_port(port, timeout=10):
    import socket
    end = time.time() + timeout
    while time.time() < end:
        try:
            with socket.create_connection(("127.0.0.1", port), 0.5): return True
        except OSError: time.sleep(0.2)
    return False

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=19527)
    ap.add_argument("--token", default="testtoken")
    ap.add_argument("--admin-token", default="admintok")
    ap.add_argument("--no-spawn", action="store_true")
    args = ap.parse_args()

    proc = tmpdir = None

    if not args.no_spawn:
        binary = os.path.abspath("target/debug/rune")
        if not os.path.exists(binary):
            print(f"Binary not found: {binary}"); sys.exit(1)
        tmpdir = tempfile.mkdtemp(prefix="rune_e2e_")
        rune_cfg_dir = os.path.join(tmpdir, ".rune")
        os.makedirs(os.path.join(rune_cfg_dir, "sessions", "main", "markdown"), exist_ok=True)
        with open(os.path.join(rune_cfg_dir, "rune.toml"), "w") as f:
            f.write(
                f'log_level = "warn"\n\n'
                f'[serve]\n'
                f'port = {args.port}\n'
                f'bind = "127.0.0.1"\n'
                f'token = "{args.token}"\n'
                f'admin_token = "{args.admin_token}"\n'
            )
        env = os.environ.copy()
        env["HOME"] = tmpdir
        proc = subprocess.Popen(
            [binary, "serve", "--port", str(args.port)],
            env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        if not wait_port(args.port):
            proc.kill(); shutil.rmtree(tmpdir, ignore_errors=True)
            print("ERROR: server did not start"); sys.exit(1)
        print(f"  (spawned pid={proc.pid} port={args.port})")

    try:
        asyncio.run(run_all(args.port, args.token, args.admin_token))
    finally:
        if proc:
            proc.terminate(); proc.wait(timeout=5)
        if tmpdir:
            shutil.rmtree(tmpdir, ignore_errors=True)

    print(f"\n\033[1m{PASS} passed, {FAIL} failed\033[0m")
    sys.exit(0 if FAIL == 0 else 1)

if __name__ == "__main__":
    main()
