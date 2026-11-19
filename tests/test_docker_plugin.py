import importlib.util
import io
import json
import shlex
import subprocess
import threading
import unittest
from contextlib import contextmanager
from pathlib import Path
from queue import Empty, Queue
from time import monotonic
from unittest.mock import patch


ENTRY = Path(__file__).resolve().parents[1] / "plugins" / "docker" / "main.py"
SPEC = importlib.util.spec_from_file_location("tern_docker", ENTRY)
plugin = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(plugin)

LOCAL = {"kind": "local"}
SSH = {"kind": "ssh", "host": "ada@work", "args": ["-l", "ada", "work"]}


class DockerPanelTests(unittest.TestCase):
    def test_inventory_queries_running_containers_and_all_compose_projects(self):
        container = {"ID": "123", "Names": "api-1", "Image": "app:v2", "Status": "Up 2 hours"}
        project = {"Name": "retired-project", "Status": "exited(1)", "ConfigFiles": "/srv/a.yml"}
        for target in [LOCAL, SSH]:
            with self.subTest(target=target):
                calls = []

                def run(argv, **kwargs):
                    self.assertFalse(kwargs.get("shell"))
                    self.assertEqual(argv[0], "ssh" if target == SSH else "docker")
                    if target == SSH:
                        options = " ".join(argv).lower()
                        self.assertIn("batchmode=yes", options)
                        self.assertIn("stricthostkeychecking=yes", options)
                        start = argv.index("-l")
                        self.assertEqual(list(argv[start:start + 3]), SSH["args"])
                        shell, flag, script = shlex.split(argv[-1])
                        self.assertEqual([shell, flag], ["sh", "-c"])
                        command = shlex.split(script.rsplit("exec docker ", 1)[1])
                    else:
                        command = argv
                    if "compose" in command:
                        self.assertIn("--all", command)
                        calls.append("projects")
                        output = json.dumps([project])
                    else:
                        self.assertIn("ps", command)
                        self.assertEqual(command[command.index("--format") + 1], "{{json .}}")
                        self.assertFalse({"-a", "--all"} & set(command))
                        calls.append("containers")
                        output = json.dumps(container)
                    return subprocess.CompletedProcess(argv, 0, stdout=output, stderr="")

                with patch("subprocess.run", side_effect=run):
                    panel = json.dumps(plugin.inventory(target))
                self.assertCountEqual(calls, ["projects", "containers"])
                for value in ["api-1", "app:v2", "Up 2 hours", "retired-project",
                              "exited(1)", "/srv/a.yml"]:
                    self.assertIn(value, panel)

    def test_compose_failure_preserves_the_container_list_and_reports_the_error(self):
        def query(target, args):
            if "compose" in args:
                raise RuntimeError("compose unavailable")
            return json.dumps({"ID": "abc", "Names": "still-visible", "Image": "x", "Status": "Up"})

        with patch.object(plugin, "docker", side_effect=query):
            panel = json.dumps(plugin.inventory(LOCAL))
        self.assertIn("still-visible", panel)
        self.assertIn("compose unavailable", panel)

    def test_empty_inventory_is_distinct_from_an_unreachable_engine(self):
        with patch.object(plugin, "docker", side_effect=["", "[]"]):
            empty = json.dumps(plugin.inventory(LOCAL)).lower()
        with patch.object(plugin, "docker", side_effect=RuntimeError("daemon offline")):
            failed = json.dumps(plugin.inventory(LOCAL)).lower()
        self.assertIn("no running containers", empty)
        self.assertIn("no compose projects", empty)
        self.assertIn("daemon offline", failed)
        self.assertNotIn("no running containers", failed)

    def test_malformed_container_list_reports_the_query_without_echoing_output(self):
        output = "private-remote-output"
        with patch.object(plugin, "docker", return_value=output), \
             patch("sys.stderr", new_callable=io.StringIO) as stderr, \
             patch("sys.stdout", new_callable=io.StringIO) as stdout:
            panel = json.dumps(plugin.inventory(SSH)).lower()
        self.assertIn("container list", panel)
        self.assertNotIn("docker unavailable", panel)
        self.assertNotIn(output, panel + stderr.getvalue() + stdout.getvalue())

    def test_switching_targets_drops_the_inflight_result(self):
        local_started = threading.Event()
        release_local = threading.Event()
        remote_drawn = threading.Event()

        class Sink(io.StringIO):
            def write(self, text):
                result = super().write(text)
                if "current-remote" in text:
                    remote_drawn.set()
                return result

        def inventory(target):
            if target == LOCAL:
                local_started.set()
                release_local.wait(5)
                return [{"type": "text", "text": "obsolete-local"}]
            return [{"type": "text", "text": "current-remote"}]

        def event(name, **fields):
            return json.dumps({"api": 1, "event": name, "panel": "inventory", **fields}) + "\n"

        def source():
            yield event("panel_opened")
            yield event("panel_target", target=LOCAL)
            self.assertTrue(local_started.wait(5), "local query never started")
            yield event("panel_target", target=SSH)
            release_local.set()
            self.assertTrue(remote_drawn.wait(5), "remote inventory never appeared")

        sink = Sink()
        with patch.object(plugin, "inventory", side_effect=inventory):
            plugin.serve(source(), sink)
        actions = [json.loads(line) for line in sink.getvalue().splitlines()]
        self.assertFalse(any("obsolete-local" in json.dumps(action) for action in actions))
        self.assertTrue(any(action.get("panel") == "inventory" and "current-remote" in
                            json.dumps(action.get("items")) for action in actions))


@contextmanager
def open_panel(target, docker):
    events, frames = Queue(), Queue()

    class Sink(io.StringIO):
        def write(self, line):
            message = json.loads(line)
            if message["action"] == "set_panel":
                frames.put(message)
            return len(line)

    def send(name, **fields):
        events.put(json.dumps({"api": 1, "panel": "inventory", "event": name, **fields}))

    def view(containing):
        deadline = monotonic() + 5
        last = []
        while monotonic() < deadline:
            try:
                frame = frames.get(timeout=max(0, deadline - monotonic()))
            except Empty:
                break
            last = frame["items"]
            if containing.casefold() in json.dumps(last).casefold():
                return frame
        raise AssertionError(f"Panel never showed {containing!r}. Last items: {last}")

    with patch.object(plugin, "docker", side_effect=docker):
        worker = threading.Thread(target=plugin.serve, args=(iter(events.get, None), Sink()))
        worker.start()
        try:
            send("panel_opened")
            send("panel_target", target=target, revision=41)
            yield send, view
        finally:
            events.put(None)
            worker.join(5)


class DockerActionTests(unittest.TestCase):
    container_id = "cafebabe" * 8
    paths = ["/srv/web stack/base.yml", "/srv/web stack/override.yml"]

    def docker(self, target, args):
        if "ps" in args:
            return json.dumps({"ID": self.container_id, "Names": "api-one",
                               "Image": "api:release", "Status": "Up"})
        if "ls" in args:
            return json.dumps([{"Name": "work-stack", "Status": "running(1)",
                                "ConfigFiles": ",".join(self.paths)}])
        if "inspect" in args:
            self.assertIn(self.container_id, args)
            return json.dumps([{"Id": self.container_id, "Name": "/api-one",
                                "Platform": "linux",
                                "Config": {"Image": "api:release", "Env": ["TOKEN=only-in-full-json"]},
                                "State": {"Status": "running", "Health": {"Status": "healthy"}},
                                "NetworkSettings": {"Ports": {"80/tcp": [{"HostIp": "127.0.0.1",
                                                                        "HostPort": "18080"}]},
                                                    "Networks": {"work-bridge": {}}},
                                "Mounts": [{"Type": "bind", "Source": "/srv/assets",
                                            "Destination": "/var/assets"}]}])
        self.fail(f"Unexpected Docker call {args}")

    def select(self, send, frame, text):
        row = next(item for item in frame["items"] if item.get("text") == text)
        send("panel_action", id=row["id"])

    def click(self, send, frame, label):
        buttons = [button for item in frame["items"] for button in [item, *item.get("buttons", [])]]
        button = next(button for button in buttons if button.get("label") == label)
        self.assertNotIn("command", button)
        send("panel_action", id=button["id"])

    def split_preview(self, frame, shell=False):
        commands = [item["command"] for item in frame["items"] if "command" in item]
        self.assertEqual(len(commands), 1)
        self.assertEqual(commands[0]["action"], "open_split")
        self.assertIs(commands[0].get("confirm_on_click", False), shell)
        self.assertEqual(any(item["type"] == "input" for item in frame["items"]), shell)
        return commands[0]

    def test_shell_and_logs_bind_the_container_id_and_pane_revision(self):
        for target in [LOCAL, SSH]:
            with self.subTest(target=target), open_panel(target, self.docker) as (send, view):
                self.select(send, view("api-one"), "api-one")
                summary = view("work-bridge")
                self.assertFalse(any("command" in item or item["type"] == "input" for item in summary["items"]))
                self.click(send, summary, "Shell")
                shell = self.split_preview(view("open_split"), shell=True)
                send("panel_input", text="/opt/custom/bin/fish")
                changed = view("/opt/custom/bin/fish")
                alternate = self.split_preview(changed, shell=True)
                self.assertIn(self.container_id, " ".join(alternate["argv"]))
                self.assertIn("/opt/custom/bin/fish", " ".join(alternate["argv"]))
                self.click(send, changed, "Back")
                self.click(send, view("work-bridge"), "Follow logs")
                logs = self.split_preview(view("open_split"))
                self.assertIn("exec", " ".join(shell["argv"]))
                self.assertIn("logs", " ".join(logs["argv"]))
                for cmd in [shell, logs]:
                    self.assertEqual(cmd["revision"], 41)
                    self.assertIn(self.container_id, " ".join(cmd["argv"]))
                    self.assertTrue(cmd["title"])
                self.assertNotEqual(shell["title"], logs["title"])
                self.assertIn("--tail", " ".join(logs["argv"]))
                self.assertIn("100", " ".join(logs["argv"]))
                self.assertRegex(" ".join(logs["argv"]), r"(^| )(-f|--follow)( |$)")
                if target == SSH:
                    self.assertEqual(shell["argv"][0], "ssh")
                    offset = shell["argv"].index("-l")
                    self.assertEqual(shell["argv"][offset:offset + len(SSH["args"])], SSH["args"])

    def test_inspection_summary_keeps_full_details_behind_a_click(self):
        for target in [LOCAL, SSH]:
            def query(actual_target, args):
                self.assertEqual(actual_target, target)
                return self.docker(actual_target, args)

            with self.subTest(target=target), open_panel(target, query) as (send, view):
                self.select(send, view("api-one"), "api-one")
                summary = view("work-bridge")
                rendered = json.dumps(summary)
                for value in ["api:release", "running", "healthy", "18080", "/var/assets"]:
                    self.assertIn(value, rendered)
                self.assertNotIn("only-in-full-json", rendered)
                self.click(send, summary, "Inspect full JSON")
                full = self.split_preview(view("open_split"))
                self.assertEqual(full["revision"], 41)
                argv = " ".join(full["argv"])
                self.assertIn("inspect", argv)
                self.assertIn(self.container_id, argv)

    def test_inspecting_a_disappeared_container_reports_the_error(self):
        def query(target, args):
            if "inspect" in args:
                self.assertIn(self.container_id, args)
                raise RuntimeError("No such object")
            return self.docker(target, args)

        with open_panel(SSH, query) as (send, view):
            self.select(send, view("api-one"), "api-one")
            error = view("No such object")
            self.assertIn(SSH["host"], json.dumps(error))
            self.assertFalse(any("command" in item for item in error["items"]))

    def test_remote_project_offers_exact_paths_in_merge_order(self):
        with open_panel(SSH, self.docker) as (send, view):
            self.select(send, view("work-stack"), "work-stack")
            edits = [item["command"] for item in view("edit_file")["items"]
                     if item.get("command", {}).get("action") == "edit_file"]
            self.assertEqual([cmd["path"] for cmd in edits], self.paths)
            self.assertEqual([cmd["revision"] for cmd in edits], [41, 41])


if __name__ == "__main__":
    unittest.main()
