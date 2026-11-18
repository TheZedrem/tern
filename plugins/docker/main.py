"""Docker inventory and actions for Tern's active terminal."""

import json
import os
from pathlib import Path
import re
import unicodedata
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


def docker_argv(target, args, interactive=False):
    if target["kind"] == "local":
        return ["docker", "--host", local_endpoint(), *args]
    else:
        # Only Tern supplies the SSH destination. The remote shell receives
        # individually quoted arguments, including the chosen executable.
        remote = (
            'unset DOCKER_CONTEXT; case "$DOCKER_HOST" in unix:///*) ;; *) '
            'sock="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/docker.sock"; '
            'if [ -S "$sock" ]; then DOCKER_HOST="unix://$sock"; '
            'else DOCKER_HOST=unix:///var/run/docker.sock; fi;; esac; '
            'export DOCKER_HOST; exec docker ' + shlex.join(args)
        )
        return ["ssh", "-tt" if interactive else "-T", "-oBatchMode=yes", "-oStrictHostKeyChecking=yes",
                "-oClearAllForwardings=yes", "-oForwardAgent=no", "-oConnectTimeout=5",
                *target["args"], "sh -c " + shlex.quote(remote)]


def docker(target, args):
    env = os.environ.copy()
    env.pop("DOCKER_CONTEXT", None)
    env.pop("DOCKER_HOST", None)
    argv = docker_argv(target, args)
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
        output = docker(target, ["ps", "--no-trunc", "--format", "{{json .}}"])
        containers = [json.loads(line) for line in output.splitlines() if line.strip()]
        for entry in containers[:200]:
            items.append({"type": "row", "_container": entry["ID"], "text": entry["Names"][:200],
                          "detail": f'{entry["Image"]} · {entry["Status"]}'[:500]})
        if not containers:
            items.append(text("No running containers."))
        elif len(containers) > 200:
            items.append(text(f"Showing 200 of {len(containers)} containers."))
    except RuntimeError as error:
        return [text(f"Docker unavailable: {error}")]
    except (ValueError, KeyError, TypeError):
        return [text("Could not read the container list returned by docker ps.")]
    items.append(text("Compose projects"))
    try:
        projects = json.loads(docker(target, ["compose", "ls", "--all", "--format", "json"]))
        for entry in projects[:200]:
            files = entry.get("ConfigFiles") or ""
            # Compose v2 joins absolute paths with commas, without escaping
            # a comma inside an individual filename.
            paths = files if isinstance(files, list) else re.split(r",(?=/|[A-Za-z]:[\\/]|\\\\)", files) if files else []
            files = ", ".join(paths)
            items.append({"type": "row", "_paths": paths, "text": entry["Name"][:200],
                          "detail": " · ".join(filter(None, [entry["Status"], files]))[:500]})
        if not projects:
            items.append(text("No Compose projects."))
        elif len(projects) > 200:
            items.append(text(f"Showing 200 of {len(projects)} projects."))
    except (RuntimeError, ValueError, KeyError, TypeError, AttributeError) as error:
        items.append(text(f"Compose unavailable: {error}"))
    return items


def safe_argument(value):
    return (isinstance(value, str) and 0 < len(value.encode()) <= 2048
            and all(unicodedata.category(char) not in ("Cc", "Cf") for char in value))


ACTION_LABELS = {"shell": "Shell", "logs": "Follow logs", "json": "Inspect full JSON"}


def details(target, row, revision, shell, action, cache):
    name = row["text"]
    if "_paths" in row:
        items = [text(name), text("Compose files in merge order")]
        for index, path in enumerate(row["_paths"]):
            if not safe_argument(path):
                items.append(text("This file path cannot be opened safely."))
                continue
            items.append({"type": "button", "id": f"edit-{index}", "label": f"Edit file {index + 1}",
                          "command": {"action": "edit_file", "revision": revision, "path": path}})
        return items if row["_paths"] else [*items, text("Compose reported no source files.")]
    ident = row["_container"]
    if not isinstance(ident, str) or not re.fullmatch(r"[a-fA-F0-9]{1,64}", ident):
        return [text("Docker returned an invalid container ID.")]
    try:
        if ident not in cache:
            data, = json.loads(docker(target, ["inspect", "--type", "container", ident]))
            # Keep only fields the summary needs between clicks.
            cache[ident] = {key: data.get(key) for key in ("State", "Platform", "NetworkSettings", "Mounts")}
            cache[ident]["Config"] = {"Image": data["Config"]["Image"]}
        data = cache[ident]
        state = data["State"]
        network = data.get("NetworkSettings") or {}
        ports = [f"{binding['HostIp']}:{binding['HostPort']} -> {port}"
                 for port, bindings in (network.get("Ports") or {}).items()
                 for binding in bindings or []]
        mounts = [f"{m['Source']} -> {m['Destination']}" for m in data.get("Mounts") or []]
        fields = [("Image", data["Config"]["Image"]), ("State", state["Status"]),
                  ("Health", (state.get("Health") or {}).get("Status")),
                  ("Published ports", ", ".join(ports) or "None"),
                  ("Networks", ", ".join(network.get("Networks") or {}) or "None"),
                  ("Mounts", ", ".join(mounts) or "None")]
        summary = [{"type": "row", "text": key, "detail": value[:2000]} for key, value in fields if value]
    except (RuntimeError, ValueError, TypeError, AttributeError, KeyError) as error:
        return [text(name), text(f"Could not inspect this container: {error}")]

    executable = shell or ("cmd.exe" if data.get("Platform") == "windows" else "/bin/sh")

    if action is None:
        buttons = [{"type": "button", "id": key, "label": label} for key, label in ACTION_LABELS.items()]
        return [text(name), *buttons, *summary]

    args = {"shell": ["exec", "-it", ident, executable],
            "logs": ["logs", "--tail", "100", "--follow", ident],
            "json": ["inspect", "--type", "container", ident]}[action]
    argv = docker_argv(target, args, interactive=action == "shell")
    label = ACTION_LABELS[action]
    title = f"{label} - {name[:48]}"
    if not safe_argument(title) or any(not safe_argument(arg) for arg in argv) or sum(len(arg.encode()) for arg in argv) > 4096:
        return [text(f"{label} unavailable for this target.")]
    items = [text(name)]
    if action == "shell":
        items.append({"type": "input", "placeholder": "Alternate shell executable (press Enter)"})
    elif action == "json":
        items.append(text("Full inspection includes environment variables."))
    items.append({"type": "button", "id": "confirm", "label": label, "command": {
        "action": "open_split", "title": title, "revision": revision, "argv": argv}})
    return items


def serve(source, sink):
    changed = threading.Condition()
    generation = 0
    revision = None
    target = None
    closed = False
    selected = None
    shell = None
    choices = {}
    action_view = None
    refresh_count = 0

    def send(action):
        sink.write(json.dumps(action) + "\n")
        sink.flush()

    def show(items):
        if target:
            title = target.get("host") or ("Docker Desktop" if os.name == "nt" else "Local Docker")
            buttons = [{"id": "refresh", "label": "Refresh"}]
            if selected is not None:
                buttons.insert(0, {"id": "back", "label": "Back"})
            items = [{"type": "row", "text": title,
                      "buttons": buttons}, *items]
        send({"action": "set_panel", "panel": "inventory", "revision": revision, "items": items})

    def worker():
        seen = 0
        cache_key = None
        cache = {}
        while True:
            with changed:
                changed.wait_for(lambda: closed or generation != seen)
                if closed:
                    return
                seen, query, row, rev, exe = generation, target, selected, revision, shell
                action = action_view
                key = (rev, refresh_count, (row or {}).get("_container"))
                if key != cache_key:
                    cache_key, cache = key, {}
            if query:
                rows = inventory(query) if row is None else details(query, row, rev, exe, action, cache)
                with changed:
                    if not closed and generation == seen:
                        choices.clear()
                        if row is None:
                            for index, item in enumerate(rows):
                                if "_container" in item or "_paths" in item:
                                    item["id"] = f"select-{seen}-{index}"
                                    choices[item["id"]] = item
                        show([{key: value for key, value in item.items() if not key.startswith("_")}
                              for item in rows])

    threading.Thread(target=worker, daemon=True).start()
    try:
        for line in source:
            event = json.loads(line)
            name = event["event"]
            with changed:
                if name in ("panel_opened", "panel_closed", "panel_target"):
                    selected = shell = action_view = None
                    choices.clear()
                    target = event.get("target") if name == "panel_target" else None
                    revision = event.get("revision")
                    generation += 1
                    if name == "panel_target":
                        show([text("Loading…" if target else "Select a terminal and enable “Read pane and SSH connection details” in Plugins settings.")])
                    changed.notify()
                elif target and name in ("panel_action", "panel_input"):
                    action = event.get("id")
                    if action in choices:
                        selected, shell = choices[action], None
                    elif action in ACTION_LABELS and selected and "_container" in selected:
                        action_view = action
                    elif action == "back":
                        if action_view:
                            action_view = None
                        else:
                            selected = shell = None
                    elif name == "panel_input" and action_view == "shell":
                        value = event.get("text", "")
                        if safe_argument(value):
                            shell = value
                        else:
                            send({"action": "ready"})
                            continue
                    elif action == "refresh":
                        refresh_count += 1
                    else:
                        send({"action": "ready"})
                        continue
                    choices.clear()
                    generation += 1
                    show([text("Loading...")])
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
