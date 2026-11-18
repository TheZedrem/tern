"""Read-only Docker inventory for Tern's active terminal."""

import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import threading


def local_endpoint():
    if os.name == "nt":
        return "npipe:////./pipe/docker_engine"
    configured = os.environ.get("DOCKER_HOST", "")
    if configured.startswith("unix:///"):
        return configured
    runtime = os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")
    socket = Path(runtime, "docker.sock")
    return "unix://" + str(socket) if socket.is_socket() else "unix:///var/run/docker.sock"


def docker(target, args):
    env = os.environ.copy()
    env.pop("DOCKER_CONTEXT", None)
    env.pop("DOCKER_HOST", None)
    if target["kind"] == "local":
        argv = ["docker", "--host", local_endpoint(), *args]
    else:
        # Only Tern supplies the SSH destination. The remote shell receives
        # fixed commands; no container name or profile value enters its script.
        remote = (
            'unset DOCKER_CONTEXT; case "$DOCKER_HOST" in unix:///*) ;; *) '
            'sock="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/docker.sock"; '
            'if [ -S "$sock" ]; then DOCKER_HOST="unix://$sock"; '
            'else DOCKER_HOST=unix:///var/run/docker.sock; fi;; esac; '
            'export DOCKER_HOST; exec docker ' + shlex.join(args)
        )
        argv = ["ssh", "-T", "-oBatchMode=yes", "-oStrictHostKeyChecking=yes",
                "-oClearAllForwardings=yes", "-oForwardAgent=no", "-oConnectTimeout=5",
                *target["args"], "sh -c " + shlex.quote(remote)]
    try:
        result = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                                timeout=12, encoding="utf-8", errors="replace", env=env,
                                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(f"{argv[0]} unavailable or timed out") from error
    if result.returncode:
        raise RuntimeError(result.stderr.strip()[:500] or f"{argv[0]} exited {result.returncode}")
    return result.stdout


def text(value):
    return {"type": "text", "text": value}


def inventory(target):
    items = [text("Running containers")]
    try:
        output = docker(target, ["ps", "--format", "json"])
        containers = [json.loads(line) for line in output.splitlines() if line.strip()]
        for entry in containers[:200]:
            items.append({"type": "row", "text": entry["Names"][:200],
                          "detail": f'{entry["Image"]} · {entry["Status"]}'[:500]})
        if not containers:
            items.append(text("No running containers."))
        elif len(containers) > 200:
            items.append(text(f"Showing 200 of {len(containers)} containers."))
    except (RuntimeError, ValueError, KeyError, TypeError) as error:
        return [text(f"Docker unavailable: {error}")]
    items.append(text("Compose projects"))
    try:
        projects = json.loads(docker(target, ["compose", "ls", "--all", "--format", "json"]))
        for entry in projects[:200]:
            files = entry.get("ConfigFiles") or ""
            if isinstance(files, list):
                files = ", ".join(files)
            items.append({"type": "row", "text": entry["Name"][:200],
                          "detail": " · ".join(filter(None, [entry["Status"], files]))[:500]})
        if not projects:
            items.append(text("No Compose projects."))
        elif len(projects) > 200:
            items.append(text(f"Showing 200 of {len(projects)} projects."))
    except (RuntimeError, ValueError, KeyError, TypeError, AttributeError) as error:
        items.append(text(f"Compose unavailable: {error}"))
    return items


def serve(source, sink):
    changed = threading.Condition()
    generation = 0
    revision = None
    target = None
    closed = False

    def send(action):
        sink.write(json.dumps(action, ensure_ascii=False) + "\n")
        sink.flush()

    def show(items):
        if target:
            title = target.get("host") or ("Docker Desktop" if os.name == "nt" else "Local Docker")
            items = [text(title), {"type": "button", "id": "refresh", "label": "Refresh"}, *items]
        send({"action": "set_panel", "panel": "inventory", "revision": revision, "items": items})

    def worker():
        seen = 0
        while True:
            with changed:
                changed.wait_for(lambda: closed or generation != seen)
                if closed:
                    return
                seen, query = generation, target
            if query:
                rows = inventory(query)
                with changed:
                    if not closed and generation == seen:
                        show(rows)

    threading.Thread(target=worker, daemon=True).start()
    try:
        for line in source:
            event = json.loads(line)
            name = event["event"]
            with changed:
                if name in ("panel_opened", "panel_closed", "panel_target"):
                    target = event.get("target") if name == "panel_target" else None
                    revision = event.get("revision")
                    generation += 1
                    if name == "panel_target":
                        show([text("Loading…" if target else "Select a terminal and grant pane_info in Plugins settings.")])
                    changed.notify()
                elif name == "panel_action" and event.get("id") == "refresh" and target:
                    generation += 1
                    show([text("Loading…")])
                    changed.notify()
                send({"action": "ready"})
    finally:
        with changed:
            closed = True
            changed.notify()


if __name__ == "__main__":
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8")
    serve(sys.stdin, sys.stdout)
