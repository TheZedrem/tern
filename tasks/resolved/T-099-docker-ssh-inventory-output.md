---
status: resolved
blocked-by: []
type: bug
---
# T-099: Handle unexpected Docker container-list output over SSH

On Windows with an SSH pane selected, the Docker sidebar shows the remote
account and Refresh, then “Docker unavailable: Expecting value: line 1
column 1 (char 0)”. The container query returned success but at least
one nonblank stdout line could not be decoded as JSON. Compose is never
queried on that path. Empty stdout already yields “No running containers.”

The owner's reproduction on the remote host prints the literal word
`json` once per container. This CLI treats bare `--format json` as
template text. Use Docker's explicit Go template `{{json .}}` for
`docker ps` and test the format argument through SSH quoting.
Verify that valid inventory is shown
and that malformed output gets a container-list diagnostic distinct
from connection failure, without dumping remote output into logs.

Area: plugins/docker. Spec: ../spec/0016-plugin-system.md

The owner verified the explicit template returns JSON on the affected
host. The plugin now uses it for local and SSH container queries and
identifies unreadable container-list output separately from CLI failures.
