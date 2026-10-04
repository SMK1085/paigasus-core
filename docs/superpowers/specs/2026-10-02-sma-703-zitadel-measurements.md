# SMA-703 Zitadel v4.15.3 token measurement

Date (UTC): 2026-10-02 19:40

Images (exact digests):
- `ghcr.io/zitadel/zitadel:v4.15.3` = `ghcr.io/zitadel/zitadel@sha256:2356d646340724b3d843c024d589314adb9b54dbd6452ebc34008b011e023eff`
- `postgres:17` = `postgres@sha256:d74eeac9a635390a49bc21bd49fccd973de707e2a53a76ac49b552b8712ec46f`
- `mcr.microsoft.com/playwright/python:v1.46.0-jammy` + `pip install playwright==1.46.0` (login driver)
- `python:3.12-slim` (M5 re-run only)

All values below are MEASURED from real tokens unless marked INFERRED. Signatures are dropped; only decoded header and payload are shown.

## Reproduction

This is a summary of a run. The scripts of the run are NOT committed: they were run stage by stage, not end to end. The run used `up.sh` (network, Postgres 17, Zitadel, first-instance machine user with PAT), `measure.py` (setup by API plus Playwright login plus token calls, run in the Playwright container with `--network container:sma703-zitadel` so `localhost:8080` is the issuer), `m5.py` (the M5 `openid` re-run), `run_measure.sh` and `down.sh`. The excerpt below leaves out some env values (`...`).

```
# up.sh (secrets are test values)
docker network create sma703-net
docker run -d --name sma703-pg --network sma703-net -e POSTGRES_PASSWORD=pg postgres:17
docker run -d --name sma703-zitadel --network sma703-net -p 8080:8080 --user 0 -v ./pat:/pat \
  -e ZITADEL_MASTERKEY=<32 chars> -e ZITADEL_DATABASE_POSTGRES_HOST=sma703-pg ... \
  -e ZITADEL_EXTERNALDOMAIN=localhost -e ZITADEL_EXTERNALPORT=8080 -e ZITADEL_EXTERNALSECURE=false -e ZITADEL_TLS_ENABLED=false \
  -e ZITADEL_FIRSTINSTANCE_PATPATH=/pat/admin.pat -e ZITADEL_FIRSTINSTANCE_ORG_MACHINE_MACHINE_USERNAME=sma703-admin \
  -e ZITADEL_FIRSTINSTANCE_ORG_MACHINE_MACHINE_NAME=sma703-admin -e ZITADEL_FIRSTINSTANCE_ORG_MACHINE_PAT_EXPIRATIONDATE=2030-01-01T00:00:00Z \
  ghcr.io/zitadel/zitadel:v4.15.3 start-from-init --masterkeyFromEnv --tlsMode disabled
```

API calls made by `measure.py` (Authorization: Bearer <PAT>):
```
POST /management/v1/projects {name}                                   -> P, P2
POST /management/v1/projects/{P}/apps/oidc {WEB, CODE, AUTHORIZATION_CODE+REFRESH_TOKEN, authMethodType NONE (PKCE), devMode, accessTokenType OIDC_TOKEN_TYPE_JWT}
PUT  /management/v1/projects/{P}/apps/{A}/oidc_config                 (M6: idTokenUserinfoAssertion, accessTokenRoleAssertion, idTokenRoleAssertion = true)
POST /v2/users/human {password, changeRequired false, email verified}
POST /management/v1/users/machine {accessTokenType ACCESS_TOKEN_TYPE_JWT}; PUT /management/v1/users/{id}/secret
GET  /oauth/v2/authorize?client_id=A&response_type=code&scope=...&code_challenge=<S256>&nonce=...&state=...   (Playwright drives login v1)
POST /oauth/v2/token grant_type=authorization_code|refresh_token|client_credentials
```

Login UI: the built-in login v1 served the flow (first hop `http://localhost:8080/ui/login/login?authRequestID=393381923327639555`). No Login v2 container was needed. v1 showed a 2-factor setup prompt after the password; the script clicks `Skip`. No MFA was configured.

Ids: P=`393381921683406851`, P2=`393381921700184067`, client_id of A=`393381921750515715`, machine client_id=`sma703-svc`, human sub=`393381921784070147`. The zitadel system project id seen in the `...:id:zitadel:aud` result is `393381906181390339`.

## Default access token type

App created without `accessTokenType`: GET returns no `accessTokenType` key (value: None). This is the protobuf zero value `OIDC_TOKEN_TYPE_BEARER`, which is Zitadel's opaque type. That mapping is INFERRED (no opaque token was minted). App A was created with `OIDC_TOKEN_TYPE_JWT` and returns JWT access tokens in every case below.

App A config after create:
```json
{
 "redirectUris": [
  "http://localhost:9999/callback"
 ],
 "responseTypes": [
  "OIDC_RESPONSE_TYPE_CODE"
 ],
 "grantTypes": [
  "OIDC_GRANT_TYPE_AUTHORIZATION_CODE",
  "OIDC_GRANT_TYPE_REFRESH_TOKEN"
 ],
 "clientId": "393381921750515715",
 "authMethodType": "OIDC_AUTH_METHOD_TYPE_NONE",
 "devMode": true,
 "accessTokenType": "OIDC_TOKEN_TYPE_JWT",
 "clockSkew": "0s",
 "allowedOrigins": [
  "http://localhost:9999"
 ]
}
```

## M1. Auth-code + PKCE, `openid profile email offline_access`

### M1 tokens

Scope requested: `openid profile email offline_access`

ID token returned: True. Refresh token returned: True.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851"
  ],
  "exp": 1791013105,
  "iat": 1790969905,
  "nbf": 1790969905,
  "client_id": "393381921750515715",
  "jti": "V2_393381935289729027-at_393381935289794563"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851"
  ],
  "exp": 1791013105,
  "iat": 1790969905,
  "auth_time": 1790969901,
  "nonce": "58e866bad23abebb",
  "amr": [
   "pwd"
  ],
  "azp": "393381921750515715",
  "client_id": "393381921750515715",
  "at_hash": "FFPzlMOE6pZPHZWKKJOObg",
  "sid": "V1_393381929921019907"
 }
}
```

## M2. Project audience scopes

### M2a: scope `urn:zitadel:iam:org:project:id:<P>:aud`

Scope requested: `openid profile email offline_access urn:zitadel:iam:org:project:id:393381921683406851:aud`

ID token returned: True. Refresh token returned: True.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851"
  ],
  "exp": 1791013110,
  "iat": 1790969910,
  "nbf": 1790969910,
  "client_id": "393381921750515715",
  "jti": "V2_393381944483643395-at_393381944483708931"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851"
  ],
  "exp": 1791013110,
  "iat": 1790969910,
  "auth_time": 1790969909,
  "nonce": "0c903d2717697499",
  "amr": [
   "pwd"
  ],
  "azp": "393381921750515715",
  "client_id": "393381921750515715",
  "at_hash": "q3PcWrNxynVwXqdL9t64Zg",
  "sid": "V1_393381941899952131"
 }
}
```

### M2b: scope `urn:zitadel:iam:org:project:id:zitadel:aud`

Scope requested: `openid profile email offline_access urn:zitadel:iam:org:project:id:zitadel:aud`

ID token returned: True. Refresh token returned: True.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381906181390339"
  ],
  "exp": 1791013116,
  "iat": 1790969916,
  "nbf": 1790969916,
  "client_id": "393381921750515715",
  "jti": "V2_393381953644068867-at_393381953644134403"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381906181390339"
  ],
  "exp": 1791013116,
  "iat": 1790969916,
  "auth_time": 1790969914,
  "nonce": "bef122af8c39ea68",
  "amr": [
   "pwd"
  ],
  "azp": "393381921750515715",
  "client_id": "393381921750515715",
  "at_hash": "aulEtUvS5miTdzZv0uWWbQ",
  "sid": "V1_393381951077089283"
 }
}
```

Result of M2b: the scope is accepted. The literal id `zitadel` resolves to the Zitadel system project, and its id `393381906181390339` is added to `aud` of BOTH tokens.

## M3. Second project P2

### M3a: scope `...:id:<P2>:aud` only

Scope requested: `openid profile email offline_access urn:zitadel:iam:org:project:id:393381921700184067:aud`

ID token returned: True. Refresh token returned: True.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381921700184067"
  ],
  "exp": 1791013121,
  "iat": 1790969921,
  "nbf": 1790969921,
  "client_id": "393381921750515715",
  "jti": "V2_393381962821140483-at_393381962821206019"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381921700184067"
  ],
  "exp": 1791013121,
  "iat": 1790969921,
  "auth_time": 1790969920,
  "nonce": "3dfd1a36ca6309fc",
  "amr": [
   "pwd"
  ],
  "azp": "393381921750515715",
  "client_id": "393381921750515715",
  "at_hash": "GeY1Iv-PWjRO9gasCkKvUA",
  "sid": "V1_393381960237449219"
 }
}
```

### M3b: scopes for P and P2 together

Scope requested: `openid profile email offline_access urn:zitadel:iam:org:project:id:393381921683406851:aud urn:zitadel:iam:org:project:id:393381921700184067:aud`

ID token returned: True. Refresh token returned: True.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381921700184067"
  ],
  "exp": 1791013127,
  "iat": 1790969927,
  "nbf": 1790969927,
  "client_id": "393381921750515715",
  "jti": "V2_393381971998277635-at_393381971998343171"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381921700184067"
  ],
  "exp": 1791013127,
  "iat": 1790969927,
  "auth_time": 1790969925,
  "nonce": "fd841b6b42c446cc",
  "amr": [
   "pwd"
  ],
  "azp": "393381921750515715",
  "client_id": "393381921750515715",
  "at_hash": "Lt8ZzdjOSFsX9hRFrg2s8A",
  "sid": "V1_393381969414586371"
 }
}
```

Result: P2's id appears in `aud` of BOTH the access token and the ID token. In every run the two `aud` arrays are identical. No scope put an audience on the access token only.

## M4. Refresh token grant

### M4a: refresh of the M1 session

Scope requested: `None`

ID token returned: True. Refresh token returned: True.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851"
  ],
  "exp": 1791013105,
  "iat": 1790969905,
  "nbf": 1790969905,
  "client_id": "393381921750515715",
  "jti": "V2_393381935289729027-at_393381935306571779"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851"
  ],
  "exp": 1791013105,
  "iat": 1790969905,
  "auth_time": 1790969901,
  "nonce": "58e866bad23abebb",
  "amr": [
   "pwd"
  ],
  "azp": "393381921750515715",
  "client_id": "393381921750515715",
  "at_hash": "9yO4kdn1jViUdIJ1kQrkjw",
  "sid": "V1_393381929921019907"
 }
}
```

### M4b: refresh of the M3a session

Scope requested: `None`

ID token returned: True. Refresh token returned: True.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381921700184067"
  ],
  "exp": 1791013121,
  "iat": 1790969921,
  "nbf": 1790969921,
  "client_id": "393381921750515715",
  "jti": "V2_393381962821140483-at_393381962821402627"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381921700184067"
  ],
  "exp": 1791013121,
  "iat": 1790969921,
  "auth_time": 1790969920,
  "nonce": "3dfd1a36ca6309fc",
  "amr": [
   "pwd"
  ],
  "azp": "393381921750515715",
  "client_id": "393381921750515715",
  "at_hash": "M2VF_UXhbKZYvnGyLryJjg",
  "sid": "V1_393381960237449219"
 }
}
```

Result: the refresh response returns an ID token and an access token. `client_id` is present on the refreshed access token and equals A's client id. The refreshed ID token keeps the original `nonce`, `auth_time`, `sid` and `iat`, with a new `at_hash`. The refreshed access token also keeps the original `iat`/`exp`/`nbf`; only `jti` changes.

## M5. Client credentials, machine user, JWT access token

First attempt with scope `openid` returned HTTP 400 `Errors.User.Machine.Secret.NotExisting` (the secret was used less than a second after it was created; the next two calls succeeded). `measure.py` now retries. The `openid` call was repeated separately with a fresh secret (`m5.py`) and gave the block below. This block is ABBREVIATED by hand: the times and some values are replaced with placeholders, and the ID token header shows no `kid`. Use M5b and M5c for the full machine-flow token shapes.

```json
{
 "scope": "openid",
 "access_token": {
  "header": {
   "alg": "RS256",
   "kid": "393381907372703747",
   "typ": "JWT"
  },
  "payload": {
   "iss": "http://localhost:8080",
   "sub": "393381990419660803",
   "aud": [
    "sma703-svc"
   ],
   "client_id": "sma703-svc",
   "jti": "<redacted>",
   "exp": "...",
   "iat": "...",
   "nbf": "..."
  }
 },
 "id_token": {
  "header": {
   "alg": "RS256",
   "typ": "JWT"
  },
  "payload": {
   "iss": "http://localhost:8080",
   "sub": "393381990419660803",
   "aud": [
    "sma703-svc"
   ],
   "auth_time": "...",
   "amr": [
    "pwd"
   ],
   "azp": "sma703-svc",
   "client_id": "sma703-svc",
   "at_hash": "<hash>"
  }
 }
}
```

### M5b: `openid` + project P aud scope

Scope requested: `openid urn:zitadel:iam:org:project:id:393381921683406851:aud`

ID token returned: True. Refresh token returned: False.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381990419660803",
  "aud": [
   "393381921683406851"
  ],
  "exp": 1791013138,
  "iat": 1790969938,
  "nbf": 1790969938,
  "client_id": "sma703-svc",
  "jti": "V2_393381990436569091-at_393381990436634627"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381990419660803",
  "aud": [
   "393381921683406851",
   "sma703-svc"
  ],
  "exp": 1791013138,
  "iat": 1790969938,
  "auth_time": 1790969938,
  "amr": [
   "pwd"
  ],
  "azp": "sma703-svc",
  "client_id": "sma703-svc",
  "at_hash": "kYHj1WCq97WeD6uAjPviug"
 }
}
```

### M5c: `openid` + project P2 aud scope

Scope requested: `openid urn:zitadel:iam:org:project:id:393381921700184067:aud`

ID token returned: True. Refresh token returned: False.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381990419660803",
  "aud": [
   "393381921700184067"
  ],
  "exp": 1791013138,
  "iat": 1790969938,
  "nbf": 1790969938,
  "client_id": "sma703-svc",
  "jti": "V2_393381990453280771-at_393381990453346307"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381990419660803",
  "aud": [
   "393381921700184067",
   "sma703-svc"
  ],
  "exp": 1791013138,
  "iat": 1790969938,
  "auth_time": 1790969938,
  "amr": [
   "pwd"
  ],
  "azp": "sma703-svc",
  "client_id": "sma703-svc",
  "at_hash": "fbv2JUj-jQPHcaxNiUsgdg"
 }
}
```

Result: `client_id` is present on the machine access token (value = the machine user's client id `sma703-svc`). An ID token IS returned for client_credentials with `openid`. It also carries `client_id`, `azp`, `amr`, `auth_time`, `at_hash`, but no `nonce`. With a project aud scope the machine access token `aud` holds only the project id; the ID token `aud` holds the project id plus the client id.

## M6. `idTokenUserinfoAssertion` and role assertion on

App config after the change:
```json
{
 "redirectUris": [
  "http://localhost:9999/callback"
 ],
 "responseTypes": [
  "OIDC_RESPONSE_TYPE_CODE"
 ],
 "grantTypes": [
  "OIDC_GRANT_TYPE_AUTHORIZATION_CODE",
  "OIDC_GRANT_TYPE_REFRESH_TOKEN"
 ],
 "clientId": "393381921750515715",
 "authMethodType": "OIDC_AUTH_METHOD_TYPE_NONE",
 "devMode": true,
 "accessTokenType": "OIDC_TOKEN_TYPE_JWT",
 "accessTokenRoleAssertion": true,
 "idTokenRoleAssertion": true,
 "idTokenUserinfoAssertion": true,
 "clockSkew": "0s",
 "allowedOrigins": [
  "http://localhost:9999"
 ]
}
```

### M6a: repeat of M1

Scope requested: `openid profile email offline_access`

ID token returned: True. Refresh token returned: True.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851"
  ],
  "exp": 1791013132,
  "iat": 1790969932,
  "nbf": 1790969932,
  "client_id": "393381921750515715",
  "jti": "V2_393381981209034755-at_393381981209100291"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851"
  ],
  "exp": 1791013132,
  "iat": 1790969932,
  "auth_time": 1790969930,
  "nonce": "1bc93cfd83c09132",
  "amr": [
   "pwd"
  ],
  "azp": "393381921750515715",
  "client_id": "393381921750515715",
  "at_hash": "SqjcW4J3auI69nS03Ae-fg",
  "sid": "V1_393381978642055171",
  "name": "Sma Seven",
  "given_name": "Sma",
  "family_name": "Seven",
  "locale": null,
  "updated_at": 1790969897,
  "preferred_username": "sma703user@example.com",
  "email": "sma703user@example.com",
  "email_verified": true
 }
}
```

### M6b: with P2 aud scope

Scope requested: `openid profile email offline_access urn:zitadel:iam:org:project:id:393381921700184067:aud`

ID token returned: True. Refresh token returned: True.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381921700184067"
  ],
  "exp": 1791013137,
  "iat": 1790969937,
  "nbf": 1790969937,
  "client_id": "393381921750515715",
  "jti": "V2_393381990369329155-at_393381990369394691"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381921700184067"
  ],
  "exp": 1791013137,
  "iat": 1790969937,
  "auth_time": 1790969936,
  "nonce": "0a9de79d9b2732c0",
  "amr": [
   "pwd"
  ],
  "azp": "393381921750515715",
  "client_id": "393381921750515715",
  "at_hash": "rdMBMkiKFdUqA3cEZd5YzA",
  "sid": "V1_393381987802415107",
  "name": "Sma Seven",
  "given_name": "Sma",
  "family_name": "Seven",
  "locale": null,
  "updated_at": 1790969897,
  "preferred_username": "sma703user@example.com",
  "email": "sma703user@example.com",
  "email_verified": true
 }
}
```

### M6c: refresh with the settings on

Scope requested: `None`

ID token returned: True. Refresh token returned: True.

Access token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381921700184067"
  ],
  "exp": 1791013137,
  "iat": 1790969937,
  "nbf": 1790969937,
  "client_id": "393381921750515715",
  "jti": "V2_393381990369329155-at_393381990369591299"
 }
}
```
ID token:
```json
{
 "header": {
  "alg": "RS256",
  "kid": "393381907372703747",
  "typ": "JWT"
 },
 "payload": {
  "iss": "http://localhost:8080",
  "sub": "393381921784070147",
  "aud": [
   "393381921700315139",
   "393381921750515715",
   "393381921683406851",
   "393381921700184067"
  ],
  "exp": 1791013137,
  "iat": 1790969937,
  "auth_time": 1790969936,
  "nonce": "0a9de79d9b2732c0",
  "amr": [
   "pwd"
  ],
  "azp": "393381921750515715",
  "client_id": "393381921750515715",
  "at_hash": "krJXtE76rloLDaUsL1s7dw",
  "sid": "V1_393381987802415107",
  "name": "Sma Seven",
  "given_name": "Sma",
  "family_name": "Seven",
  "locale": null,
  "updated_at": 1790969897,
  "preferred_username": "sma703user@example.com",
  "email": "sma703user@example.com",
  "email_verified": true
 }
}
```

Result: the only change is extra profile/email claims in the ID token (`name`, `given_name`, `family_name`, `locale`, `updated_at`, `preferred_username`, `email`, `email_verified`). Neither setting changed `client_id` presence nor the header `typ`. The test user holds no project roles, so no role claim appeared in the access token. The role claim effect is NOT measured.

## M7. Per-token claim table (all measured)

| token | header typ | payload typ | client_id | client_id == A | azp | nonce | at_hash |
|---|---|---|---|---|---|---|---|
| M1_base access_token | JWT | absent | yes | yes | no | no | no |
| M1_base id_token | JWT | absent | yes | yes | yes | yes | yes |
| M4_refresh_of_M1 access_token | JWT | absent | yes | yes | no | no | no |
| M4_refresh_of_M1 id_token | JWT | absent | yes | yes | yes | yes | yes |
| M2_aud_P access_token | JWT | absent | yes | yes | no | no | no |
| M2_aud_P id_token | JWT | absent | yes | yes | yes | yes | yes |
| M2_aud_zitadel_project access_token | JWT | absent | yes | yes | no | no | no |
| M2_aud_zitadel_project id_token | JWT | absent | yes | yes | yes | yes | yes |
| M3_aud_P2 access_token | JWT | absent | yes | yes | no | no | no |
| M3_aud_P2 id_token | JWT | absent | yes | yes | yes | yes | yes |
| M4_refresh_of_M3_aud_P2 access_token | JWT | absent | yes | yes | no | no | no |
| M4_refresh_of_M3_aud_P2 id_token | JWT | absent | yes | yes | yes | yes | yes |
| M3b_aud_P_and_P2 access_token | JWT | absent | yes | yes | no | no | no |
| M3b_aud_P_and_P2 id_token | JWT | absent | yes | yes | yes | yes | yes |
| M6_userinfo_and_roles_on access_token | JWT | absent | yes | yes | no | no | no |
| M6_userinfo_and_roles_on id_token | JWT | absent | yes | yes | yes | yes | yes |
| M6b_userinfo_and_roles_on_aud_P2 access_token | JWT | absent | yes | yes | no | no | no |
| M6b_userinfo_and_roles_on_aud_P2 id_token | JWT | absent | yes | yes | yes | yes | yes |
| M6c_refresh_with_settings_on access_token | JWT | absent | yes | yes | no | no | no |
| M6c_refresh_with_settings_on id_token | JWT | absent | yes | yes | yes | yes | yes |
| M5_cc_openid_aud_P access_token | JWT | absent | yes | n/a (machine) | no | no | no |
| M5_cc_openid_aud_P id_token | JWT | absent | yes | n/a (machine) | yes | no | yes |
| M5_cc_openid_aud_P2 access_token | JWT | absent | yes | n/a (machine) | no | no | no |
| M5_cc_openid_aud_P2 id_token | JWT | absent | yes | n/a (machine) | yes | no | yes |

Header keys are always exactly `alg=RS256`, `kid`, `typ=JWT`, for both token kinds, in every run.

## M8

- Does any ID token carry `client_id`? YES. Every ID token above has it, including the human flows and the machine flow. Value is A's client id (human) or the machine client id.
- Does any access token lack `client_id`? NO. Every JWT access token above has it.
- Consequence: `client_id` presence cannot tell an ID token from an access token on Zitadel v4.15.3. Measured distinguishers (always present on ID tokens, always absent on access tokens): `at_hash`, `azp`, `amr`, `auth_time`. `nonce` is on human-flow ID tokens only. `jti` and `nbf` are on access tokens only (INFERRED to be stable; seen in all 20+ tokens). `sid` is on human-flow ID tokens only. No header or payload `typ` separates them (header `typ=JWT` on both; no payload `typ`; no `at+jwt`).

## Not measured
- Opaque access token (not minted; the default-type mapping is inferred).
- Role claim in the access token (the test user has no project role).
- Login v2 behaviour (v1 was used).
- Other Zitadel versions.
- Whether the token `aud` has the same shape for a confidential app (A is a public PKCE client).
