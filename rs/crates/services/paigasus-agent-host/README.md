# paigasus-agent-host

The agent host service. This crate holds the configuration, the database migrations and the HTTP
surface of the service. The surface serves `/healthz`, `/readyz` and `/metrics`. Any other path
returns the 404 error envelope.

The agent host needs its own database. A separate schema in a shared database is not supported.

## Configuration

A TOML file `agent-host.toml` and environment variables set the configuration. The environment
prefix is `AGENT_HOST_`. A double underscore sets a nested key, for example
`AGENT_HOST_METRICS__ADDR`. An environment variable overrides the file.

| Key | Environment variable | Default | Meaning |
|---|---|---|---|
| `http_addr` | `AGENT_HOST_HTTP_ADDR` | `0.0.0.0:8086` | Address of the HTTP listener. |
| `database_url` | `AGENT_HOST_DATABASE_URL` | none (required) | Postgres connection string. |
| `log_level` | `AGENT_HOST_LOG_LEVEL` | `info` | Log filter. |
| `metrics.enabled` | `AGENT_HOST_METRICS__ENABLED` | `true` | Serve `/metrics`. |
| `metrics.addr` | `AGENT_HOST_METRICS__ADDR` | none | Separate metrics listener. Its port must differ from the port of `http_addr`. |
| `migration.lock_wait_secs` | `AGENT_HOST_MIGRATION__LOCK_WAIT_SECS` | `120` | Seconds to wait for the migration lock. Range 1 to 3600. |
| `memory.backend` | `AGENT_HOST_MEMORY__BACKEND` | `postgres` | Memory store backend. Only `postgres` exists now. A backend switch does not move data. |
| `memory.caps.max_file_bytes` | `AGENT_HOST_MEMORY__CAPS__MAX_FILE_BYTES` | `65536` | Largest memory file, in bytes. Greater than 0. |
| `memory.caps.max_store_bytes` | `AGENT_HOST_MEMORY__CAPS__MAX_STORE_BYTES` | `1048576` | Largest memory total of a scope, in bytes. Greater than 0. |
| `memory.caps.max_store_files` | `AGENT_HOST_MEMORY__CAPS__MAX_STORE_FILES` | `512` | Largest number of memory files in a scope. Greater than 0. |
| `memory.caps.max_loaded_file_bytes` | `AGENT_HOST_MEMORY__CAPS__MAX_LOADED_FILE_BYTES` | `8192` | Largest file that `view` returns in full. Greater than 0, at most `max_file_bytes`. |

## Migrations

The service keeps its migration state in the table `agent_host_migrations`. It runs the migrations
under the advisory lock with the key `5_580_786`.

## Local run

Run `moon run paigasus-agent-host-rs:dev`. The command calls `scripts/dev.sh`.

The script starts a Postgres 16 container on a free port of `127.0.0.1`. Then it runs
`cargo run --locked -p paigasus-agent-host` with `AGENT_HOST_DATABASE_URL` and
`AGENT_HOST_HTTP_ADDR=127.0.0.1:<free port>`. The script prints both ports. Press Ctrl-C to stop
the service. The script then removes its container.

Use `scripts/dev.sh --check` to test the boot. The script builds the service first. Then it starts
the service, waits up to 60 s for `/readyz` to return 200, and stops. It exits 0 on success and 1
on failure.

Each container has the label `paigasus.dev=agent-host` and the label `paigasus.dev.pid=<pid of the
script>`. At start, the script removes only labelled containers whose process is no longer alive.
A second run from another worktree keeps its own container. A SIGKILL leaves a container. The next
run removes it.
