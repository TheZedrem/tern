import importlib.util
import io
import json
import shlex
import subprocess
import threading
import unittest
from pathlib import Path
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


if __name__ == "__main__":
    unittest.main()
