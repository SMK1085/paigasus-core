# paigasus-agent-host

The agent host service. This crate holds the configuration and the database migrations of the
service. The HTTP surface comes in a later change.

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

## Migrations

The service keeps its migration state in the table `agent_host_migrations`. It runs the migrations
under the advisory lock with the key `5_580_786`.
