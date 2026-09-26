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
| `zones.<id>.console.image.{repository,tag}` | — | The console image. `tag` is pinned to the published console version (for example `0.1.0`). An empty `tag` falls back to the chart `appVersion`. That is a released version of every image, but it can be older than the pinned tags. It is the fallback tag, not the deployed version |
| `zones.<id>.console.replicas` | — | Console replicas (default 2) |
| `zones.iam.backend.image.{repository,tag}` | — | The IAM image. `tag` is pinned to the published IAM version, like the console tags |
| `zones.iam.backend.apiKeysPepperSecret` | yes | A Secret with key `pepper`: base64 of at least 32 bytes |
| `zones.iam.backend.apiKeysSecretVersion` | no | Change it after you rotate the pepper Secret, so the IAM pod restarts |
| `zones.iam.backend.bootstrapAdmins` | no | A list of `{issuer, subject}`. IAM grants `platform_admin` at Root to each identity after its first login. Default `[]`: no user can do anything (§ 9) |
| `zones.iam.backend.extraEnv` | no | More env entries (Kubernetes `EnvVar`) for the IAM container, for example `RUST_LOG`. Default `[]` (§ 9) |
| `zones.gateway.backend.url` | when `gateway` is on | The base URL of an existing gateway backend. The chart does not deploy it |
| `ingress.host` | yes | The one public host. `PAIGASUS_PUBLIC_ORIGIN` is `https://<host>` |
| `ingress.className` | no | The IngressClass of your controller |
| `ingress.tlsSecretName` | yes | The TLS Secret for `ingress.host`. The ingress must end TLS |
| `ingress.annotations` | no | Extra annotations. Do not add a rewrite annotation (§ 3) |
| `oidc.issuer` | yes | The IdP issuer URL. It must be `https` |
| `oidc.clientId` | yes | The console's OIDC client. By default IAM also uses it as the access-token audience. Then an ID token passes IAM's audience check, and the chart shows a warning (§ 6) |
| `oidc.audience` | no | The access-token audience IAM accepts. Default: `oidc.clientId`. Recommended: a dedicated API audience. Follow the migration order in § 6 |
| `oidc.acknowledgeClientIdAudience` | no | Set it to the value of `oidc.clientId` to remove the audience warning (§ 6). It does not change what IAM accepts |
| `oidc.scopes` | no | The scopes that both consoles request. Empty: `openid profile email offline_access`. The list must contain `openid`, or the render fails. Keep `offline_access`, or the IdP issues no refresh token. When set, the consoles also send it on each refresh. Entra ID needs it (§ 6) |
| `oidc.authorizationAudience` | no | The `audience` parameter that both consoles send in the authorization request. Empty: no `audience` parameter. It must equal `oidc.audience`, or the render fails. Auth0 needs it (§ 6) |
| `oidc.existingSecret` | yes | A Secret with keys `oidc-client-secret` and `session-redis-url` |
| `oidc.secretVersion` | no | Change it after the Secret changes, so the console pods restart |
| `oidc.caBundle.existingConfigMap` | no | A ConfigMap with the PEM root certificates of a private IdP CA (§ 7) |
| `oidc.caBundle.key` | no | The key in that ConfigMap (default `ca.crt`) |
| `oidc.caBundle.version` | no | Change it after the ConfigMap changes, so all three Deployments restart |
| `postgres.existingSecret` | yes | A Secret with key `database-url`: the complete Postgres DSN |
| `postgres.secretVersion` | no | Change it after the Secret changes, so the IAM pod restarts |

## 2. Refused combinations

`paigasus.validate` in `templates/_helpers.tpl` stops the render with its own message. The last
two items in this list are in `templates/_audience.tpl` instead. `templates/console-env-configmap.yaml`
calls them, and that file renders on every install. The chart refuses:

- no enabled zone;
- the `gateway` zone without the `iam` zone;
- a zone id that is not a known service slug;
- a `basePath` that is empty, has no leading `/`, ends with `/`, or is used by two zones;
- an empty value for each required key in § 1;
- `zones.iam.backend.deploy: false` (an external IAM is not supported);
- `zones.gateway.backend.deploy: true` (the chart cannot run the gateway backend);
- `zones.<id>.backend.url` empty when the chart does not deploy that backend;
- `oidc.caBundle.existingConfigMap` set with an empty `oidc.caBundle.key`;
- `oidc.scopes` set without the scope `openid` (`openidx` does not count);
- `oidc.authorizationAudience` set with leading or trailing whitespace;
- `oidc.authorizationAudience` set while `oidc.audience` is empty, or set to a value that does not
  equal `oidc.audience`;
- a `zones.iam.backend.bootstrapAdmins` value that is not a list, or an entry that is not a map;
- a bootstrap admin with an empty or missing `issuer` or `subject`, or with a value that is not a
  string (quote a subject of digits only);
- a bootstrap admin `issuer` that is not `https`, or that is not equal to `oidc.issuer` (§ 9);
- a `zones.iam.backend.extraEnv` value that is not a list, an entry without a string `name`, or
  two entries with one name;
- an `extraEnv` name that the chart sets itself, or a name that starts with such a name and `__`
  (§ 9).

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
| `zones.iam.backend.bootstrapAdmins` or `zones.iam.backend.extraEnv` | the IAM pod, not the consoles | it changes the env in the IAM pod template (`tests/env.sh` rows B7a and B7b). IAM is not available during the restart, as for `oidc.audience` |
| `oidc.acknowledgeClientIdAudience` | nothing | it changes only the IAM Deployment's `metadata` annotation and the NOTES, not a pod template (`tests/env.sh` row W14) |
| `oidc.scopes` or `oidc.authorizationAudience` | both consoles, not IAM | it changes the `console-env` ConfigMap and so `checksum/console-env` (`tests/env.sh` rows O5 and O6) |

A change of `oidc.caBundle.version` restarts every pod that mounts the bundle: both consoles and
IAM. That is expected, as for `oidc.secretVersion`.

## 6. The IdP contract

IAM validates the **access token** that the console sends with each gRPC call. It does not
validate the ID token. The login callback calls `authn.whoAmI` with the access token. A refused
token does NOT stop the login. The console then shows IAM as unusable. So a wrong IdP setup shows
late and unclearly. Check these four items before you install:

1. **Audience.** The access token's `aud` claim must contain the audience that IAM accepts. IAM
   accepts `oidc.audience` when it is set. It accepts `oidc.clientId` when `oidc.audience` is not
   set. The recommended setup is a dedicated API audience. See "The recommended audience setup"
   after this list. The value replaces the client id. It does not add another value next to the
   client id. Before you choose the value, decode a real access token and read its `aud` claim.
   The value helps only when the IdP issues a JWT access token for the console's scopes
   (`oidc.scopes`, default `openid profile email offline_access`). The console sends an
   `audience` parameter only when `oidc.authorizationAudience` is set. It never sends a
   `resource` parameter. This value does not work with an opaque token, or a token for a
   different API.
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

The check does not protect an IdP whose ID token has no `typ` claim. For such an IdP, a
dedicated API audience is the only protection. This works only when the IdP can put a different
audience into the access token than into the ID token. Dex cannot: both tokens have the same
`aud`, so IAM accepts a Dex ID token as a bearer token. See "Dex" below.

**IAM refuses a sender-constrained token (SMA-690).** IAM refuses an access token that has one
of these markers:

- A `cnf` claim with any value except `null`. A DPoP-bound token (RFC 9449) and an mTLS-bound
  token (RFC 8705) have this claim.
- A `typ` claim of `DPoP`, in any letter case. Keycloak sets this value on a DPoP-bound access
  token.

IAM cannot check the binding of such a token. So it does not accept the token as a bearer token.

Keycloak binds an access token when the client sends a `DPoP` header to the token endpoint. The
client needs no DPoP setting for this. So a client that calls Paigasus must not send a `DPoP`
header to the token endpoint. A bound login stays bound when the client refreshes the token. A
client that got a bound token must log in again without a `DPoP` header.

Do not set the Keycloak client attribute `dpop.bound.access.tokens` on a client that calls
Paigasus. A Keycloak client policy can also require DPoP. Do not use such a policy for this
client (not measured).

The console does not send a `DPoP` header. The SDKs do not get tokens. They send the token that
you give them. If you use an SDK, do not turn on DPoP in your own OIDC library.

The IAM log shows the refusal at `info`: "it is bound to a key, and IAM cannot check the
binding". The line gives the issuer and the marker `cnf` or `typ DPoP`. The same rate limit
applies as for the refusal of a token that is not an access token.

By default the console requests the scopes `openid profile email offline_access`. Set
`oidc.scopes` to request a different list. The list must contain `openid`. Keep `offline_access`.

Without it, the IdP issues no refresh token. Every user must log in again when the access token
expires. When `oidc.scopes` is set, the console also sends the list as the `scope` of each
refresh request. When it is empty, a refresh request has no `scope`, as before SMA-692.

**Set `oidc.scopes` only when your IdP needs it.** This applies to any IdP, not only Entra ID
(see "Entra ID moving to a new scope list" below). RFC 6749 § 6 lets an authorization server
refuse a refresh `scope` that is not a subset of the originally granted scope. So a refresh with
this list can fail on an IdP that enforces that rule. The user is then signed out at each
access-token expiry, not only at the next login. After you set or change `oidc.scopes`, read the
`oauthError` field of the `session.refresh_failed` log line to check for this.

**The recommended audience setup (SMA-691).** An OIDC ID token has the client id as its `aud`.
So when the IAM audience equals `oidc.clientId`, an ID token passes IAM's audience check. This is
the default.

Give the API its own audience, for example `api://paigasus`. Put it into the access token's
`aud`. Do not put it into the ID token's `aud`. Then set `oidc.audience` to it.

**Migration order.** A set `oidc.audience` replaces the client id. IAM then refuses every live
access token whose `aud` holds only the client id. IAM also restarts with a gap, because it has
one replica and `maxSurge: 0` (§ 5). Do the steps in this order:

1. In the IdP, add the API audience to the access token, next to the client id. Do not add it to
   the ID token.
2. Wait for one access-token lifetime. Then every live access token has the new audience.
3. Set `oidc.audience` to the API audience and upgrade. IAM restarts and then accepts only the
   API audience.
4. Optional: remove the client id from the access token's `aud`.

If you do step 3 before step 1, IAM refuses the token of every console session. An IdP change
that replaces `aud`, and does not add to it, has the same result.

**Migration for Auth0 and Entra ID (SMA-692).** Step 1 above adds a second audience next to the
client id. Auth0 has no equal step: an Auth0 access token has one API audience. The chart also
refuses `oidc.authorizationAudience` unless it equals `oidc.audience`, so IAM and the consoles
change in one upgrade. The real cases:

- **Auth0 with a tenant Default Audience D, moving to `oidc.authorizationAudience` = D.** The
  tokens do not change. Nothing breaks.
- **Auth0 moving from no API (or from D) to a new API A.** This is a hard cut. A session that
  logged in before the upgrade keeps a token for the old audience, and an Auth0 refresh keeps
  that audience. IAM refuses those tokens. Every user must log in again. IAM and the consoles
  restart at different times.
- **Entra ID moving to a new scope list.** The refresh of an old session can fail, because the
  refresh now sends the new `oidc.scopes`. The error code is not measured. Entra ID reports many
  conditions as `invalid_grant`, which deletes the session. It reports others as
  `invalid_scope`, which ends the session when the access token expires. In both cases the user
  must log in again. The console log line `session.refresh_failed` shows `reason: rejected` for
  `invalid_grant`, and the code in the field `oauthError` for the other codes.
- **Mixed pods.** During the rollout, old and new console pods share one session store. For a
  short time, a pod with the other scope list can refresh a session.

**The warning.** The chart shows a warning when both of these conditions are true:

- The IAM audience equals `oidc.clientId`. The IAM audience is `oidc.audience`, or
  `oidc.clientId` when `oidc.audience` is empty.
- `oidc.acknowledgeClientIdAudience` does not equal `oidc.clientId`.

The warning has two forms:

- `helm install` and `helm upgrade` print it in the release NOTES. Flux's helm-controller also
  stores the NOTES in the release. The first line is `WARNING (SMA-691): the IAM audience equals
  oidc.clientId, so an ID token passes IAM's audience check.`
- The IAM backend Deployment gets the annotation `paigasus.io/iam-audience-warning` in its
  `metadata`. A tool that renders with `helm template`, for example Argo CD, does not show NOTES.
  Read the annotation there. The annotation is not on the pod template, so it does not restart a
  pod.

The warning does not stop an install or an upgrade. It does not change what IAM accepts. A GitOps
sync does not fail because of it.

**The acknowledgement.** If your IdP cannot give the API its own audience, set
`oidc.acknowledgeClientIdAudience` to the value of `oidc.clientId`. The warning then does not
show. The acknowledgement does not change what IAM accepts. IAM still accepts an ID token as a
bearer token, except a Keycloak ID token (SMA-686).

- The value must equal the client id exactly, with the same letter case. `true` does not work.
- When you change `oidc.clientId`, the warning shows again.
- Quote the value in a values file. An unquoted large number can change to an exponent form
  (`1e+06`), and the warning then continues to show.

**No warning does not mean a safe setup.** Any `oidc.audience` that differs from the client id
removes the warning. If the IdP also puts that audience into the ID token, the ID token still
passes IAM's audience check, and nothing warns. Decode a real ID token. Its `aud` must not
contain the value of `oidc.audience`.

**Per IdP. Not measured.** These lines state what each IdP offers. This chart did not measure
them.

- **Keycloak.** Add an "Audience" protocol mapper to the console client, or to a client scope of
  that client. Set "Included Custom Audience" to the API audience. Set "Add to access token" on
  and "Add to ID token" off. The mapper adds a value to `aud`. It does not replace `aud`. Keycloak
  example 2 below shows the mapper. IAM also refuses a Keycloak ID token by its `typ` claim
  (SMA-686).
- **Okta.** Use a custom authorization server whose audience is the API identifier. See the Okta
  example below.
- **Auth0.** Auth0 issues an access token for an API only when the authorization request has the
  `audience` parameter, or when the tenant has a "Default Audience".
  - Create an API. Its identifier is the audience. Enable "Allow Offline Access" on the API, or
    the console gets no refresh token.
  - Set `oidc.audience` and `oidc.authorizationAudience` to that identifier. Quote both values in
    a values file.
  - IAM needs `email` in the access token (item 2). Auth0 does not put it into an API access
    token by default. Add it with a post-login Action. Not measured.
  - The console does not send `audience` on a refresh. Auth0 keeps the original audience on a
    refresh.
  - Alternative: the tenant "Default Audience". It applies to every application of the tenant.
    With it, `oidc.authorizationAudience` can stay empty.
- **Entra ID.** An access token for an API needs a scope of that API.
  - Register a SEPARATE app registration for the API. Do not expose the API on the console's own
    registration. The v2 `aud` would then be the console's client id. That setup is the one the
    warning above is about.
  - Set its `accessTokenAcceptedVersion` to 2. IAM refuses a v1.0 access token: its `iss`
    (`https://sts.windows.net/<tenant>/`) is not the v2 issuer of the discovery document. v1.0 is
    not supported.
  - Expose one scope. Give the console's registration the delegated permission. Give consent.
  - Set `oidc.scopes` to `openid profile email offline_access` plus exactly one scope of that API,
    for example `api://paigasus-api/access`. One request can hold scopes of only one resource.
  - Set `oidc.audience` to the API registration's application (client) id, a GUID. A v2 access
    token has that value as its `aud`.
  - Add `email` as an optional claim of the access token. Not measured.
  - Before the switch, decode a real access token and check its `aud`, `iss` and `email`.
- **Dex.** Dex gives the ID token and the access token the same `aud`. No audience setting helps.
  Set `oidc.acknowledgeClientIdAudience` to remove the warning. IAM still accepts a Dex ID token
  as a bearer token. SMA-686 residual R1 stays open for Dex.

**Keycloak example 1: the kind job's setup. This setup shows the warning.** Keycloak does not put
the client id into the access token's `aud` by default. The kind job adds an audience mapper to
the client. The mapper adds the client id, so the IAM audience equals `oidc.clientId`:

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
is a complete example of this setup.

**Keycloak example 2: a dedicated API audience. Not tested in the kind job.** Add this mapper to
the console client (step 1 of the migration order). Keep the mapper of example 1 until step 4:

```json
{
  "name": "paigasus-api-audience",
  "protocol": "openid-connect",
  "protocolMapper": "oidc-audience-mapper",
  "config": {
    "included.custom.audience": "api://paigasus",
    "id.token.claim": "false",
    "access.token.claim": "true"
  }
}
```

Then set `oidc.audience=api://paigasus` (step 3). Quote the value in a values file.

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

## 9. The first platform admin and extra IAM env (SMA-697)

**Without a bootstrap admin, no user can do anything.** IAM denies every action by default. The
IAM log shows `default-deny (no matching permit)`. Only a `platform_admin` can grant a role, and
only `zones.iam.backend.bootstrapAdmins` makes the first one. At boot with an empty list, IAM
logs the warning `no authz.bootstrap_admins configured`.

```yaml
zones:
  iam:
    backend:
      bootstrapAdmins:
        - issuer: https://idp.example.com   # must equal oidc.issuer
          subject: "392488538992280259"     # the IdP "sub" claim; quote it
      extraEnv:
        - name: RUST_LOG
          value: "paigasus_iam=debug,info"
```

The chart renders the list into `IAM_AUTHZ__BOOTSTRAP_ADMINS`, in the same figment inline form as
`IAM_AUTHN__ISSUERS`. After the user logs in and IAM provisions the user, IAM grants
`platform_admin` at Root scope. IAM does this on each login until the grant exists, so a failed
grant heals at the next login.

- **The issuer must equal `oidc.issuer`.** IAM compares the `(issuer, subject)` pair as exact
  strings with the issuer of the validated token. That is always the configured issuer. An entry
  with another issuer never gets the grant, and IAM gives no signal. IAM itself does not refuse
  such an entry, so the chart refuses it.
- **Find the subject.** It is the `sub` claim of the user's access token. For Zitadel, it is the
  user ID, which is digits only. Quote it. YAML reads digits as a number, and a number with 18
  digits loses precision. The chart refuses a number. With `--set`, use `--set-string`.
- **The user must provision first.** IAM grants the role only after JIT provisioning succeeds.
  Provisioning needs an `email` claim in the access token. Without it, IAM answers
  `403 provisioning-failed` and grants nothing.
- **Removing an entry does not revoke the grant.** Revoke `platform_admin` through the IAM API.

**`extraEnv`.** The chart appends these entries after its own entries, as written. Use it for
`RUST_LOG`, which IAM reads at start (`paigasus_logging::env_filter`). The chart refuses a name
that it sets itself: `IAM_HTTP_ADDR`, `IAM_GRPC_ADDR`, `IAM_MIGRATION__LOCK_WAIT_SECS`,
`IAM_DATABASE_URL`, `IAM_AUTHN__ISSUERS`, `IAM_API_KEYS__PEPPER`,
`IAM_AUTHN__EXTRA_CA_BUNDLE_PATH` and `IAM_AUTHZ__BOOTSTRAP_ADMINS`. It also refuses a name that
starts with one of these names and `__`. Set those values through their chart values. The list is
`paigasus.iamReservedEnv` in `templates/_iam-backend.tpl`, and `tests/env.sh` row B6 keeps it
equal to the rendered names. `extraEnv` can set other `IAM_*` keys, for example
`IAM_AUTHZ__ENFORCE_TENANCY`. The chart does not check those values; IAM checks them at boot.
