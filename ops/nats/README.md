# `ops/nats/`

Provisioning and permission artifacts for the `PAIGASUS_IAM` NATS account (SMA-493) — the account
`paigasus-iam`'s outbox relay publishes into, and the AI Gateway's cache-invalidation client
(SMA-492) pulls from. Everything an operator needs to stand up that account in a new environment,
and everything a test needs to prove the deployed permission lists behave as designed, lives here.

For the reasoning behind the specific subject grants — why the publisher needs a `subscribe`
grant, why a pull consumer's filter can't be enforced with permissions, what the JetStream-domain-
prefixed forms look like, and more — see [`permissions.md`](./permissions.md). This file covers
only the mechanics of using the directory; `permissions.md` covers the *why*.

## What's here

| path | purpose |
|---|---|
| `subjects.env` | The single source of truth for every subject/inbox-prefix grant. `provision.sh` and `check-subjects.sh` both source it — edit grants here, nowhere else. |
| `provision.sh` | Mints the account, its three users, the `.creds` files, the stream, and the durable consumer against a real broker. Run once per environment, by hand. |
| `check-subjects.sh` | CI gate: asserts `test/accounts.conf.tmpl` grants *exactly* the subjects declared in `subjects.env`, in both directions. Keeps the artifact that is tested from drifting away from the artifact that is deployed. |
| `permissions.md` | The design rationale behind every grant — read this before editing `subjects.env`. |
| `test/accounts.conf.tmpl` | A static-nkey mirror of the production account, for a `nats-server` fixture in integration tests. `{{SYS_NKEY}}`, `{{PUBLISHER_NKEY}}`, `{{CONSUMER_NKEY}}`, `{{PROVISIONER_NKEY}}` are rendered at test time with freshly minted public keys — never committed key material. |
| `test/nats-server.conf` | Plaintext fixture broker config; `include "accounts.conf"` (the rendered template). |
| `test/nats-server-tls.conf` | Same fixture, plus a `tls` block expecting a certificate/key at `/etc/nats/server-cert.pem` / `server-key.pem`, minted per test run. |

## Requirements

Provisioning a real environment needs **three** separate tools — no single one of them is
sufficient:

- **`nsc`** — mints the operator, the `PAIGASUS_IAM` account, its three users, and their `.creds`
  files (JWT + nkey seed). This is account/identity management; it never talks to a running
  broker.
- **`nats`** (the CLI) — creates the `IAM_EVENTS` stream and the `gateway-cache-invalidator`
  durable consumer. This *does* talk to a running broker and requires one of the `.creds` files
  `nsc` just produced.
- **A running `nats-server`** that has loaded the resolver config `nsc` generates. Without it,
  neither the account nor its users exist from the broker's point of view, and the `nats` CLI step
  above has nothing to connect to.

## Run order

`provision.sh` is a two-pass script because minting an account's identities and getting a broker
to recognize them are genuinely separate steps with a manual action in between:

1. Run `NATS_URL=tls://<broker>:4222 ./provision.sh`. This mints the operator, account, and three
   users (writing their `.creds` files to `$OUT_DIR`, default `./out`), then generates a resolver
   config stanza at `$OUT_DIR/resolver.conf` and stops — it does **not** yet create the stream or
   consumer, because the broker doesn't know about the account yet.
2. **Include** the generated `$OUT_DIR/resolver.conf` in the broker's `nats-server.conf`.
3. **Restart** the broker so it picks up the resolver config.
4. Re-run with `PUSH=1`: `PUSH=1 NATS_URL=tls://<broker>:4222 ./provision.sh`. This pushes the
   account JWT to the now-listening broker (`nsc push`) and then, against that live broker,
   creates the `IAM_EVENTS` stream and the `gateway-cache-invalidator` durable consumer.

Deploy `iam-publisher.creds` with `paigasus-iam` and `gateway-consumer.creds` with the gateway.
**Never** ship `iam-provisioner.creds` in a deployment — see `permissions.md` §9 for why it is an
operator artifact, not a service credential.

`paigasus-iam`'s production `[outbox.publisher]` block, consuming the `iam-publisher.creds` just
deployed:

```toml
[outbox.publisher]
backend          = "nats"
url              = "tls://nats.internal:4222"
credentials_file = "/etc/paigasus/iam-publisher.creds"
root_ca_bundle   = "/etc/paigasus/nats-ca.pem"  # REPLACES the system trust store
inbox_prefix     = "_INBOX_IAM_PUB"             # MUST match PUBLISHER_INBOX_PREFIX
```

See `permissions.md` §6 for why `root_ca_bundle` replaces rather than extends the system trust
store, and §7 for why `inbox_prefix` must match this account's grant exactly.

## Widening an existing user's grant

Adding a subject to `subjects.env` for a user that already exists is **not** what re-running
`provision.sh` does. `add_user`'s `nsc add user` call fails against an existing user, the script's
own guard swallows that failure ("user $name already exists"), and it then regenerates `.creds`
from the **unchanged** local JWT — the grant is not widened, the run exits `0`, and nothing tells
you it didn't work. Re-running `provision.sh` to pick up a `subjects.env` edit only works for a
**brand-new** user; for an existing one it is a silent no-op.

To actually widen a live user's permissions:

```bash
nsc edit user --account PAIGASUS_IAM --name <user> --allow-pub '<subject>'
# repeat --allow-pub / --allow-sub (each additive, not a replacement) for every subject being
# added, then regenerate the .creds file so it carries the newly re-signed permission set:
nsc generate creds --account PAIGASUS_IAM --name <user> > <path-to-deploy>.creds
```

`<user>` is one of `iam-publisher` / `gateway-consumer` / `iam-provisioner`; `<subject>` is exactly
what you added to that user's `*_PUB`/`*_SUB` array in `subjects.env` (keep the file and `nsc`'s
local state in sync by hand — editing one does not update the other).

**No `nsc push` here.** Unlike an account-level change (JetStream limits, signing keys — what the
`provision.sh` phase-split's push step is for), a user's permission grant is baked directly into
that user's own signed JWT and is never uploaded to the broker at all — the client presents it at
`CONNECT` time, and the broker verifies its signature against the account key it already has. The
step that actually matters is regenerating the `.creds` file (the local JWT changed) and delivering
it to wherever the service reads it, via the same **atomic replacement** rotation requires
(`RUNBOOK-nats.md` §5, `permissions.md` §8) — never a truncate-and-rewrite in place.
`NatsEventPublisher` re-reads the file on every connection attempt (D8), so the running service
picks up the wider grant on its next reconnect; no restart is needed either way.

## More

For deployment/runbook-level operational guidance (rotation, incident response, monitoring),
see [`docs/ops/RUNBOOK-nats.md`](../../docs/ops/RUNBOOK-nats.md).
