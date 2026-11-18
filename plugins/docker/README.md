# Docker sidebar

Open the Docker panel to list running containers and Compose projects for
the active terminal. Switch panes to change machines. Use **Refresh** to
query the same machine again.

## Install

1. Run a Tern build that supports `follow_pane`. In **Settings → Plugins**
   click **Open plugins folder**.
2. Copy this whole `docker` folder there so the manifest is at
   `plugins/docker/plugin.yaml`. Restart Tern to discover it.
3. Enable **Docker**, select **Read pane and SSH connection details**
   (`pane_info`), and open **Docker** in the sidebar. **Refresh** is
   beside the machine name.

The plugin uses Python 3.8+ from PATH (`python` on Windows, `python3` on
Linux). It needs only the standard library. Install the Docker CLI and
Compose v2 on each machine whose inventory you want to see.

## Which engine is queried?

- **Windows 11** uses Docker Desktop's local `docker_engine` named pipe.
- **Linux** uses a local Unix-socket `DOCKER_HOST`, the user's rootless
  socket, or `/var/run/docker.sock`.
- **SSH profile** uses system OpenSSH to run the two list commands on
  the pane's host, with its alias, user, port and identity-file path. The
  host needs a POSIX shell and Docker access for that account. Accept
  the host key in a normal SSH connection first and use a key or agent
  that can authenticate without prompting. A manually typed `ssh` in a
  local shell still counts as a local pane.

Compose rows include stopped projects known to the engine and their
reported configuration paths. Each section shows up to 200 entries.
There is no background polling. Inventory stays in memory.
