# SMA-686 measurements: which claims separate an access token from an ID token

Measured on 2026-09-25 in scratch containers. No repo file changed during the measurements. The
spec is `2026-09-25-sma-686-refuse-id-token-bearer-design.md`. Scratch credentials are replaced by
`<redacted>`.

## Round 1: Keycloak 26.4, the realm fixtures

Image `quay.io/keycloak/keycloak:26.4`, `start-dev --import-realm`. Realm A is a copy of
`rs/crates/services/paigasus-iam/tests/fixtures/keycloak-realm.json`. Realm B is a copy of
`ci/kind/realm/paigasus-realm.json` with `directAccessGrantsEnabled: true` and a local redirect
URI. Cases B3 and B5 add one client attribute each.

| Case | Token | Header | Payload claim names | `typ` | `aud` | `at_hash` | `nonce` |
|---|---|---|---|---|---|---|---|
| A, password grant, `scope=openid` | access | RS256, `typ: JWT`, kid | acr aud azp email email_verified exp family_name given_name iat iss jti name preferred_username scope sid sub typ | `Bearer` | `paigasus` | no | no |
| A | ID | RS256, `typ: JWT`, kid | acr at_hash aud azp email email_verified exp family_name given_name iat iss jti name preferred_username sid sub typ | `ID` | `paigasus-cli` | yes | no |
| B, authorization-code flow, `nonce=abc123` | access | RS256, `typ: JWT`, kid | acr allowed-origins aud auth_time azp email email_verified exp family_name given_name iat iss jti name preferred_username realm_access scope sid sub typ | `Bearer` | `paigasus-console` | no | no |
| B | ID | RS256, `typ: JWT`, kid | acr at_hash aud auth_time azp email email_verified exp family_name given_name iat iss jti name nonce preferred_username sid sub typ | `ID` | `paigasus-console` | yes | `abc123` |
| B3: B + `access.token.header.type.rfc9068: true` | access | RS256, `typ: at+jwt`, kid | as B access | `Bearer` | `paigasus-console` | no | no |
| B3 | ID | RS256, `typ: JWT`, kid | as B ID (password grant, so no nonce) | `ID` | `paigasus-console` | yes | no |
| B5: B + `client.use.lightweight.access.token.enabled: true` | access | RS256, `typ: JWT`, kid | azp exp iat iss jti scope sid typ | `Bearer` | (absent) | no | no |
| B5 | ID | RS256, `typ: JWT`, kid | as B ID | `ID` | `paigasus-console` | yes | no |

No token had `c_hash`. The refresh token header is `{"alg":"HS512","typ":"JWT"}`. The userinfo
endpoint returned 200 for the case-B access token and 401 for the case-B ID token.

## Round 2: Dex, Keycloak refresh grant, Keycloak logout token

The raw evidence follows unchanged, except for redacted scratch credentials and demoted headings.

**Note on the rule named below.** This round was measured against the DRAFT rule of the spec
challenge: payload `typ` equal to `ID`, or an `at_hash` claim, or a `c_hash` claim. This round
showed that every Dex access token has `at_hash`, so that draft rule was replaced. The
implemented rule checks only the payload `typ` for `ID` or `Logout` (spec D2, D3). Where the
evidence below says a token "carries a marker" or "would be caught", read it against the draft
rule. Do not use those statements to add an `at_hash` or `c_hash` check: it refuses every Dex
access token.

### SMA-686 measurement evidence: real OIDC tokens from Dex and Keycloak

All facts below are MEASURED against locally run containers. Anything not run is
labeled "not measured". Signatures and secrets are removed from the decoded
payloads below; signature lengths and SHA-256 hashes of the signature segment
are kept as non-secret fingerprints. Raw (unscrubbed except as noted) command
output and per-token decode JSON files live alongside this file in the same
directory (`d1_*`, `d2_*`, `d3_*`, `k1_*`, `k2_*`).

### Images used (measured digests)

- Dex: `ghcr.io/dexidp/dex:v2.45.1`
  digest: `ghcr.io/dexidp/dex@sha256:8499afd690c437f52301efd2b05b2455da5bd2dfc20332cd697dc9937f808462`
  (v2.45.1 was the newest non-draft, non-prerelease tag on the dexidp/dex GitHub
  releases API at measurement time; v2.44.0 was pulled first, then replaced.)
- Keycloak: `quay.io/keycloak/keycloak:26.4`
  digest: `quay.io/keycloak/keycloak@sha256:9409c59bdfb65dbffa20b11e6f18b8abb9281d480c7ca402f51ed3d5977e6007`
  Server reported itself as "Keycloak 26.4.7" in its own startup log.

### Part 1 — Dex

### Setup (measured)

Config file `dex-config.yaml` (in this directory): `issuer: http://127.0.0.1:5556/dex`,
`storage.type: memory`, `enablePasswordDB: true`, one `staticPasswords` user
(`admin@example.com` / `admin`, using Dex's own documented example bcrypt hash
for the password "password" — a local Python 3.14 environment had no `bcrypt`
module installed, so the documented example hash was used instead of generating
a fresh one), one `staticClients` entry (`paigasus-console` /
`<redacted>`, redirect URI `http://127.0.0.1:5555/callback`), and
`oauth2: { passwordConnector: local, skipApprovalScreen: true }`.

Command:
```
docker run -d --name dex-meas -p 5556:5556 \
  -v "$(pwd)/dex-config.yaml:/etc/dex/config.yaml:ro" \
  ghcr.io/dexidp/dex:v2.45.1 dex serve /etc/dex/config.yaml
```
Readiness measured via `GET /dex/.well-known/openid-configuration` returning
HTTP 200 (took 0 extra poll tries after the container start — first attempt
succeeded).

### Case D1 — password grant

Command:
```
curl -s -X POST http://127.0.0.1:5556/dex/token \
  -u 'paigasus-console:<redacted>' \
  -d grant_type=password -d username=admin@example.com -d password=<redacted> \
  -d scope='openid email profile'
```
HTTP 200. Response had keys: `access_token, token_type, expires_in, id_token`
(no `refresh_token` — scope did not include `offline_access`).

**access_token** — IS a JWT (3 dot-separated segments, both header and payload
parse as JSON).

| Field | Value |
|---|---|
| header | `{"alg":"RS256","kid":"fdcda2350b614319e07eb03db4b497cb9766ea66"}` (no `typ` header field at all) |
| payload claim names (sorted) | `at_hash, aud, email, email_verified, exp, iat, iss, name, sub` |
| `typ` (payload) | absent |
| `aud` | `paigasus-console` (the client id) |
| `azp` | absent |
| `nonce` | absent (no nonce sent — password grant has no nonce parameter) |
| `at_hash` | **present**: `zpd9JXvozJDtWnBgaW7qlw` |
| `c_hash` | absent |
| signature | 342 b64 chars, sha256 `7465c04a2def390b853c5e766c3bdc67c583f991a3bd8c6db2cdd1fbaf6fa07c` |

**id_token** — also a JWT, same header shape (`alg: RS256`, same `kid`).

| Field | Value |
|---|---|
| payload claim names (sorted) | `at_hash, aud, email, email_verified, exp, iat, iss, name, sub` |
| `typ` | absent |
| `aud` | `paigasus-console` |
| `azp` | absent |
| `nonce` | absent |
| `at_hash` | present: `toWo68z7JxHPFG9hEnC4cw` |
| `c_hash` | absent |
| signature | 342 b64 chars, sha256 `feb6be6b74bd66cc3c81def378ad3fe6834c66d3647900f8768c2bf6fc1e1ac2` |

Note: Dex's access_token and id_token are structurally near-identical JWTs
signed with the same key — the access token itself carries `at_hash`, even
though `at_hash` is normally an ID-token-only claim that hashes the access
token. Dex is putting it in both.

### Case D2 — authorization-code flow, scripted with curl + cookie jar

Steps (all measured, all succeeded on first try — no retry needed):
1. `GET /dex/auth?client_id=paigasus-console&redirect_uri=...&response_type=code&scope=openid%20email%20profile&nonce=abc123&state=xyz789`
   with `-c cookies.txt -L` → followed two Dex-internal 302s down to the local
   login form at `/dex/auth/local/login?back=&state=<dex-internal-state>`.
   (No cookies were actually set by Dex in this memory-storage config; Dex
   threads its own internal state through the URL instead.)
2. Parsed the returned login HTML for the form `action` URL.
3. `POST` that URL with `login=admin@example.com&password=<redacted>` (`-b/-c cookies.txt`,
   no `-L`) → HTTP 303 with
   `Location: http://127.0.0.1:5555/callback?code=w2q4gxwkbcjidx3v5aw4o6tpb&state=xyz789`.
4. Extracted `code` from that Location header.
5. `POST /dex/token` with `grant_type=authorization_code`, that `code`, and
   `redirect_uri=http://127.0.0.1:5555/callback`, Basic-authed as the client.
   → HTTP 200.

**access_token**:

| Field | Value |
|---|---|
| header | `{"alg":"RS256","kid":"fdcda2350b614319e07eb03db4b497cb9766ea66"}` |
| payload claim names (sorted) | `at_hash, aud, email, email_verified, exp, iat, iss, name, nonce, sub` |
| `typ` | absent |
| `aud` | `paigasus-console` |
| `azp` | absent |
| `nonce` | **present**: `abc123` (the nonce round-tripped into the access token, not just the id_token) |
| `at_hash` | present: `JfztU3DPa_2gHVGiL7VYSg` |
| `c_hash` | absent |

**id_token**:

| Field | Value |
|---|---|
| payload claim names (sorted) | `at_hash, aud, c_hash, email, email_verified, exp, iat, iss, name, nonce, sub` |
| `typ` | absent |
| `aud` | `paigasus-console` |
| `azp` | absent |
| `nonce` | present: `abc123` |
| `at_hash` | present: `OfSqRKIpzAWUNt4_Dcsgig` |
| `c_hash` | **present**: `FoaUOInIuaRuLMbbGq7dmA` (only the id_token has c_hash, as expected — it hashes the authorization code) |

### Case D3 — refresh grant

Obtained a refresh token with a first password-grant call using
`scope='openid email profile offline_access'` (had `refresh_token` in the
response, unlike D1). Then:
```
curl -s -X POST http://127.0.0.1:5556/dex/token \
  -u 'paigasus-console:<redacted>' \
  -d grant_type=refresh_token -d refresh_token=<...> \
  -d scope='openid email profile offline_access'
```
HTTP 200, new `access_token` + `id_token` + `refresh_token` returned.

**NEW access_token** (post-refresh):

| Field | Value |
|---|---|
| header | `{"alg":"RS256","kid":"fdcda2350b614319e07eb03db4b497cb9766ea66"}` (same signing key as D1/D2) |
| payload claim names (sorted) | `at_hash, aud, email, email_verified, exp, iat, iss, name, sub` |
| `typ` | absent |
| `aud` | `paigasus-console` |
| `azp` | absent |
| `nonce` | absent (refresh grant, no nonce parameter) |
| `at_hash` | present: `siVWmSRxP_0LfG4nath7pw` |
| `c_hash` | absent |

**NEW id_token** (post-refresh): same claim set, `at_hash` present
(`qHtLXbU65s-gfvc8XeT3jA`), `c_hash` absent, `nonce` absent.

### Part 2 — Keycloak 26.4

### Setup (measured)

Copied `/Users/smaschek/dev/paigasus/paigasus-core/ci/kind/realm/paigasus-realm.json`
into this scratchpad directory. That file, as checked into the repo, has the
client secret and user password as **template placeholders**
(`__PAIGASUS_KIND_CLIENT_SECRET__`, `__PAIGASUS_KIND_USER_PASSWORD__`) — they
are substituted at deploy time by tooling outside this file, so there was no
real secret to "read". For this isolated measurement copy only, the script
`modify_realm.py` set concrete values:
- `client_secret = <redacted>`
- `user password = <redacted>`
- `directAccessGrantsEnabled: true` on client `paigasus-console` (was `false`)
- appended redirect URI `http://127.0.0.1:8090/callback` and web-origin
  `http://127.0.0.1:8090` (a URI under this measurement's control)
- `sslRequired: none` (was `external`) so plain HTTP works against the
  loopback-mapped container port

User from the file: `paigasus-kind` / `paigasus-kind@paigasus.test`, realm role
`offline_access`.

Command:
```
docker run -d --name kc-meas -p 8080:8080 \
  -e KC_BOOTSTRAP_ADMIN_USERNAME=admin -e KC_BOOTSTRAP_ADMIN_PASSWORD=<redacted> \
  -v "$(pwd)/paigasus-realm.json:/opt/keycloak/data/import/paigasus-realm.json:ro" \
  quay.io/keycloak/keycloak:26.4 start-dev --import-realm
```
Readiness measured via `GET /realms/paigasus/.well-known/openid-configuration`
returning HTTP 200 after 2 poll tries (~4s). Server log confirmed
`Realm 'paigasus' imported` and `Keycloak 26.4.7 ... started`.

### Case K1 — password grant with offline_access, then refresh

```
curl -s -X POST http://127.0.0.1:8080/realms/paigasus/protocol/openid-connect/token \
  -d grant_type=password -d client_id=paigasus-console -d client_secret=<redacted> \
  -d username=paigasus-kind -d password=<redacted> -d scope='openid offline_access'
```
HTTP 200, response had `refresh_token`. Then:
```
curl -s -X POST http://127.0.0.1:8080/realms/paigasus/protocol/openid-connect/token \
  -d grant_type=refresh_token -d client_id=paigasus-console -d client_secret=<redacted> \
  -d refresh_token=<...>
```
HTTP 200.

**REFRESHED access_token**:

| Field | Value |
|---|---|
| header | `{"alg":"RS256","kid":"A7EAB0QeVKWtPDtS5vDcI1RVVqu3sYLwSo7MLUm1t9E","typ":"JWT"}` |
| payload claim names (sorted) | `acr, allowed-origins, aud, azp, email, email_verified, exp, family_name, given_name, iat, iss, jti, name, preferred_username, realm_access, scope, sid, sub, typ` |
| `typ` (payload) | **`"Bearer"`** (present, but not "ID" — case-insensitive compare against "ID" does not match "Bearer") |
| `aud` | `paigasus-console` |
| `azp` | `paigasus-console` |
| `nonce` | absent (no nonce param was ever sent in this grant type) |
| `at_hash` | **absent** |
| `c_hash` | **absent** |

**REFRESHED id_token**:

| Field | Value |
|---|---|
| header | `{"alg":"RS256","kid":"A7EAB0QeVKWtPDtS5vDcI1RVVqu3sYLwSo7MLUm1t9E","typ":"JWT"}` |
| payload claim names (sorted) | `acr, at_hash, aud, azp, email, email_verified, exp, family_name, given_name, iat, iss, jti, name, preferred_username, sid, sub, typ` |
| `typ` (payload) | `"ID"` (matches the ID-token marker) |
| `aud` | `paigasus-console` |
| `azp` | `paigasus-console` |
| `nonce` | absent |
| `at_hash` | present: `1j6KCA3v3KZrvXxsjMwCAQ` |
| `c_hash` | absent (password grant, no authorization code to hash) |

So on Keycloak, the ID-token marker (`typ: "ID"`, `at_hash`) stays confined to
the id_token; the refreshed access_token carries neither `at_hash` nor
`c_hash` and its own `typ` is `"Bearer"`, not `"ID"`.

### Case K2 — back-channel logout token

Set on client `paigasus-console` via the Keycloak Admin REST API (equivalent
to the realm-JSON attributes requested — done via API instead of a container
restart, same net effect, on the already-running import):
`attributes.backchannel.logout.url = http://host.docker.internal:18099/bcl`,
`attributes.backchannel.logout.session.required = "true"`. `host.docker.internal`
resolved from inside the container without any extra `--add-host` flag (Docker
Desktop provides it by default) — measured via `docker exec kc-meas getent hosts host.docker.internal`.

Single bounded foreground command: started `python3 bcl_server.py &` (tiny
`http.server`-based POST receiver on port 18099), polled (max 30 × 1s) until
it answered a probe POST, did a fresh password-grant login to get a
`refresh_token`, then:
```
curl -s -X POST http://127.0.0.1:8080/realms/paigasus/protocol/openid-connect/logout \
  -d client_id=paigasus-console -d client_secret=<redacted> \
  -d refresh_token=<...>
```
→ HTTP 204. Polled (max 30 × 1s) for the server to have written a received
`logout_token` to disk — succeeded on the first check (0 extra poll tries
after the write). Killed the server (`kill $SERVER_PID; wait`) in the same
command. Total wall time for K2 was a few seconds, well inside the ~20 minute
budget.

**logout_token**:

| Field | Value |
|---|---|
| header | `{"alg":"RS256","kid":"A7EAB0QeVKWtPDtS5vDcI1RVVqu3sYLwSo7MLUm1t9E","typ":"logout+jwt"}` |
| payload claim names (sorted) | `aud, events, exp, iat, iss, jti, sid, sub, typ` |
| `exp` | **present** |
| `typ` (payload) | `"Logout"` |
| `aud` | `paigasus-console` |
| `events` | **present**: `{"http://schemas.openid.net/event/backchannel-logout": {}}` |
| `nonce` | absent |

### Summary tables

### Access tokens across all cases: JWT-ness and ID-token markers

| Case | Grant | access_token is JWT? | header typ | payload typ | at_hash | c_hash | nonce |
|---|---|---|---|---|---|---|---|
| D1 | password | yes | (none) | (none) | present | absent | absent |
| D2 | auth code | yes | (none) | (none) | present | absent | present (`abc123`) |
| D3 | refresh | yes | (none) | (none) | present | absent | absent |
| K1 (refreshed) | refresh | yes | `JWT` | `"Bearer"` | absent | absent | absent |

### id_tokens across all cases

| Case | at_hash | c_hash | payload typ |
|---|---|---|---|
| D1 id_token | present | absent | (none) |
| D2 id_token | present | present | (none) |
| D3 id_token (refreshed) | present | absent | (none) |
| K1 id_token (refreshed) | present | absent | `"ID"` |

### K2 logout_token

| Field | Value |
|---|---|
| alg | RS256 |
| header typ | `logout+jwt` |
| payload typ | `Logout` |
| exp present | yes |
| aud present | yes (`paigasus-console`) |
| events present | yes |
| nonce present | no |

### Answers to the four questions

**(a) Does the Dex access token carry at_hash / c_hash / typ? Is it a JWT
signed with RS256 or ES256, with a kid?**
Measured: the Dex access_token is a JWT in every case tried (D1 password, D2
auth-code, D3 refresh). It always carries `at_hash`. It carries `c_hash` in
none of the three cases measured (only the id_token got `c_hash`, and only in
the auth-code case, D2). It never carries a payload `typ` claim, and its
header never carries a `typ` field either. It is signed `RS256` with a `kid`
present in the header in all three cases (same `kid` across D1/D2/D3, i.e. one
signing key was in use for the whole run).

**(b) Would the Dex access token pass today's IAM checks in a default config
(aud = client id)?**
Not measured directly against IAM's own code (that would require running
IAM's check, which was out of scope here). Measured fact relevant to that
question: the SMA-686 refuse-rule is "payload `typ` == 'ID' (case-insensitive),
or `at_hash` present, or `c_hash` present". The Dex access_token has no `typ`
claim in any case measured, but it does have `at_hash` in every case measured
(D1, D2, D3). So by the stated rule, an IAM check that inspects `at_hash`
presence would flag/refuse the Dex access_token; a check that only inspected
`typ` would not catch it, since Dex never sets a payload `typ` at all. `aud`
was `paigasus-console` (the client id) in every Dex case measured, satisfying
"default config aud = client id" as stated in the question.

**(c) Does the refreshed Keycloak access token carry any marker?**
Measured: no. The K1 refreshed access_token has neither `at_hash` nor
`c_hash`, and its payload `typ` is `"Bearer"`, not `"ID"`. None of the three
SMA-686 markers are present on it.

**(d) Keycloak logout token: alg, typ header, payload typ, exp present?**
Measured: `alg = RS256`, header `typ = "logout+jwt"`, payload `typ = "Logout"`,
`exp` is present. (Payload `typ` is `"Logout"`, not `"ID"`, so the SMA-686
`typ`-marker rule as stated would not flag this token; it does not carry
`at_hash` or `c_hash` either.)

### Files in this directory (raw evidence)

- `dex-config.yaml`, `paigasus-realm.json` (modified copy), `modify_realm.py`,
  `decode_jwt.py`, `bcl_server.py` — configs/scripts used
- `d1_response.json`, `d2_response.json`, `d3_initial_response.json`,
  `d3_refresh_response.json` — raw Dex token endpoint responses (full JWTs,
  now-dead since the container was destroyed; scope for concern is low but
  these still contain full tokens, not just decoded claims)
- `d1_access_decoded.json`, `d1_id_decoded.json`, `d2_access_decoded.json`,
  `d2_id_decoded.json`, `d3_access_decoded.json`, `d3_id_decoded.json` —
  decoded header+payload+signature-fingerprint only (no live signature bytes)
- `k1_initial_response.json`, `k1_refresh_response.json`,
  `k2_login_response.json` — raw Keycloak token endpoint responses (also dead
  post-teardown)
- `k1_access_decoded.json`, `k1_id_decoded.json`,
  `k2_logout_token_decoded.json`, `logout_token_received.txt` — decoded/raw
  logout token evidence
- `client_current.json` / `client_updated.json` — the client object as read
  from and written to the Keycloak Admin API for K2 (contains the synthetic
  `<redacted>` made up for this run, not a real credential)

### Containers

Both `dex-meas` and `kc-meas` were removed with `docker rm -f` after all
measurements completed (verified via `docker ps -a` filtered on those names
returning no rows).
