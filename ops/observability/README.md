# Observability stack (local)

Local Prometheus + Grafana stack for scraping `/metrics` off the IAM and
Gateway services during development. Not for production use.

## Quickstart

1. Start the two services (each exposes `/metrics` on its HTTP listener):

   ```bash
   cargo run -p paigasus-iam       # http://0.0.0.0:8080/metrics
   cargo run -p paigasus-gateway   # http://0.0.0.0:8088/metrics
   ```

2. Start the stack:

   ```bash
   cd ops/observability
   docker compose up
   ```

3. Prometheus: <http://localhost:9090> (scrapes both services via
   `host.docker.internal`). Grafana: <http://localhost:3000> (anonymous
   admin login, provisioned dashboards under **Dashboards**).

4. Stop with `docker compose down` (add `-v` to also drop Prometheus's TSDB
   volume).

## Layout

- `docker-compose.yml` — Prometheus + Grafana services.
- `prometheus/prometheus.yml` — scrape config for the `iam` and `gateway`
  jobs, plus alert-rule loading.
- `prometheus/rules/` — alerting/recording rules (`*.rules.yml`), with
  `promtool`-driven unit tests under `prometheus/rules/tests/`.
- `grafana/provisioning/` — Grafana datasource + dashboard-provider config.
- `grafana/dashboards/` — provisioned dashboard JSON for both services.

## Runbook

For alert response and troubleshooting, see
[`docs/ops/RUNBOOK-observability.md`](../../docs/ops/RUNBOOK-observability.md).
