# Chart RUNBOOK — `charts/paigasus` (SMA-513)

Operator reference for the Paigasus Helm chart. It installs the IAM console, the AI Gateway
console and the IAM backend behind one ingress origin. The design is in
`docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md` and its two
addenda. `charts/paigasus/README.md` holds the developer detail.

## 1. Values reference

| Key | Required | Meaning |
| -- | -- | -- |
| `zones.<id>.enabled` | — | Turns a zone on or off. It controls six projections together (§ 4) |
| `zones.<id>.basePath` | — | The zone's path prefix. It must start with `/`, must not end with `/`, and must be unique |
| `zones.<id>.console.image.{repository,tag}` | — | The console image. `tag` defaults to the chart `appVersion` |
| `zones.<id>.console.replicas` | — | Console replicas (default 2) |
| `zones.iam.backend.image.{repository,tag}` | — | The IAM image |
| `zones.iam.backend.apiKeysPepperSecret` | yes | A Secret with key `pepper`: base64 of at least 32 bytes |
| `zones.iam.backend.apiKeysSecretVersion` | no | Change it after you rotate the pepper Secret, so the IAM pod restarts |
| `zones.gateway.backend.url` | when `gateway` is on | The base URL of an existing gateway backend. The chart does not deploy it |
| `ingress.host` | yes | The one public host. `PAIGASUS_PUBLIC_ORIGIN` is `https://<host>` |
| `ingress.className` | no | The IngressClass of your controller |
| `ingress.tlsSecretName` | yes | The TLS Secret for `ingress.host`. The ingress must end TLS |
| `ingress.annotations` | no | Extra annotations. Do not add a rewrite annotation (§ 3) |
| `oidc.issuer` | yes | The IdP issuer URL. It must be `https` |
| `oidc.clientId` | yes | The console's OIDC client. IAM also uses it as the access-token audience. Set `oidc.audience` to use a different value (§ 6) |
| `oidc.audience` | no | The access-token audience IAM accepts. Default: `oidc.clientId`. Set it only when the IdP puts another value in `aud` (§ 6) |
| `oidc.existingSecret` | yes | A Secret with keys `oidc-client-secret` and `session-redis-url` |
| `oidc.secretVersion` | no | Change it after the Secret changes, so the console pods restart |
| `oidc.caBundle.existingConfigMap` | no | A ConfigMap with the PEM root certificates of a private IdP CA (§ 7) |
| `oidc.caBundle.key` | no | The key in that ConfigMap (default `ca.crt`) |
| `oidc.caBundle.version` | no | Change it after the ConfigMap changes, so all three Deployments restart |
| `postgres.existingSecret` | yes | A Secret with key `database-url`: the complete Postgres DSN |
| `postgres.secretVersion` | no | Change it after the Secret changes, so the IAM pod restarts |

## 2. Refused combinations

`paigasus.validate` in `templates/_helpers.tpl` stops the render with its own message. The chart
refuses:

- no enabled zone;
- the `gateway` zone without the `iam` zone;
- a zone id that is not a known service slug;
- a `basePath` that is empty, has no leading `/`, ends with `/`, or is used by two zones;
- an empty value for each required key in § 1;
- `zones.iam.backend.deploy: false` (an external IAM is not supported);
- `zones.gateway.backend.deploy: true` (the chart cannot run the gateway backend);
- `zones.<id>.backend.url` empty when the chart does not deploy that backend;
- `oidc.caBundle.existingConfigMap` set with an empty `oidc.caBundle.key`.

## 3. No rewrite annotation

Do not add a rewrite annotation to the ingress. Each console has its `basePath` compiled in and
serves its full path. A rewrite that removes `/iam` breaks every route in that zone.

## 4. One values block, six projections, and the zone-map upgrade

A zone's `enabled` flag controls six things together. These are its ingress rule, its
`PAIGASUS_ZONES` entry, its `PAIGASUS_SERVICES` entry, `PAIGASUS_IAM_GRPC_URL`, its console
Deployment and Service, and its backend Deployment and Service. So a zone cannot be routed but not
shown, or shown but not routed. This is decision D6.

**The limit of D6.** The chart cannot express "the gateway zone is shown but not routed", or the
reverse. That is on purpose.

**Upgrade order for a zone-map change.** One `helm upgrade` changes the Ingress, the zone-map
ConfigMap and the Deployments at the same time. Helm does not order them. The consoles restart
because the `checksum/zonemap` annotation changes. During the restart:

- When you remove a zone, its ingress rule goes away at once. Old console pods of the other zones
  still show a link to it until their replacements are Ready. That link then gives the
  controller's 404.
- When you add a zone, the new zone can be Ready before the old pods of the other zones show it.

Do not test the result until no old console pod exists. `helm upgrade --wait` waits for the new
pods, not for the old pods to go. The kind job waits for both (`ci/kind/run.sh`, the settle step).

## 5. What restarts what

| You change | Restarts | Why |
| -- | -- | -- |
| a value that changes a console's zone map or env | the consoles | `checksum/zonemap`, `checksum/console-env` |
| the contents of `oidc.existingSecret` | nothing, until you change `oidc.secretVersion` | the chart cannot see the Secret |
| the contents of `postgres.existingSecret` or the pepper Secret | nothing, until you change its version value | the same |
| the contents of the CA ConfigMap | nothing, until you change `oidc.caBundle.version` | Node and IAM read the file once, at start |
| `oidc.audience` | the IAM pod, not the consoles | it changes `IAM_AUTHN__ISSUERS` in the IAM pod template. IAM has one replica and `maxSurge: 0` (`templates/backend-deployment.yaml`). IAM is not available during the restart. |

A change of `oidc.caBundle.version` restarts every pod that mounts the bundle: both consoles and
IAM. That is expected, as for `oidc.secretVersion`.

## 6. The IdP contract

IAM validates the **access token** that the console sends with each gRPC call. It does not
validate the ID token. The login callback calls `authn.whoAmI` with the access token. A refused
token does NOT stop the login. The console then shows IAM as unusable. So a wrong IdP setup shows
late and unclearly. Check these four items before you install:

1. **Audience.** The access token's `aud` claim must contain the audience that IAM accepts. IAM
   accepts `oidc.audience` when it is set. It accepts `oidc.clientId` when `oidc.audience` is not
   set. Set `oidc.audience` in two cases. First, set it when you cannot make the IdP put the
   client id into `aud`. Second, set it when the IdP's ID token has no Keycloak `typ` claim. The
   paragraph after this list tells you how to choose the value in that case. The value replaces
   the client id. It does not add another value next to the client id. Before you choose the
   value, decode a real access token and read its `aud` claim. The value helps only when the IdP
   issues a JWT access token for the console's scopes (`openid profile email offline_access`).
   The console sends no `audience` or `resource` parameter. This value does not work with an
   opaque token, or a token for a different API.
   A wrong or missing audience shows in the IAM log at `info`, with the issuer and the accepted
   audiences (`ci/kind/README.md`, "Where to look first"). The log does not show the token's
   `aud`.
2. **Email.** The access token must carry an `email` claim. IAM creates the principal on the first
   login from it.
3. **Algorithm.** The token must be signed with RS256 or ES256, and its header must carry a `kid`.
4. **Discovery.** The discovery document's `issuer` must equal `oidc.issuer`, and its `jwks_uri`
   must be `https`.

**IAM refuses a token that is not an access token (SMA-686).** IAM refuses a bearer token that
has one of these markers:

- A `typ` claim of `ID` or `Logout`, in any letter case. Keycloak sets these values on its ID
  token and on its back-channel logout token. Its access token has `typ: Bearer`.
- A header `typ` of `logout+jwt`, or an `events` claim with the back-channel logout event. An IdP
  that supports OpenID Connect Back-Channel Logout sets these on its logout token.

The IAM log shows a refusal at `info`, with the issuer and the matched marker. For each issuer
and each kind of refusal, IAM writes at most one line in 10 seconds. The next line gives the
number of refusals that IAM did not log.

This adds no requirement on the IdP. No Keycloak or Dex access token measured for SMA-686 has
one of these markers. Do not add a mapper that sets `typ` on the access token.

The check does not protect an IdP whose ID token has no `typ` claim. Dex is an example. Decode a
real ID token and a real access token from your IdP. If the ID token has no `typ: ID`, find an
audience for `oidc.audience`. The access token's `aud` must contain it. The ID token's `aud` must
not contain it. If your IdP cannot do this, IAM accepts its ID token as a bearer token. For Dex,
both tokens have the same `aud`, so this remedy does not work.

The console requests the scopes `openid profile email offline_access`.

**Keycloak example.** Keycloak does not put the client id into the access token's `aud` by
default. Add an audience mapper to the client:

```json
{
  "name": "paigasus-console-audience",
  "protocol": "openid-connect",
  "protocolMapper": "oidc-audience-mapper",
  "config": {
    "included.custom.audience": "paigasus-console",
    "id.token.claim": "false",
    "access.token.claim": "true"
  }
}
```

Put `basic`, `profile`, `email` and `offline_access` in the client's default client scopes. In
Keycloak 25 and later the `sub` claim comes from the `basic` scope. Give each user an email
address and the `offline_access` role. The kind job's realm, `ci/kind/realm/paigasus-realm.json`,
is a complete example.

**Okta example. Not tested against a live Okta tenant.** An Okta authorization server puts its own
audience into the access token's `aud`, not the client id. The default authorization server uses
`api://default`.

1. Set `oidc.issuer` to the authorization-server issuer (`https://<org>.okta.com/oauth2/default`,
   or your custom server). Do not use the org authorization server (`https://<org>.okta.com`).
   Other parties must not validate its access tokens.
2. Set `oidc.audience=api://default`, or the audience of your custom server. Quote the value in a
   values file.
3. Add an access policy to that authorization server, with a rule for the console client. Okta
   issues no token without one. The `default` server of an Integrator Free Plan org has no
   access policy. The rule must allow the authorization code and refresh token grants. It
   must allow the scopes `openid`, `profile`, `email` and `offline_access`. See the Okta
   guide "Create access policies".
4. Add an `email` claim (value `user.email`, included in the access token) to that authorization
   server. IAM needs `email` (item 2).

**Warning.** In Okta's default configuration, the access token's `sub` can be the user's login, not
the fixed user id. IAM keys the identity on the access token's `(iss, sub)`. If that is true for
your tenant, a login rename makes a new identity. Check `sub` in a real access token before
production use. This chart did not verify Okta's `sub` behavior.

## 7. An IdP with a private CA (`oidc.caBundle`)

The consoles and IAM refuse an issuer that is not `https`. If the IdP certificate chain ends at a
private CA, put the CA's PEM root certificates into a ConfigMap and set
`oidc.caBundle.existingConfigMap`. A CA certificate is public, so it is a ConfigMap, not a Secret.

```bash
kubectl -n <namespace> create configmap paigasus-idp-ca --from-file=ca.crt=<your-root-ca.pem>
helm upgrade <release> charts/paigasus --reuse-values \
  --set oidc.caBundle.existingConfigMap=paigasus-idp-ca
```

The chart then mounts the key read-only under `/etc/paigasus/idp-ca/` in every console pod and in
the IAM pod. The consoles get `NODE_EXTRA_CA_CERTS`. IAM gets `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH`.

**Failure modes.**

- **A missing ConfigMap or a missing key.** Every pod that mounts it stops at volume setup, with a
  `FailedMount` event. The pod does not start. Read `kubectl describe pod`.
- **A file with no PEM certificate.** IAM does not start. Its log names the bundle path.
- **A key whose content is not a valid PEM certificate, for the consoles.** This case fails OPEN.
  Node prints a warning and starts. The failure shows only at login, as a TLS error against the
  IdP in the console log. After a change, read the console log for a `NODE_EXTRA_CA_CERTS` warning.
- **Scope.** The consoles trust the bundle for EVERY TLS connection, Redis included. IAM trusts it
  only for JWKS fetches. Put only the roots the IdP needs in the bundle.
- **Rotation.** After you change the ConfigMap, change `oidc.caBundle.version`. Node and IAM read
  the bundle only at start.

## 8. The kind job

`.github/workflows/chart.yml` installs this chart into kind and runs the Playwright specs in
`ts/apps/iam-console/tests/cluster/`. It is not a required check. It uses **Traefik** as the
ingress controller, because the Kubernetes project retired ingress-nginx (announced on 2025-11-11;
the repository was archived on 2026-03-24). The chart renders a plain `networking.k8s.io/v1`
Ingress with no controller annotation, so it works with any controller that serves that API.
Set `ingress.className` to your controller's class.

To run the job by hand on a branch: `gh workflow run chart.yml --ref <branch>`. To run it locally,
see `ci/kind/README.md`. The job maps the two host names with a CoreDNS `hosts` block. That is a
kind-only device: a real cluster needs real DNS for the IdP host.

The job runs the steps in this order: `up`, `images`, `install a`, `specs a`, `stub up`,
`specs journeys`, `upgrade b`, `specs b`. `stub up` starts a static gateway stub, so the gateway
zone is `available` for the two journeys (SMA-514). `stub up` and `specs journeys` run when
`install a` succeeded, even if `specs a` failed. Phase B runs only when every step before it passed.

To re-run one journey on a local kind cluster after `stub up`:

    STATE="${PAIGASUS_KIND_STATE:-${TMPDIR:-/tmp}/paigasus-kind}"
    PAIGASUS_KIND_USERNAME=paigasus-kind PAIGASUS_KIND_PASSWORD="$(cat "$STATE/user-password")" \
      PAIGASUS_KIND_OUTPUT_DIR="$STATE/diagnose/playwright/journeys" \
      pnpm --dir ts/apps/iam-console exec playwright test \
        --config tests/cluster/playwright.config.ts --project journeys journeys/auth-roundtrip.spec.ts

Use `journeys/zone-round-trip.spec.ts` for the other one. This direct run skips the job's guards (the skip scan, the exactly-2 count and the report check). Only `run.sh specs journeys` applies them. In CI there is no per-journey re-run: re-run the failed job with `gh run rerun <run-id> --failed`.
