# SMA-690 measurements: DPoP (RFC 9449) sender-constrained tokens on Keycloak 26.4

Measured on 2026-09-26 in scratch Docker containers. No repo file was changed. The prior
method (SMA-686, `docs/superpowers/specs/2026-09-25-sma-686-measurements.md`) was copied for
container setup and JWT decoding style. All commands ran in the foreground. No host software
was installed; every tool (`curl`, `python`, `pip`) ran inside a container. Two containers were
used: the Keycloak server (`kc690`) and, for each probe run, a throwaway `python:3.12-slim`
container on the same Docker network (`sma690-net`). Both were removed with `docker rm -f` (and
the network with `docker network rm`) at the end; verified via `docker ps -a` returning no rows.

## Image and version

- Image: `quay.io/keycloak/keycloak@sha256:9409c59bdfb65dbffa20b11e6f18b8abb9281d480c7ca402f51ed3d5977e6007`
  (`docker inspect --format '{{.RepoDigests}}'` confirmed this exact digest after pull).
- Server self-reported version, from `docker logs kc690`:
  `Keycloak 26.4.7 on JVM (powered by Quarkus 3.27.1) started in 4.434s.`

## Realm and its modifications

Copied `rs/crates/services/paigasus-iam/tests/fixtures/keycloak-realm.json` from the read-only
repo worktree into the scratch dir, unmodified in content. Realm name inside the file:
`paigasus-test`. Its one client, `paigasus-cli`, is `publicClient: true`,
`directAccessGrantsEnabled: true`, with **no `attributes` key at all** (confirmed by inspecting
the raw JSON before any change — see command below). Its one user is `alice` / `alice-password`.

**Modification 1 (mechanical, required by Keycloak, not a content change):** Keycloak's
`--import-realm` matches the import file name against the realm name inside it and refuses to
start otherwise (`File name / realm name mismatch. keycloak-realm.json, contains realm
paigasus-test. File name should be paigasus-test-realm.json`). The file was copied again under
the name `paigasus-test-realm.json` and that copy was mounted; `keycloak-realm.json` was not
used to start the server. No field inside the JSON was touched for this step.

**Modification 2 (for M3 only, live via Admin REST API, not a file edit):** After M1/M2/M4/M5/M6
were measured against the client's original (unmodified) config, an admin token was obtained
from the `master` realm (`admin` / a scratch-only bootstrap password), then
`GET /admin/realms/paigasus-test/clients?clientId=paigasus-cli` found the client's internal id,
and `PUT /admin/realms/paigasus-test/clients/{id}` added the attribute
`"dpop.bound.access.tokens": "true"` to the client's `attributes` map (HTTP 204). A follow-up
GET confirmed the attribute was persisted. No other attribute or field was changed. (The script's
own `client_attributes_before` field is a false read — a Python aliasing bug made it alias the
same dict as `client_attributes_after`, so both show `"true"`. The real "before" state — no
`dpop.bound.access.tokens` key present at all — is the one confirmed separately by parsing the
raw fixture JSON before the container ever started; see the inspection output below.)

Command used to inspect the raw fixture before any change:
```
python3 -c "
import json
d = json.load(open('keycloak-realm.json'))
for c in d['clients']:
    print(c['clientId'], c.get('directAccessGrantsEnabled'), c.get('attributes'))
"
# -> paigasus-cli True None
```

## Setup commands

```
docker network create sma690-net

docker pull quay.io/keycloak/keycloak@sha256:9409c59bdfb65dbffa20b11e6f18b8abb9281d480c7ca402f51ed3d5977e6007

docker run -d --name kc690 --network sma690-net -p 18080:8080 \
  -e KC_BOOTSTRAP_ADMIN_USERNAME=admin -e KC_BOOTSTRAP_ADMIN_PASSWORD=<redacted> \
  -v "$SCRATCH/paigasus-test-realm.json:/opt/keycloak/data/import/paigasus-test-realm.json:ro" \
  quay.io/keycloak/keycloak@sha256:9409c59bdfb65dbffa20b11e6f18b8abb9281d480c7ca402f51ed3d5977e6007 \
  start-dev --import-realm
```

Readiness was polled (bounded loop, max 30 tries, 2s apart) via a `curlimages/curl:8.10.1`
container hitting `http://kc690:8080/realms/paigasus-test/.well-known/openid-configuration`;
it returned HTTP 200 on the 3rd try (~6s). `docker logs kc690` confirmed
`Realm 'paigasus-test' imported` and the version line above.

Preliminary check: the realm's own OIDC discovery document was fetched and confirmed Keycloak
26.4.7 does support DPoP at the protocol level — it advertises
`"dpop_signing_alg_values_supported":["PS384","RS384","EdDSA","ES384","ES256","RS256","ES512","PS256","PS512","RS512"]`.
(An earlier attempt to confirm the client-attribute name `dpop.bound.access.tokens` by grepping
the server's own jars for the literal string found nothing across ~430 jars searched with
`grep -a`; that search technique is inconclusive — likely a class-file string-encoding or
`unzip -p` streaming artifact, not evidence the attribute doesn't exist — and was superseded by
the direct, positive, behavioral measurement in M3 below.)

## DPoP proof construction (`probe.py`, `probe2.py`, in the scratch directory)

Both scripts run inside a `python:3.12-slim` container on `sma690-net`:
```
docker run --rm --network sma690-net -v "$SCRATCH:/w" python:3.12-slim \
  sh -c "pip install -q pyjwt[crypto] requests cryptography && python /w/probe.py"
```
A DPoP proof is built as: generate an EC P-256 key pair
(`cryptography.hazmat.primitives.asymmetric.ec`), export the public key as a JWK
`{"kty":"EC","crv":"P-256","x":...,"y":...}`, then
`jwt.encode(payload, priv, algorithm="ES256", headers={"typ":"dpop+jwt","alg":"ES256","jwk":jwk})`
with payload `{"jti": uuid4, "htm": "POST", "htu": <token endpoint URL exactly as called>,
"iat": now}`, sent as the `DPoP:` request header. The RFC 7638 thumbprint is computed as
`base64url(sha256(json.dumps({"crv","kty","x","y"}, sorted, no whitespace)))`.

Client: `paigasus-cli` (public, no secret). User: `alice` / `alice-password`. Token endpoint:
`http://kc690:8080/realms/paigasus-test/protocol/openid-connect/token`. Grant used throughout:
`password` (direct access grant), `scope=openid` unless noted.

Execution order (for a clean, unpolluted client state): **M1, M2, M4, M5, M6 ran first, against
the client's original config; M3 ran last**, after the live Admin-API attribute change described
above. This still answers every question asked, in the stated M1–M6 numbering.

## Results table

| Case | Grant | DPoP header sent? | Client `dpop.bound.access.tokens` | HTTP status | Token endpoint `token_type` | Access token header `typ`/`alg` | Access token payload `typ` | Access token `cnf` |
|---|---|---|---|---|---|---|---|---|
| M1 | password | no | not set | 200 | `Bearer` | `JWT` / `RS256` | `Bearer` | absent |
| M2 | password | yes | not set | 200 | `DPoP` | `JWT` / `RS256` | `DPoP` | `{"jkt":"ozJQKjdjCiVszg68cz5fw8reAz8Z9P-f5bntw6eeTCc"}` |
| M3a | password | no | `true` | 400 `invalid_request` | — | — | — | — |
| M3b | password | yes | `true` | 200 | `DPoP` | `JWT` / `RS256` | `DPoP` | `{"jkt":"BhTFvI2nyDHWfxRW9CxqhzirnA0PCliDe1ANDhDaycQ"}` |
| M4 (refresh, no proof) | refresh_token | no | not set (still M2's state) | 400 `invalid_grant` | — | — | — | — |
| M4 (refresh, with proof, same key as M2) | refresh_token | yes | not set | 200 | `DPoP` | `JWT` / `RS256` | `DPoP` | `{"jkt":"ozJQKjdjCiVszg68cz5fw8reAz8Z9P-f5bntw6eeTCc"}` (same jkt as M2) |
| M5 (userinfo, M2 token as Bearer, no proof) | n/a | no | not set | 401 | n/a | n/a | n/a | n/a |
| M6 (id_token from M2) | n/a | n/a | not set | 200 (part of M2) | n/a | `JWT` / `RS256` | `ID` | absent |

mTLS-bound (RFC 8705) tokens: **not measured**.

## Full decoded payloads (redacted: none beyond signature, per instructions — signature kept
only as length + SHA-256 fingerprint)

### M1 — password grant, no DPoP header, client has no DPoP setting

Response non-token fields: `{"expires_in":300,"refresh_expires_in":1800,"token_type":"Bearer","scope":"openid profile email", ...}`

Access token:
```json
{
  "header": {"alg":"RS256","typ":"JWT","kid":"kcCuTmzB4r0niRtt1cPKjy3v-oxdqJ-cFKF8kGR2sGs"},
  "payload": {
    "exp":1790440643,"iat":1790440343,
    "jti":"onrtro:c45e4324-647c-d981-b6d1-7a4749fe854f",
    "iss":"http://kc690:8080/realms/paigasus-test",
    "aud":"paigasus","sub":"32e0ec1a-9a8d-483d-ae3b-232c77b4893e",
    "typ":"Bearer","azp":"paigasus-cli","sid":"4592db44-66fa-206e-12db-1f08d4b40845",
    "acr":"1","scope":"openid profile email","email_verified":true,
    "name":"Alice Example","preferred_username":"alice",
    "given_name":"Alice","family_name":"Example","email":"alice@example.com"
  }
}
```
No `cnf` claim. `token_type` in the response body is `"Bearer"`. Payload `typ` is `"Bearer"`.
Header `typ` is `"JWT"`.

id_token (scope included `openid`): payload `typ:"ID"`, `at_hash` present, no `cnf`.

### M2 — password grant WITH a valid DPoP proof, SAME client, no client setting changed

Response non-token fields: `{"expires_in":300, ..., "token_type":"DPoP","scope":"openid profile email"}`

Access token:
```json
{
  "header": {"alg":"RS256","typ":"JWT","kid":"kcCuTmzB4r0niRtt1cPKjy3v-oxdqJ-cFKF8kGR2sGs"},
  "payload": {
    "exp":1790440643,"iat":1790440343,
    "jti":"onrtro:c92cecc8-0a24-d350-30c0-55800f4ca723",
    "iss":"http://kc690:8080/realms/paigasus-test",
    "aud":"paigasus","sub":"32e0ec1a-9a8d-483d-ae3b-232c77b4893e",
    "typ":"DPoP","azp":"paigasus-cli","sid":"0aaf21eb-541f-8eb6-d38e-3273a14534e3",
    "acr":"1","cnf":{"jkt":"ozJQKjdjCiVszg68cz5fw8reAz8Z9P-f5bntw6eeTCc"},
    "scope":"openid profile email","email_verified":true,
    "name":"Alice Example","preferred_username":"alice",
    "given_name":"Alice","family_name":"Example","email":"alice@example.com"
  }
}
```
**The token is bound.** Payload `typ` is `"DPoP"` (not `"Bearer"` — Keycloak flips this the
moment any DPoP proof is presented, with no client-side opt-in required). Header `typ` stays
`"JWT"` (the RFC 9068 `at+jwt` header type was not in play here — not configured on this
client). Response `token_type` is `"DPoP"`. `cnf` is `{"jkt":"ozJQKjdjCiVszg68cz5fw8reAz8Z9P-f5bntw6eeTCc"}`.

The DPoP proof's public JWK used for this request:
```json
{"kty":"EC","crv":"P-256","x":"20f6K5mIbJ-5L7ksNAI9Qmnr54kFs9Wp7Gd1k0aYMns","y":"LHBQtrHGbtreWwGFCT166mZmJ-ozgjW3aFlqHiurITE"}
```
Its RFC 7638 SHA-256 thumbprint, computed independently in the same script:
`ozJQKjdjCiVszg68cz5fw8reAz8Z9P-f5bntw6eeTCc`.
**This equals `cnf.jkt` exactly.** Verified programmatically (`M2_jkt_matches_thumbprint: true`).

id_token: same claim set as M1's id_token in shape (`typ:"ID"`, `at_hash` present). See M6 below
for its `cnf`.

### M3 — same client, with `dpop.bound.access.tokens = "true"` added via Admin API

**M3a, no proof:** HTTP 400,
`{"error":"invalid_request","error_description":"DPoP proof is missing"}`.
This is a **fresh** password grant (no prior DPoP proof ever sent in this attribute-enabled
state, no pre-existing bound session) — so this is real evidence the client attribute itself
now enforces DPoP, not merely session continuity from an earlier bound token (see the M4
discussion below for why that distinction matters).

**M3b, with proof:** HTTP 200, `token_type: "DPoP"`. Access token:
```json
{
  "header": {"alg":"RS256","typ":"JWT","kid":"kcCuTmzB4r0niRtt1cPKjy3v-oxdqJ-cFKF8kGR2sGs"},
  "payload": {
    "exp":1790440680,"iat":1790440380,
    "jti":"onrtro:a15ceed4-bb2e-b22d-f81c-73243bc9df68",
    "iss":"http://kc690:8080/realms/paigasus-test",
    "aud":"paigasus","sub":"32e0ec1a-9a8d-483d-ae3b-232c77b4893e",
    "typ":"DPoP","azp":"paigasus-cli","sid":"a287d30b-83f7-b14f-8313-40bdaf904d82",
    "acr":"1","cnf":{"jkt":"BhTFvI2nyDHWfxRW9CxqhzirnA0PCliDe1ANDhDaycQ"},
    "scope":"openid profile email","email_verified":true,
    "name":"Alice Example","preferred_username":"alice",
    "given_name":"Alice","family_name":"Example","email":"alice@example.com"
  }
}
```

### M4 — refresh grant on M2's refresh token, with and without a DPoP proof

**Without proof:** HTTP 400, `{"error":"invalid_grant","error_description":"DPoP proof is missing"}`.
Note this happened *before* the M3 attribute change (client still had no
`dpop.bound.access.tokens` attribute at this point) — Keycloak enforced the proof anyway,
because the refresh token itself was already bound to a `jkt` from the M2 grant. So DPoP
binding, once established on a token pair, propagates to the refresh grant regardless of any
client-level "required" setting; the client attribute (M3) additionally enforces it from a
client's very first grant.

**With proof** (same EC key/JWK as M2 — proof of possession must reuse the bound key):
HTTP 200, `token_type: "DPoP"`. New access token payload `typ` is `"DPoP"`, and
`cnf.jkt` is `ozJQKjdjCiVszg68cz5fw8reAz8Z9P-f5bntw6eeTCc` — **the same `jkt` as M2**, i.e. the
binding survived the refresh unchanged (as expected — it is the same key doing continued proof
of possession).

### M5 — Keycloak's userinfo endpoint, M2's access token as plain `Authorization: Bearer`, no proof

HTTP 401. Empty body. `WWW-Authenticate` header:
`Bearer realm="paigasus-test", error="invalid_token", error_description="Token verification failed"`.
**Keycloak itself refuses its own DPoP-bound access token when it is presented as a plain
Bearer token with no proof.**

### M6 — id_token issued alongside the M2 (DPoP-bound) access token

Payload claim names: `acr, at_hash, aud, azp, email, email_verified, exp, family_name,
given_name, iat, iss, jti, name, preferred_username, sid, sub, typ`. **No `cnf` claim.**
The ID token itself is never sender-constrained — only the access token (and, per M4, the
refresh token) carry `cnf`.

## Answers to M1–M6 (5-line summary)

M1 (plain Bearer, no DPoP anywhere): `token_type=Bearer`, payload `typ=Bearer`, header
`typ=JWT`, no `cnf`. M2 (same client, one DPoP proof header added, no server config change):
Keycloak silently binds the token — `token_type=DPoP`, payload `typ=DPoP`, `cnf.jkt` present and
equal to the independently computed RFC 7638 thumbprint of the proof's JWK. M3
(`dpop.bound.access.tokens=true` set via the Admin API): a proof-less grant now fails outright
(400 `invalid_request`, "DPoP proof is missing"); a proofed grant succeeds the same way as M2.
M4: refreshing M2's token without a proof fails (400 `invalid_grant`, "DPoP proof is missing")
even though the client had no enforced-DPoP setting yet — the binding itself propagates to
refresh; with the same key's proof it succeeds and keeps the identical `jkt`. M5: Keycloak's own
userinfo endpoint refuses the M2 DPoP-bound access token when sent as a bare Bearer token (401,
"Token verification failed"). M6: the ID token issued alongside a DPoP-bound access token
carries no `cnf` — binding applies only to the access/refresh tokens. mTLS-bound (RFC 8705)
tokens were not measured.

## Raw evidence files (scratch only, not committed)

The measurement ran in a session scratch directory. These files stayed there and are not in the
repo. This document transcribes every value that the SMA-690 spec uses.

- `keycloak-realm.json` — the unmodified copy of the repo fixture (used only for the
  pre-inspection command above).
- `paigasus-test-realm.json` — the same content, renamed to satisfy Keycloak's import filename
  rule; this is the file actually mounted and imported.
- `probe.py` — M1, M2, M4, M5, M6 (run before the M3 attribute change).
- `probe2.py` — obtains an admin token, sets `dpop.bound.access.tokens=true` via the Admin REST
  API, then runs M3a/M3b.
- `probe_results_part1.json`, `probe_results_part2.json` — the full raw decoded output written
  by each script (superset of what's transcribed above).
- `run_probe1.sh`, `run_probe2.sh` — the exact `docker run` invocations used.
- `setup1.sh`, `wait_ready.sh` — realm-fixture inspection and the readiness-poll loop.
- `grep_dpop*.sh` — the (inconclusive/negative) jar-string search for the attribute name,
  kept for completeness; superseded by the OIDC-discovery-document check and the M3 behavioral
  result.

## Containers

`kc690` was removed with `docker rm -f kc690`; the `sma690-net` network was removed with
`docker network rm sma690-net`. Every `python:3.12-slim` and `curlimages/curl` container ran
with `--rm` and self-removed on exit. Verified via `docker ps -a` returning no rows (see command
output captured during the session).
