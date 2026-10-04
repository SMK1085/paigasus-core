# SMA-682 Keycloak SSO with online scopes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the Keycloak SSO session after a console login on the kind stack, prove it and its end at logout in journey J1, and correct the documentation about `offline_access` on Keycloak.

**Architecture:** The package default scope list does not change (D1). The kind realm moves `offline_access` to the optional client scopes, and `ci/kind/values/a.yaml` sets `oidc.scopes` to `openid profile email` (D2, D3). J1 gets two new steps that send a `prompt=none` authorization request as a browser navigation in a new context that holds only the two captured Keycloak cookies (D6), and step 5 gets the D8 assertions. Measurements on the pinned Keycloak 26.7 image come first, and their results select the text of some comments and documents.

**Tech Stack:** Keycloak 26.7 (docker, kind), curl, Playwright 1.63 (`@playwright/test`), Node `node:test`, Helm chart values, Markdown.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-682-keycloak-sso-online-scopes-design.md` (approved 2026-10-04; Q1 = no deployment, no migration, M9 informative; Q2 = runbook states (i), (ii), (iii), recommends (iii) when no other application needs SSO, (i) or (ii) when one does; Q3 unanswered, so D4 stays: the kind user keeps the `offline_access` role).

**Before Task 1:** the spec and this plan are not committed yet (`git status` shows the spec as untracked). The coordinator commits both before Task 1 starts. An implementer does not commit them.

## Global Constraints

- Keycloak image for every measurement: `quay.io/keycloak/keycloak:26.7@sha256:82a77884f3af238beab1e7afd63b5f530e1b5c0590bd7aa60b40a40463e29b2c` (`ci/kind/manifests/keycloak.yaml:30`).
- The D2 scope list, verbatim: `openid profile email`.
- The probe redirect URI, verbatim: `https://console.paigasus.test/kind-sso-probe`.
- The client id: `paigasus-console`. The Keycloak cookie names: `KEYCLOAK_IDENTITY`, `KEYCLOAK_SESSION`.
- The package default `DEFAULT_OIDC_SCOPES` (`ts/packages/paigasus-auth/src/runtime.ts:112`) does not change (D1). No code change in `@paigasus/auth` `src/` (spec § 4).
- The package e2e realm `ts/packages/paigasus-auth/tests/e2e/keycloak-realm.json` does not change (D5).
- The kind user keeps `"realmRoles": ["offline_access"]` (D4).
- The kind realm keeps the Keycloak default SSO timeouts (D9). No chart template changes.
- The seven J1 step titles, verbatim and in this order:
  1. `cold visit: /iam/orgs goes through /iam/auth/login to the IdP form`
  2. `login: the callback returns to /iam/orgs with a session`
  3. `control: the sid replays in both zones`
  4. `control: the IdP session answers prompt=none with a code`
  5. `logout: the shell form ends at the IdP and returns to /iam/ with no session cookie`
  6. `the old sid is refused by both zones`
  7. `the IdP session is dead: prompt=none returns login_required`
- The D8 message, verbatim: `D8: Keycloak answered end-session with a page, not a redirect (a confirmation page?)`.
- Never use `context.request` or any `APIRequestContext` against `console.paigasus.test` or `idp.paigasus.test` (`playwright.config.ts:42-46`). Never use `redirect_uri=/iam/auth/callback` for the probe (spec § 10).
- No file under `ts/apps/iam-console/tests/cluster/` may contain the text `.skip(`, `.fixme(`, `.fail(` or `.only(`, also not in a comment (`ci/kind/run.sh:644`, `SKIP_ERE`).
- Every new source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0` (`#` for YAML and shell). This plan makes no new source file in the repository.
- Conventional commits with a workspace scope. The allowed scopes are `rs py ts contracts ci docs deps release repo claude workspace` (`ts/packages/commitlint-config/index.cjs:42`). There is NO `ops` scope: use `docs(ci):`, `fix(ci):`, `test(ts):` and `docs(repo):` as each task says.
- A commit body must not contain a line that starts with `#NNN`, and no line of the form `token: value` (for example `M6: code`). commitlint `footer-leading-blank` refuses it. Body lines are at most 100 characters.
- Never `--no-verify`. Never `git commit --amend`. Never `git reset`. Never bare `git stash`.
- End every commit message with a blank line and then `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Before any moon, pnpm, helm, node or buf command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Do not install host software (no `brew install`, no `proto upgrade`). If a tool is missing, stop and report.
- No heredoc and no here-string in a shell command or script on this Mac (root `CLAUDE.md`: a new pipe can hold only 512 bytes). Write every file with the Write tool.
- Work only in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso` on branch `feature/sma-682-keycloak-sso-online-scopes`. Use absolute paths. Check `git branch --show-current` before each commit.

## Review Focus

1. A probe hop reaches a console zone (a wrong `redirect_uri`, or a `txn_missing` re-login through `/iam/auth/login`): `silentAuthorize` must fail with "no hop may reach a console zone", not pass on a silent re-login. Pinned in Task 3, Step 5 (the zone-hop assertion in `silentAuthorize`).
2. Keycloak 26.7 sets a different set of SSO cookies (one name missing, or one name two times): step 4 must fail at its named cookie check, not later inside `silentAuthorize`. Pinned in Task 1 (the M1 cookie-name record and its stop rule) and Task 3, Step 5 (the `toContain` and `toHaveLength` assertions of step 4).
3. The end-session request gets no response object (an aborted navigation): step 5 must fail with the D8 message, not with a matcher type error about `undefined`. Pinned in Task 3, Step 5 (`?? 0` in the D8 assertions).
4. The IdP returns a different `state`, or none: `silentAuthorize` must fail on the state check. Pinned in Task 3, Step 5 (the state assertion).
5. A journeys report from an old J1 (five titles) against the new checker: the checker must fail and name the two new titles. Pinned in Task 3, Step 1 (the new `node:test` row in `ci/kind/journeys-report.test.mjs`).

---

### Task 1: Measurements M1-M10 on Keycloak 26.7

**Files:**
- Create (scratch, OUTSIDE the repository): `$WORK/realms.py`, `$WORK/kc-lib.sh`, `$WORK/measure.sh`, `$WORK/import/*.json`, `$WORK/measurements.log`
- Modify: `docs/superpowers/specs/2026-09-27-sma-682-keycloak-sso-online-scopes-design.md` (append `## 13. Measurements`; the branch edits in Step 8)

**Interfaces:**
- Consumes: nothing.
- Produces: the § 13 record, and three results that later tasks read: the **M6 branch** (A = M6 returned a code; B = M6 returned `login_required`), the **M8 result** (code or `login_required`), and the **M10 result** (code or `login_required`). Task 3 and Task 4 select text by these three results. Task 5 runs the D8 bite only in branch A.

The method copies SMA-681 spec § 3 exactly: `docker run` of the pinned image, `start-dev --import-realm`, a scratch copy of the realm with the secrets filled and `http://localhost:9999/*` added to the client's redirect URIs and `post.logout.redirect.uris`, curl with one cookie jar per user agent, and the scope `openid profile email`. No repository file changes, except the spec in Steps 7 and 8. Differences from SMA-681, all required by this spec: the image is 26.7 (not 26.4.7); the NEW realm has the § 3.1 changes; a second realm `paigasus-old` is the UNCHANGED realm (for M5 and M9); a second client `sma682-second` exists for M10; the realm keeps the default token lifespan.

- [ ] **Step 1: Check the tools, and make the scratch directory**

Set `WORK` to a new directory inside your agent scratchpad directory (outside the repository). Run:

```bash
export WORK=<your scratchpad directory>/sma682-measure
mkdir -p "$WORK"
docker info --format '{{.ServerVersion}}' && command -v curl python3 openssl
```

Expected: a Docker server version and three paths. If Docker does not answer, STOP and report "infrastructure: Docker is not running" to the coordinator.

- [ ] **Step 2: Write the realm builder**

Write `$WORK/realms.py`:

```python
# Builds the two scratch realms for SMA-682 § 6.1 from the repository realm. Not a repository file.
import copy
import json
import os
import sys

src, out, secret, password = sys.argv[1:5]
base = json.load(open(src))
os.makedirs(out, exist_ok=True)


def fill(realm):
    client = realm["clients"][0]
    client["secret"] = secret
    client["redirectUris"].append("http://localhost:9999/*")
    client["attributes"]["post.logout.redirect.uris"] += "##http://localhost:9999/*"
    realm["users"][0]["credentials"][0]["value"] = password
    return realm


# The NEW realm: spec § 3.1 (probe URI; offline_access from default to optional) plus fill().
new = fill(copy.deepcopy(base))
console = new["clients"][0]
console["redirectUris"].insert(2, "https://console.paigasus.test/kind-sso-probe")
console["defaultClientScopes"].remove("offline_access")
console["optionalClientScopes"].append("offline_access")
# M10: a second confidential client with the same scopes, no audience mapper.
second = copy.deepcopy(console)
second["clientId"] = "sma682-second"
second["protocolMappers"] = []
new["clients"].append(second)
json.dump(new, open(os.path.join(out, "paigasus-realm.json"), "w"), indent=2)

# The UNCHANGED realm (M5, M9): only fill() and a new realm name.
old = fill(copy.deepcopy(base))
old["realm"] = "paigasus-old"
json.dump(old, open(os.path.join(out, "paigasus-old-realm.json"), "w"), indent=2)
```

Run:

```bash
python3 "$WORK/realms.py" /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso/ci/kind/realm/paigasus-realm.json "$WORK/import" sma682-client-secret sma682-user-password
chmod a+r "$WORK"/import/*.json
python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); c=r["clients"][0]; print(c["defaultClientScopes"], c["optionalClientScopes"], c["redirectUris"])' "$WORK/import/paigasus-realm.json"
```

Expected: `offline_access` only in the second list, and the probe URI in the third list.

- [ ] **Step 3: Write the curl library**

Write `$WORK/kc-lib.sh`:

```bash
# SMA-682 § 6.1 curl helpers. Not a repository file. Source it from measure.sh.
KC=http://localhost:18080
RU=http://localhost:9999/cb
PLO=http://localhost:9999/out
CS=sma682-client-secret
PW=sma682-user-password
USER_NAME=paigasus-kind

qparam() { python3 -c 'import sys,urllib.parse as u; print(u.parse_qs(u.urlsplit(sys.argv[1]).query).get(sys.argv[2], [""])[0])' "$1" "$2"; }
jwt() { python3 -c 'import sys,json,base64; p=sys.argv[1].split(".")[1]; p+="="*(-len(p)%4); print(json.loads(base64.urlsafe_b64decode(p)).get(sys.argv[2], ""))' "$1" "$2"; }
jget() { python3 -c 'import sys,json; print(json.load(open(sys.argv[1])).get(sys.argv[2], ""))' "$1" "$2"; }
cookies() { awk '!/^# / && NF >= 7 { print $6 }' "$1" | sort | tr '\n' ' '; }

# summ "<http_code> <location>": the status, whether a code is present, and the error.
summ() {
  local status=${1%% *} loc=${1#* } code
  code=$(qparam "$loc" code)
  echo "status=$status code=$([ -n "$code" ] && echo yes || echo no) error=$(qparam "$loc" error) to=${loc%%\?*}"
}

# authorize JAR REALM CLIENT SCOPE [PROMPT] -> "<http_code> <location>"; the body goes to $WORK/last.html
authorize() {
  local args=(-s -o "$WORK/last.html" -w '%{http_code} %{redirect_url}' -c "$1" -b "$1"
    -G "$KC/realms/$2/protocol/openid-connect/auth"
    --data-urlencode "client_id=$3" --data-urlencode response_type=code --data-urlencode "scope=$4"
    --data-urlencode "redirect_uri=$RU" --data-urlencode "state=$(openssl rand -hex 8)")
  if [ -n "${5:-}" ]; then args+=(--data-urlencode "prompt=$5"); fi
  curl "${args[@]}"
}

# login JAR REALM CLIENT SCOPE -> the authorization code. A live SSO session answers 302 with no form.
login() {
  local out status loc action
  out=$(authorize "$1" "$2" "$3" "$4")
  status=${out%% *}
  loc=${out#* }
  if [ "$status" = 302 ]; then qparam "$loc" code; return; fi
  if [ "$status" != 200 ]; then echo "login: authorize answered $out" >&2; return 2; fi
  action=$(grep -o -m1 'action="[^"]*login-actions/authenticate[^"]*"' "$WORK/last.html" | sed -e 's/^action="//' -e 's/"$//' -e 's/&amp;/\&/g')
  if [ -z "$action" ]; then echo "login: no login form in the answer" >&2; return 2; fi
  loc=$(curl -s -o /dev/null -w '%{redirect_url}' -c "$1" -b "$1" \
    --data-urlencode "username=$USER_NAME" --data-urlencode "password=$PW" --data-urlencode credentialId= "$action")
  qparam "$loc" code
}

# token REALM CLIENT CODE OUTFILE -> http status; the body goes to OUTFILE
token() {
  curl -s -o "$4" -w '%{http_code}' -d grant_type=authorization_code -d "client_id=$2" -d "client_secret=$CS" \
    --data-urlencode "code=$3" --data-urlencode "redirect_uri=$RU" "$KC/realms/$1/protocol/openid-connect/token"
}

# refresh REALM CLIENT REFRESH_TOKEN SCOPE OUTFILE -> http status
refresh() {
  curl -s -o "$5" -w '%{http_code}' -d grant_type=refresh_token -d "client_id=$2" -d "client_secret=$CS" \
    --data-urlencode "refresh_token=$3" --data-urlencode "scope=$4" "$KC/realms/$1/protocol/openid-connect/token"
}

# revoke REALM CLIENT TOKEN -> http status. RFC 7009 with no token_type_hint, as openid-client's
# tokenRevocation sends it from bestEffortRevoke (src/adapters/oidc.ts:479-486).
revoke() {
  curl -s -o /dev/null -w '%{http_code}' -d "client_id=$2" -d "client_secret=$CS" \
    --data-urlencode "token=$3" "$KC/realms/$1/protocol/openid-connect/revoke"
}

# endsession JAR REALM CLIENT [ID_TOKEN_HINT] -> "<http_code> <location>"
endsession() {
  local args=(-s -o "$WORK/last.html" -w '%{http_code} %{redirect_url}' -c "$1" -b "$1"
    -G "$KC/realms/$2/protocol/openid-connect/logout"
    --data-urlencode "client_id=$3" --data-urlencode "post_logout_redirect_uri=$PLO"
    --data-urlencode "state=$(openssl rand -hex 8)")
  if [ -n "${4:-}" ]; then args+=(--data-urlencode "id_token_hint=$4"); fi
  curl "${args[@]}"
}

newjar() { : >"$WORK/$1.txt"; echo "$WORK/$1.txt"; }
```

- [ ] **Step 4: Write the measurement script**

Write `$WORK/measure.sh`. The row order matters: M9 changes the realm `paigasus-old`, so it runs after M5 and last.

```bash
# SMA-682 § 6.1 measurements. Not a repository file. Run: /bin/bash "$WORK/measure.sh"
set -euo pipefail
: "${WORK:?set WORK}"
. "$WORK/kc-lib.sh"
IMAGE='quay.io/keycloak/keycloak:26.7@sha256:82a77884f3af238beab1e7afd63b5f530e1b5c0590bd7aa60b40a40463e29b2c'
NEW=paigasus
OLD=paigasus-old
C=paigasus-console
S='openid profile email'

docker rm -f sma682-kc >/dev/null 2>&1 || true
# The import directory is mounted read-only, as the kind job mounts the realm ConfigMap.
docker run -d --name sma682-kc -p 127.0.0.1:18080:8080 \
  -e KC_BOOTSTRAP_ADMIN_USERNAME=admin -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin \
  -v "$WORK/import:/opt/keycloak/data/import:ro" \
  "$IMAGE" start-dev --import-realm >/dev/null
ready=no
for _ in $(seq 1 120); do
  if curl -sf "$KC/realms/$NEW/.well-known/openid-configuration" >/dev/null \
    && curl -sf "$KC/realms/$OLD/.well-known/openid-configuration" >/dev/null; then ready=yes; break; fi
  sleep 2
done
[ "$ready" = yes ] || { echo "INFRA: Keycloak did not start"; docker logs sma682-kc | tail -n 40; exit 2; }
echo "Keycloak: $(docker exec sma682-kc /opt/keycloak/bin/kc.sh --version 2>/dev/null | grep -m1 -i keycloak || echo unknown)"

echo "== M1: code exchange, new realm, scope '$S'"
J1=$(newjar j1)
code=$(login "$J1" $NEW $C "$S")
echo "M1 token status=$(token $NEW $C "$code" "$WORK/m1.json")"
RT1=$(jget "$WORK/m1.json" refresh_token)
echo "M1 refresh typ=$(jwt "$RT1" typ) granted scope='$(jget "$WORK/m1.json" scope)'"
echo "M1 cookies in jar: $(cookies "$J1")"

echo "== M2: prompt=none right after M1, same jar"
echo "M2 $(summ "$(authorize "$J1" $NEW $C openid none)")"

echo "== M3: refresh with scope '$S'"
echo "M3 refresh status=$(refresh $NEW $C "$RT1" "$S" "$WORK/m3.json")"
echo "M3 body error='$(jget "$WORK/m3.json" error)' new typ=$(jwt "$(jget "$WORK/m3.json" refresh_token)" typ) id_token=$([ -n "$(jget "$WORK/m3.json" id_token)" ] && echo yes || echo no)"

echo "== M4: logout with the hint and no revoke, then M2 again"
HINT=$(jget "$WORK/m3.json" id_token)
[ -n "$HINT" ] || HINT=$(jget "$WORK/m1.json" id_token)
echo "M4 end-session $(summ "$(endsession "$J1" $NEW $C "$HINT")")"
echo "M4 then prompt=none $(summ "$(authorize "$J1" $NEW $C openid none)")"

echo "== M5: control, UNCHANGED realm, old default scope"
J5=$(newjar j5)
code=$(login "$J5" $OLD $C "$S offline_access")
echo "M5 token status=$(token $OLD $C "$code" "$WORK/m5.json") typ=$(jwt "$(jget "$WORK/m5.json" refresh_token)" typ)"
echo "M5 cookies in jar after the exchange: $(cookies "$J5")"
echo "M5 prompt=none $(summ "$(authorize "$J5" $OLD $C openid none)")"

echo "== M6: new login, revoke the online refresh token, then prompt=none in the same jar"
J6=$(newjar j6)
code=$(login "$J6" $NEW $C "$S")
echo "M6 token status=$(token $NEW $C "$code" "$WORK/m6.json") typ=$(jwt "$(jget "$WORK/m6.json" refresh_token)" typ)"
echo "M6 revoke status=$(revoke $NEW $C "$(jget "$WORK/m6.json" refresh_token)")"
echo "M6 prompt=none $(summ "$(authorize "$J6" $NEW $C openid none)")"

echo "== M7: after M6, end-session with NO hint, same jar"
echo "M7 end-session $(summ "$(endsession "$J6" $NEW $C)")"
echo "M7 body holds a logout confirmation: $(grep -c -i 'log out' "$WORK/last.html" || true) line(s)"

echo "== M8: two jars, two logins; revoke jar 1's refresh token; prompt=none in jar 2"
J8A=$(newjar j8a)
J8B=$(newjar j8b)
code=$(login "$J8A" $NEW $C "$S"); token $NEW $C "$code" "$WORK/m8a.json" >/dev/null
code=$(login "$J8B" $NEW $C "$S"); token $NEW $C "$code" "$WORK/m8b.json" >/dev/null
echo "M8 revoke jar 1 status=$(revoke $NEW $C "$(jget "$WORK/m8a.json" refresh_token)")"
echo "M8 prompt=none jar 2 $(summ "$(authorize "$J8B" $NEW $C openid none)")"
echo "M8 prompt=none jar 1 $(summ "$(authorize "$J8A" $NEW $C openid none)")"

echo "== M10: second client (online) first, then the console with offline_access, same jar"
J10=$(newjar j10)
code=$(login "$J10" $NEW sma682-second "$S")
echo "M10 second client token status=$(token $NEW sma682-second "$code" "$WORK/m10a.json") typ=$(jwt "$(jget "$WORK/m10a.json" refresh_token)" typ)"
code=$(login "$J10" $NEW $C "$S offline_access")
echo "M10 console token status=$(token $NEW $C "$code" "$WORK/m10b.json") typ=$(jwt "$(jget "$WORK/m10b.json" refresh_token)" typ)"
echo "M10 prompt=none for the second client $(summ "$(authorize "$J10" $NEW sma682-second openid none)")"

echo "== M9: unchanged realm, offline token, then offline_access moved to optional by the admin API"
J9=$(newjar j9)
code=$(login "$J9" $OLD $C "$S offline_access")
echo "M9 token status=$(token $OLD $C "$code" "$WORK/m9.json") typ=$(jwt "$(jget "$WORK/m9.json" refresh_token)" typ)"
curl -s -o "$WORK/adm.json" -d grant_type=password -d client_id=admin-cli -d username=admin -d password=admin \
  "$KC/realms/master/protocol/openid-connect/token"
ADM=$(jget "$WORK/adm.json" access_token)
curl -s -o "$WORK/clients.json" -H "Authorization: Bearer $ADM" "$KC/admin/realms/$OLD/clients?clientId=$C"
CID=$(python3 -c 'import sys,json; print(json.load(open(sys.argv[1]))[0]["id"])' "$WORK/clients.json")
curl -s -o "$WORK/scopes.json" -H "Authorization: Bearer $ADM" "$KC/admin/realms/$OLD/client-scopes"
SID=$(python3 -c 'import sys,json; print([s["id"] for s in json.load(open(sys.argv[1])) if s["name"] == "offline_access"][0])' "$WORK/scopes.json")
echo "M9 remove default status=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE -H "Authorization: Bearer $ADM" "$KC/admin/realms/$OLD/clients/$CID/default-client-scopes/$SID")"
echo "M9 add optional status=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "Authorization: Bearer $ADM" "$KC/admin/realms/$OLD/clients/$CID/optional-client-scopes/$SID")"
echo "M9 refresh status=$(refresh $OLD $C "$(jget "$WORK/m9.json" refresh_token)" "$S" "$WORK/m9r.json")"
echo "M9 body error='$(jget "$WORK/m9r.json" error)' description='$(jget "$WORK/m9r.json" error_description)' new typ=$(jwt "$(jget "$WORK/m9r.json" refresh_token)" typ)"

docker rm -f sma682-kc >/dev/null
echo "done"
```

- [ ] **Step 5: Run the measurements**

Run with the system bash (no here-document pipe risk):

```bash
/bin/bash "$WORK/measure.sh" 2>&1 | tee "$WORK/measurements.log"
```

Expected: one line per row and `done`. If a line starts with `INFRA:`, or `login:` prints an error, the method failed, not Keycloak. Fix the script and run it again. Never record a method failure as a result.

- [ ] **Step 6: Apply the stop rules. Do not continue past a STOP.**

Read `$WORK/measurements.log`. Each STOP means: write the § 13 section with the rows you have (Step 7), commit it (Step 9), and report to the coordinator with the log lines. Do not start Task 2.

- **M5** does not show `code=no error=login_required` → STOP. The defect can be gone in 26.7 (spec § 6.1).
- **M3** status is not `200` → STOP. The D2 refresh `scope` needs a new decision (spec § 3.2).
- **M2** does not show `status=302 code=yes` → STOP. AC 1 has no basis on 26.7. (This rule is not in the spec. It follows from AC 1. The coordinator decides.)
- **M1 cookies** do not hold both `KEYCLOAK_IDENTITY` and `KEYCLOAK_SESSION` → STOP. Step 4 of J1 checks exactly these two names (spec § 3.3). (This rule is not in the spec. The coordinator decides the cookie list.)
- **M1** typ is not `Refresh` → STOP. D2 does not give an online token on 26.7.
- **M8** shows `error=login_required` in jar 2 → NOT a stop. Record it. It selects the R6 and runbook text below.

- [ ] **Step 7: Write `## 13. Measurements` in the spec**

Append this section at the end of the spec, after § 12. Fill each `Result` cell with the values that the log printed for that row (status, `code=yes|no`, `error=…`, `typ`). Copy the Keycloak version line from the log.

```markdown
## 13. Measurements

Measured on 2026-10-DD (the run date) by Task 1 of the plan, with
`quay.io/keycloak/keycloak:26.7@sha256:82a77884f3af238beab1e7afd63b5f530e1b5c0590bd7aa60b40a40463e29b2c`
(version line of the run: `<copy it>`), `start-dev --import-realm`, run locally with `docker run`.
The method is the method of the SMA-681 spec § 3: two scratch realms, curl, one cookie jar per
user agent, scope `openid profile email`, the realm default token lifespan. No repository file
changed. The realm `paigasus` is the § 3.1 realm, with the secrets filled, `http://localhost:9999/*`
added to the redirect URIs and `post.logout.redirect.uris`, and a second confidential client
`sma682-second` (the same scopes, no audience mapper) for M10. The realm `paigasus-old` is the
unchanged `ci/kind/realm/paigasus-realm.json` with the same three additions, and no second client.
The revoke is RFC 7009 with no `token_type_hint`, as `bestEffortRevoke` sends it.

| Row | Request | Result | Decides |
|---|---|---|---|
| M1 | The code exchange | <status, typ, granted scope> | D2 |
| M1 cookies | The jar after the login | <the cookie names> | step 4 cookie names |
| M2 | `prompt=none` right after M1, the same jar | <status, code, error> | AC 1 |
| M3 | Refresh with `scope=openid profile email` | <status, typ, id_token yes/no> | § 3.2 |
| M4 | Logout with the hint and no revoke, then M2 again | <end-session status; then status, code, error> | step 7 |
| M5 | Unchanged realm, old default scope, then M2 | <typ; cookies; status, code, error> | the defect on 26.7 |
| M6 | New login, revoke, then M2 in the same jar | <revoke status; status, code, error> | § 3.3.1 |
| M7 | After M6, end-session with no hint | <status; confirmation lines> | § 3.3.1 cross-check |
| M8 | Two jars, revoke jar 1, M2 in jar 2 (and jar 1) | <revoke status; jar 2; jar 1> | R6 |
| M9 | Unchanged realm, offline token, scope moved by the admin API, refresh | <admin statuses; refresh status, error, typ> | § 3.6 (informative, Q1) |
| M10 | Second client first, console with `offline_access` second, then M2 for the second client | <typs; status, code, error> | R2, approach B |
| M11 | Probe URI on the kind stack | recorded from the PR's `chart` run, see below | D6 |

Branch of § 3.3.1: <A: M6 returned a code | B: M6 returned `login_required`>. M7 agrees:
<yes/no, and why>.

M11 is covered by the `chart` workflow (`kind` job) on the pull request, not locally. J1 writes one
`prompt=none probe` annotation per probe, and `journeys-report.mjs` prints each annotation in the
job log of the step "Specs, journeys" on a green run. Plan Task 5 copies the two lines here.
```

Write `M1` and `M1 cookies` as two rows as shown. If a cell has nothing to record (for example jar 1 in M8), write `not measured`.

- [ ] **Step 8: Apply the branch edits to the spec**

Use the M6, M8 and M10 results. Make exactly these edits with the Edit tool.

**Branch A (M6 returned a code):**
- § 2, row D8, last cell: append ` M6 returned a code (§ 13): D8 is the end-to-end proof of SMA-681 AC 1 with a live SSO session.`
- § 5 AC 2: replace `If M6 shows that the revoke ends the SSO session, this AC says "the console logout sequence ends the IdP session" (§ 3.3.1).` with `M6 returned a code (§ 13), so step 7 proves that end-session ends the IdP session.`
- § 5 AC 3: replace `Whether this also proves SMA-681 AC 1 with a live session depends on M6 (§ 3.3.1).` with `M6 returned a code (§ 13), so this also proves SMA-681 AC 1 with a live session.`
- § 8 R7: replace `R7 (only if M6 returns \`login_required\`):` with `R7 (does not apply: M6 returned a code, § 13):`

**Branch B (M6 returned `login_required`):**
- § 2, row D8, last cell: append ` M6 returned \`login_required\` (§ 13): D8 is "no confirmation page on the console's real logout sequence". It is not a proof of SMA-681 AC 1 with a live session.`
- § 5 AC 2: replace the same sentence as in branch A with `M6 showed that the revoke ends the SSO session (§ 13), so this AC is: the console logout sequence ends the IdP session (§ 3.3.1).`
- § 5 AC 3: replace the same sentence as in branch A with `M6 returned \`login_required\` (§ 13), so this does not also prove SMA-681 AC 1 with a live session.`
- § 6.2, the D8 bullet: append ` Recorded: M6 returned \`login_required\` (§ 13), so the D8 mutation cannot bite (§ 3.3.1). It is not run.`
- § 8 R7: replace `R7 (only if M6 returns \`login_required\`):` with `R7 (M6 returned \`login_required\`, § 13):`

**M8 (both branches):** append to § 8 R6 either ` Measured (§ 13, M8): yes. A revoke in jar 1 ended the SSO session of jar 2.` (M8 jar 2 `login_required`) or ` Measured (§ 13, M8): no. Jar 2 kept its SSO session.` (M8 jar 2 code).

**M10 (both branches):** append to § 8 R2 either ` M10 (§ 13): a console login with \`offline_access\` ended the SSO session of the second client in the same jar.` (`login_required`) or ` M10 (§ 13): the SSO session of the second client in the same jar survived a console login with \`offline_access\`.` (code).

**M9:** append to § 3.6 ` M9 (§ 13): the refresh returned <status>.` with the measured status and, if not 200, the `error`.

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso
git branch --show-current
git add docs/superpowers/specs/2026-09-27-sma-682-keycloak-sso-online-scopes-design.md
git commit -m "docs(ci): record the SMA-682 Keycloak 26.7 measurements" -m "Rows M1 to M10 of spec section 6.1, measured with docker and curl on the pinned
Keycloak 26.7 image. The M6 result selects the section 3.3.1 branch.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Report to the coordinator: the branch (A or B), the M8 result, the M10 result, and the M9 status. Each later task needs them.

---

### Task 2: The kind realm, the kind values and the Keycloak manifest comment

**Files:**
- Modify: `ci/kind/realm/paigasus-realm.json:17-20` (redirect URIs) and `:54-55` (client scopes)
- Modify: `ci/kind/values/a.yaml:39-44` (the `oidc:` block)
- Modify: `ci/kind/manifests/keycloak.yaml:10-12` (header comment)

**Interfaces:**
- Consumes: Task 1 passed its stop rules (M3 = 200, M5 reproduced).
- Produces: a kind stack where the console requests `openid profile email`, Keycloak grants `offline_access` only on request, and `https://console.paigasus.test/kind-sso-probe` is a registered redirect URI. Task 3's J1 steps depend on all three.

- [ ] **Step 1: Prove that the render has no scope today (red)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso
helm template paigasus charts/paigasus -f ci/kind/values/a.yaml --kube-version 1.31.0 | grep -c 'PAIGASUS_OIDC_SCOPES'
```

Expected: `0` (and grep exit 1). The kind render has no scope key today.

- [ ] **Step 2: Change the realm**

In `ci/kind/realm/paigasus-realm.json`, replace lines 17-20:

```json
      "redirectUris": [
        "https://console.paigasus.test/iam/auth/callback",
        "https://console.paigasus.test/gateway/auth/callback"
      ],
```

with:

```json
      "redirectUris": [
        "https://console.paigasus.test/iam/auth/callback",
        "https://console.paigasus.test/gateway/auth/callback",
        "https://console.paigasus.test/kind-sso-probe"
      ],
```

Replace lines 54-55:

```json
      "defaultClientScopes": ["basic", "web-origins", "acr", "profile", "roles", "email", "offline_access"],
      "optionalClientScopes": ["address", "phone", "microprofile-jwt"]
```

with:

```json
      "defaultClientScopes": ["basic", "web-origins", "acr", "profile", "roles", "email"],
      "optionalClientScopes": ["address", "phone", "microprofile-jwt", "offline_access"]
```

Do not change line 66 (`"realmRoles": ["offline_access"]`, D4). Do not add a comment field: Keycloak refuses an unknown field.

- [ ] **Step 3: Change the kind values**

In `ci/kind/values/a.yaml`, replace lines 39-44:

```yaml
oidc:
  issuer: https://idp.paigasus.test/realms/paigasus
  clientId: paigasus-console
  existingSecret: paigasus-oidc
  caBundle:
    existingConfigMap: paigasus-idp-ca
```

with:

```yaml
oidc:
  issuer: https://idp.paigasus.test/realms/paigasus
  clientId: paigasus-console
  # The kind realm is the Keycloak example of the scope setup in RUNBOOK-chart.md § 6 (Keycloak
  # example 1). No offline_access: Keycloak then issues an online refresh token and keeps the SSO
  # session, which J1 steps 4 and 7 check (SMA-682). The realm grants offline_access only as an
  # OPTIONAL client scope (ci/kind/realm/paigasus-realm.json). The realm keeps the Keycloak default
  # SSO timeouts, so it is not an example of a session-length setup.
  scopes: "openid profile email"
  existingSecret: paigasus-oidc
  caBundle:
    existingConfigMap: paigasus-idp-ca
```

- [ ] **Step 4: Change the Keycloak manifest comment**

In `ci/kind/manifests/keycloak.yaml`, replace lines 10-12:

```yaml
# /server/reverseproxy, /server/importExport, /observability/health. 26.4 is the tag the
# paigasus-auth e2e uses. Refresh with:
#   docker buildx imagetools inspect quay.io/keycloak/keycloak:26.4 --format '{{json .Manifest.Digest}}'
```

with:

```yaml
# /server/reverseproxy, /server/importExport, /observability/health. The kind job pins 26.7 (the
# image below). The paigasus-auth e2e pins 26.4 (ts/packages/paigasus-auth/tests/e2e/global-setup.ts).
# SMA-682 measured the kind realm on this 26.7 digest. Refresh with:
#   docker buildx imagetools inspect quay.io/keycloak/keycloak:26.7 --format '{{json .Manifest.Digest}}'
```

- [ ] **Step 5: Verify (green)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso
node -e 'const r=JSON.parse(require("fs").readFileSync("ci/kind/realm/paigasus-realm.json","utf8")); const c=r.clients[0]; if (c.defaultClientScopes.includes("offline_access") || !c.optionalClientScopes.includes("offline_access") || !c.redirectUris.includes("https://console.paigasus.test/kind-sso-probe") || r.users[0].realmRoles[0] !== "offline_access") { console.error("realm check FAILED"); process.exit(1); } console.log("realm check ok");'
helm template paigasus charts/paigasus -f ci/kind/values/a.yaml --kube-version 1.31.0 | grep 'PAIGASUS_OIDC_SCOPES'
moon run repo:helm-render --force
```

Expected: `realm check ok`; one line `  PAIGASUS_OIDC_SCOPES: "openid profile email"` (the one `console-env` ConfigMap that both consoles read, `charts/paigasus/templates/console-env-configmap.yaml:16-20`); `repo:helm-render` passes (row 7 renders `a.yaml` and `a.yaml` + `b.yaml`).

- [ ] **Step 6: Commit**

```bash
git branch --show-current
git add ci/kind/realm/paigasus-realm.json ci/kind/values/a.yaml ci/kind/manifests/keycloak.yaml
git commit -m "fix(ci): request online scopes on the kind stack so Keycloak keeps SSO (SMA-682)" -m "The kind realm grants offline_access only as an optional client scope and registers
the J1 probe redirect URI. a.yaml sets oidc.scopes to openid profile email.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: J1 steps 4 and 7, the D8 assertions, and the report checker

The checker and the journey change in ONE task. `ci/kind/journeys-report.test.mjs:204-207` compares the real journey file with `EXPECTED_STEPS`, so a commit with only one of the two leaves that test red (`ci/kind/README.md`: "Change a step title in the spec file and in `EXPECTED_STEPS` in the same commit").

**Files:**
- Modify: `ci/kind/journeys-report.mjs:37-44` (`EXPECTED_STEPS` for J1)
- Modify: `ci/kind/journeys-report.test.mjs:86-88` (early-return row) and add one row after it
- Modify: `ci/kind/fixtures/journeys-report/pass.json:38-44`, `ci/kind/fixtures/journeys-report/skipped.json:52-73`
- Modify: `ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts` (whole file, 192 lines today)
- Do not change: `ci/kind/fixtures/journeys-report/early-return.json`, `fail-expected.json`, `journeys-report.test.mjs:91` (spec § 3.4)

**Interfaces:**
- Consumes: Task 1's M6 branch (A or B) for two comment blocks; Task 2's realm, values and probe URI.
- Produces: `EXPECTED_STEPS['auth-roundtrip.spec.ts']` = the seven titles of the Global Constraints; in the journey file: `const PROBE_REDIRECT: string`, `const KEYCLOAK_COOKIES: readonly ['KEYCLOAK_IDENTITY', 'KEYCLOAK_SESSION']`, `async function silentAuthorize(browser: Browser, authorizationEndpoint: URL, idpCookies: readonly Cookie[]): Promise<URL>`, and the annotation type `prompt=none probe` that Task 5 reads for M11.

- [ ] **Step 1: Change the checker and its test first**

In `ci/kind/journeys-report.mjs`, replace lines 38-44:

```js
  'auth-roundtrip.spec.ts': Object.freeze([
    'cold visit: /iam/orgs goes through /iam/auth/login to the IdP form',
    'login: the callback returns to /iam/orgs with a session',
    'control: the sid replays in both zones',
    'logout: the shell form ends at the IdP and returns to /iam/ with no session cookie',
    'the old sid is refused by both zones',
  ]),
```

with:

```js
  'auth-roundtrip.spec.ts': Object.freeze([
    'cold visit: /iam/orgs goes through /iam/auth/login to the IdP form',
    'login: the callback returns to /iam/orgs with a session',
    'control: the sid replays in both zones',
    'control: the IdP session answers prompt=none with a code',
    'logout: the shell form ends at the IdP and returns to /iam/ with no session cookie',
    'the old sid is refused by both zones',
    'the IdP session is dead: prompt=none returns login_required',
  ]),
```

In `ci/kind/journeys-report.test.mjs`, replace lines 86-88:

```js
test('early-return.json: every counter passes, the missing steps do not (now after step 2 of 5)', () => {
  expectFail(join(FIXTURES, 'early-return.json'), '3 step(s) never ran');
});
```

with:

```js
test('early-return.json: every counter passes, the missing steps do not (now after step 2 of 7)', () => {
  expectFail(join(FIXTURES, 'early-return.json'), '5 step(s) never ran');
});

test('a J1 report without the two SSO steps fails and names them (SMA-682)', () => {
  expectFail(
    variant((d) => {
      const result = d.suites[0].specs[0].tests[0].results[0];
      result.steps = result.steps.filter((step) => !step.title.includes('prompt=none'));
    }),
    '2 step(s) never ran: "control: the IdP session answers prompt=none with a code", "the IdP session is dead: prompt=none returns login_required"',
  );
});
```

- [ ] **Step 2: Run the checker tests (red)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso
node --test ci/kind/journeys-report.test.mjs
```

Expected: FAIL. These four rows fail, because `pass.json` and the real journey file still hold five J1 steps: `pass.json passes`, `pass.json prints its annotation after the pass summary (controller ruling F3)`, `a test inside a describe block is still found`, `the real journeys spec files hold exactly the expected step titles`. Any other failing row means a wrong edit: stop and check Step 1.

- [ ] **Step 3: Change the two fixtures**

In `ci/kind/fixtures/journeys-report/pass.json`, replace lines 39-43 (the five J1 step lines) with:

```json
                    { "title": "cold visit: /iam/orgs goes through /iam/auth/login to the IdP form", "duration": 1000 },
                    { "title": "login: the callback returns to /iam/orgs with a session", "duration": 1000 },
                    { "title": "control: the sid replays in both zones", "duration": 1000 },
                    { "title": "control: the IdP session answers prompt=none with a code", "duration": 1000 },
                    { "title": "logout: the shell form ends at the IdP and returns to /iam/ with no session cookie", "duration": 1000 },
                    { "title": "the old sid is refused by both zones", "duration": 1000 },
                    { "title": "the IdP session is dead: prompt=none returns login_required", "duration": 1000 }
```

In `ci/kind/fixtures/journeys-report/skipped.json`, replace lines 53-72 (the five J1 step objects inside `"steps": [ … ]`) with:

```json
                    {
                      "title": "cold visit: /iam/orgs goes through /iam/auth/login to the IdP form",
                      "duration": 1000
                    },
                    {
                      "title": "login: the callback returns to /iam/orgs with a session",
                      "duration": 1000
                    },
                    {
                      "title": "control: the sid replays in both zones",
                      "duration": 1000
                    },
                    {
                      "title": "control: the IdP session answers prompt=none with a code",
                      "duration": 1000
                    },
                    {
                      "title": "logout: the shell form ends at the IdP and returns to /iam/ with no session cookie",
                      "duration": 1000
                    },
                    {
                      "title": "the old sid is refused by both zones",
                      "duration": 1000
                    },
                    {
                      "title": "the IdP session is dead: prompt=none returns login_required",
                      "duration": 1000
                    }
```

`skipped.json` keeps 0 J2 steps. Run `node -e 'JSON.parse(require("fs").readFileSync(process.argv[1]))' <file>` on both files: no output means valid JSON.

- [ ] **Step 4: Run the checker tests (still one red)**

```bash
node --test ci/kind/journeys-report.test.mjs
```

Expected: exactly one failing row, `the real journeys spec files hold exactly the expected step titles`. It names the five old titles of the journey file.

- [ ] **Step 5: Write the journey**

Replace the whole content of `ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts` with the text below. It is written for **branch A**. If Task 1 recorded **branch B**, apply the two replacements after the code block.

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-514 scenario 1 (spec § 5): the auth round trip against the kind stack. A cold visit goes to
// the IdP and back through the callback. Logout through the shell then kills the session on the
// server, in BOTH zones, not only in the browser.
//
// The seven step titles are pinned by ci/kind/journeys-report.mjs (EXPECTED_STEPS): `run.sh specs
// journeys` checks them in this file before the run and in the JSON report after it, so a step
// that never ran fails the job although every counter says "passed" (spec § 7.2).
//
// A replay context holds ONLY the session cookie; it does not stop the redirect. Measured on
// Playwright 1.63 (Task 4 review, decision D8): a `context.route` handler never sees a
// server-side redirect hop, so a route cannot stop one. The real handleLogin therefore runs in
// step 6: it deletes the presented sid, which is already dead, and sends the new context to the
// Keycloak form. That context is never reused or closed after it shows Keycloak (SMA-652). The
// third step's replay control has no stop either: if the control fails, handleLogin deletes the
// LIVE sid, but the test has already failed at that control by then.
//
// SSO (SMA-682 D2, D3): the kind realm grants `offline_access` only as an OPTIONAL client scope,
// and ci/kind/values/a.yaml sets `oidc.scopes` to `openid profile email`. So the console gets an
// online refresh token, and Keycloak keeps its SSO session after the code exchange. Steps 4 and 7
// ask Keycloak about that session with a prompt=none authorization request. Each request runs in
// a NEW context that holds only the two Keycloak cookies that step 4 read from context A after the
// login (SMA-514 D10, restored). The end-session response expires KEYCLOAK_IDENTITY in context A,
// so a probe in context A could show login_required while the server session is still alive. A
// replay of the old values asks the SERVER. Step 4 is the control of step 7: the same cookies and
// the same request returned a code before logout.
//
// The probe's redirect_uri is https://console.paigasus.test/kind-sso-probe, a registered redirect
// URI that no ingress rule routes (SMA-682 D6). With /iam/auth/callback, the foreign state gives
// txn_missing, the callback sends the browser to /iam/auth/login, and handleLogin logs in again
// silently through SSO. That adds a console session and can hide a failure. Step 4's code is never
// exchanged, and Keycloak lets it expire. With prompt=none, Keycloak never shows its form (OIDC
// Core § 3.1.2.1), so no Keycloak tab exists to reuse (SMA-652).
//
// D8 (SMA-682): step 5 asserts that Keycloak answers end-session with a redirect, not a page, and
// that no Keycloak document after Sign out has status 200. Since SMA-681, logout sends
// `id_token_hint`, and step 5 asserts it: a three-part JWT issued to `paigasus-console`.
// handleLogout revokes the refresh token BEFORE it builds the end-session redirect. Measured
// (SMA-682 spec § 13, row M6): that revoke leaves the SSO session alive. So the SSO session is live
// at end-session, and D8 is the end-to-end proof of SMA-681 AC 1: with the hint, Keycloak
// redirects at once. Step 7 proves that end-session ends the IdP session.
import { randomUUID } from 'node:crypto';
import { expect, test, type Browser, type BrowserContext, type Cookie, type Request, type Response } from '@playwright/test';
import { CONSOLE_HOST, IDP_HOST, SESSION_COOKIE, credential, redirectChain, sessionCookie, waitForHydration } from '../support/login';

const ORIGIN = `https://${CONSOLE_HOST}`;
const ZONES = [
  { base: '/iam', page: '/iam/orgs' },
  { base: '/gateway', page: '/gateway/overview' },
] as const;
const WAIT = { timeout: 30_000 } as const;
const PROBE_REDIRECT = `${ORIGIN}/kind-sso-probe`;
const KEYCLOAK_COOKIES = ['KEYCLOAK_IDENTITY', 'KEYCLOAK_SESSION'] as const;

function isConsolePath(request: Request, pathname: string): boolean {
  const url = new URL(request.url());
  return url.hostname === CONSOLE_HOST && url.pathname === pathname;
}

/** A new context with no state but `sid`. Nothing stops a redirect: handleLogin runs for real. */
async function replayContext(browser: Browser, sid: string): Promise<BrowserContext> {
  const context = await browser.newContext({ baseURL: ORIGIN, ignoreHTTPSErrors: true });
  // `url`, not `domain`: a __Host- cookie must be host-only (no Domain attribute), Secure, Path=/.
  await context.addCookies([{ name: SESSION_COOKIE, value: sid, url: `${ORIGIN}/`, secure: true, httpOnly: true, sameSite: 'Lax' }]);
  return context;
}

/** The earliest request in `response`'s redirect chain (the one the caller's `goto` made). */
function firstRequestOf(response: Response): Request {
  let request = response.request();
  for (let from = request.redirectedFrom(); from !== null; from = request.redirectedFrom()) request = from;
  return request;
}

/**
 * A prompt=none authorization request, sent as a browser navigation in a NEW context that holds
 * only `idpCookies`. Returns the Location of the Keycloak 302. The server-side SSO session decides
 * the answer, not the cookie state of any other context. The context is never closed or reused.
 */
async function silentAuthorize(browser: Browser, authorizationEndpoint: URL, idpCookies: readonly Cookie[]): Promise<URL> {
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  await context.addCookies(idpCookies);
  const page = await context.newPage();
  const state = randomUUID();
  const url = new URL(authorizationEndpoint.href);
  url.search = new URLSearchParams({ client_id: 'paigasus-console', response_type: 'code', scope: 'openid', redirect_uri: PROBE_REDIRECT, state, prompt: 'none' }).toString();
  const response = await page.goto(url.href);
  if (response === null) throw new Error('page.goto(prompt=none) returned no response');
  const chain = await redirectChain(response);
  const hops = chain.map((hop) => `${hop.url.hostname}${hop.url.pathname} ${String(hop.status)}`).join(' -> ');
  const [first, second] = chain;
  expect(first?.url.hostname, `prompt=none: the first hop must be the IdP: ${hops}`).toBe(IDP_HOST);
  expect(first?.status ?? 0, `prompt=none: the IdP must answer with a redirect, not a page: ${hops}`).toBeGreaterThanOrEqual(300);
  expect(first?.status ?? 0, `prompt=none: the IdP must answer with a redirect, not a page: ${hops}`).toBeLessThan(400);
  expect(second === undefined ? undefined : `${second.url.origin}${second.url.pathname}`, `prompt=none: the IdP must redirect to the probe URI: ${hops}`).toBe(PROBE_REDIRECT);
  // D6 (c): no hop may reach a console zone, where a txn_missing callback would log in again.
  expect(
    chain.filter((hop) => hop.url.hostname === CONSOLE_HOST && ZONES.some((zone) => hop.url.pathname.startsWith(zone.base))).map((hop) => hop.url.pathname),
    `prompt=none: no hop may reach a console zone: ${hops}`,
  ).toEqual([]);
  if (second === undefined) throw new Error(`prompt=none: the chain has no second hop: ${hops}`);
  expect(second.url.searchParams.get('state'), 'prompt=none: the IdP must return the same state').toBe(state);
  // The probe URI's own status is not asserted (no ingress rule routes it; 404 expected). The
  // annotation records it, and the job log prints it on a green run (SMA-682 M11).
  const result = second.url.searchParams.has('code') ? 'code' : `error=${second.url.searchParams.get('error') ?? '(none)'}`;
  test.info().annotations.push({ type: 'prompt=none probe', description: `${hops} (${result})` });
  return second.url;
}

test('J1: a cold visit logs in through the IdP, and logout ends the session in both zones (SMA-514 scenario 1)', async ({ browser, context, page }) => {
  const authorizationEndpoint = await test.step('cold visit: /iam/orgs goes through /iam/auth/login to the IdP form', async () => {
    expect(await context.cookies(), 'context A starts with no cookie').toEqual([]);
    const response = await page.goto('/iam/orgs');
    if (response === null) throw new Error('page.goto(/iam/orgs) returned no response');
    const chain = await redirectChain(response);
    const hops = chain.map((hop) => `${hop.url.hostname}${hop.url.pathname} ${String(hop.status)}`);
    expect(
      chain.some((hop) => hop.url.hostname === CONSOLE_HOST && hop.url.pathname === '/iam/auth/login'),
      `the chain must pass /iam/auth/login: ${hops.join(' -> ')}`,
    ).toBe(true);
    const last = chain.at(-1);
    expect(last?.url.hostname, `the chain must end at the IdP: ${hops.join(' -> ')}`).toBe(IDP_HOST);
    expect(last?.url.pathname.endsWith('/protocol/openid-connect/auth'), `the chain must end at the authorization endpoint: ${hops.join(' -> ')}`).toBe(true);
    await expect(page.locator('#username')).toBeVisible();
    if (last === undefined) throw new Error('the cold-visit chain is empty');
    // Steps 4 and 7 use this endpoint: no discovery request and no second source of truth.
    return new URL(`${last.url.origin}${last.url.pathname}`);
  });

  const sid = await test.step('login: the callback returns to /iam/orgs with a session', async () => {
    const callback = page.waitForRequest((request) => isConsolePath(request, '/iam/auth/callback'), WAIT);
    const landing = page.waitForResponse((response) => response.request().resourceType() === 'document' && isConsolePath(response.request(), '/iam/orgs'), WAIT);
    await page.locator('#username').fill(credential('PAIGASUS_KIND_USERNAME'));
    await page.locator('#password').fill(credential('PAIGASUS_KIND_PASSWORD'));
    await page.locator('#kc-login').click();
    await callback;
    expect((await landing).status(), '/iam/orgs after the callback').toBe(200);
    await waitForHydration(page);
    return sessionCookie(page);
  });

  await test.step('control: the sid replays in both zones', async () => {
    // Without this control, the "old sid is refused" step could pass because the replay method is
    // broken, not because the session is dead.
    for (const zone of ZONES) {
      const replay = await replayContext(browser, sid);
      const replayPage = await replay.newPage();
      const response = await replayPage.goto(zone.page);
      if (response === null) throw new Error(`page.goto(${zone.page}) returned no response`);
      expect(response.request().redirectedFrom(), `${zone.page}: the replayed sid must be accepted with no redirect`).toBeNull();
      expect(response.status(), zone.page).toBe(200);
      expect(new URL(replayPage.url()).pathname).toBe(zone.page);
      await waitForHydration(replayPage);
    }
  });

  const idpCookies = await test.step('control: the IdP session answers prompt=none with a code', async () => {
    // SMA-682 AC 1. On the old realm (offline_access as a default scope), Keycloak clears both
    // cookies after the code exchange, so this step fails here at the cookie check.
    const cookies = (await context.cookies(`https://${IDP_HOST}/realms/paigasus/`)).filter((cookie) => KEYCLOAK_COOKIES.some((name) => name === cookie.name));
    const names = cookies.map((cookie) => cookie.name);
    for (const name of KEYCLOAK_COOKIES) expect(names, `Keycloak keeps ${name} after the console login (SMA-682)`).toContain(name);
    expect(names, 'exactly one cookie per Keycloak cookie name').toHaveLength(KEYCLOAK_COOKIES.length);
    const location = await silentAuthorize(browser, authorizationEndpoint, cookies);
    expect(location.searchParams.get('error'), 'prompt=none with a live IdP session: no error').toBeNull();
    expect(location.searchParams.get('code'), 'prompt=none with a live IdP session: a code').not.toBeNull();
    return cookies;
  });

  await test.step('logout: the shell form ends at the IdP and returns to /iam/ with no session cookie', async () => {
    const requested: string[] = [];
    const documents: { readonly url: URL; readonly status: number }[] = [];
    page.on('request', (request) => requested.push(new URL(request.url()).pathname));
    page.on('response', (response) => {
      if (response.request().resourceType() === 'document') documents.push({ url: new URL(response.url()), status: response.status() });
    });
    const post = page.waitForRequest((request) => request.method() === 'POST' && isConsolePath(request, '/iam/auth/logout'), WAIT);
    const endSession = page.waitForRequest((request) => {
      const url = new URL(request.url());
      return url.hostname === IDP_HOST && url.pathname.endsWith('/protocol/openid-connect/logout');
    }, WAIT);
    const back = page.waitForRequest((request) => {
      const url = new URL(request.url());
      return url.hostname === CONSOLE_HOST && url.pathname === '/iam/' && url.searchParams.has('state');
    }, WAIT);

    // The user menu is the LAST menu trigger in the header (the org switcher comes before it).
    await page.getByRole('banner').locator('button[aria-haspopup="menu"]').last().click();
    await page.getByRole('menuitem', { name: 'Sign out' }).click();

    expect((await (await post).response())?.status(), 'POST /iam/auth/logout').toBe(302);
    // The 302 alone does not prove the SMA-653 defect class is absent: under a CSP form-action that
    // blocks the redirect, Chromium gets the 302 and then refuses to follow it. The end-session
    // request observed below is the proof that the browser actually followed it to the IdP.
    // page.waitForRequest sees the wire: handleLogout's degraded arm never contacts the IdP
    // (docs/superpowers/specs/2026-09-09-sma-506-measurements.md:803-838).
    const endSessionRequest = await endSession;
    expect(new URL(endSessionRequest.url()).searchParams.get('client_id'), 'end-session client_id').toBe('paigasus-console');

    // D8 (SMA-682): a 3xx, not a page. It comes BEFORE the hint assertions and before the wait for
    // /iam/, so a logout with no hint fails here with the D8 message (SMA-682 spec § 6.2).
    const endSessionStatus = (await endSessionRequest.response())?.status() ?? 0;
    expect(endSessionStatus, 'D8: Keycloak answered end-session with a page, not a redirect (a confirmation page?)').toBeGreaterThanOrEqual(300);
    expect(endSessionStatus, 'D8: Keycloak answered end-session with a page, not a redirect (a confirmation page?)').toBeLessThan(400);

    // SMA-681 AC 2: the end-session request carries the stored ID token. Its `aud` must name the
    // client: logout sends the hint only then (http/routes.ts), and Keycloak rejects any other `aud`.
    const hint = new URL(endSessionRequest.url()).searchParams.get('id_token_hint');
    expect(hint, 'end-session id_token_hint (SMA-681)').not.toBeNull();
    const parts = (hint ?? '').split('.');
    expect(parts, 'id_token_hint is a three-part JWT').toHaveLength(3);
    const claims = JSON.parse(Buffer.from(parts[1] ?? '', 'base64url').toString('utf8')) as { aud?: unknown };
    const audiences: unknown[] = Array.isArray(claims.aud) ? claims.aud : [claims.aud];
    expect(audiences.includes('paigasus-console'), 'id_token_hint aud names paigasus-console').toBe(true);

    // D8 held with a live SSO session (SMA-682 spec § 13, row M6), so the hint is what made
    // Keycloak redirect at once.
    await back;
    await page.waitForURL((url) => url.hostname === CONSOLE_HOST && /^\/iam\/?$/.test(url.pathname));
    await expect(page.getByTestId('public-home')).toBeVisible();
    expect(
      documents.filter((doc) => doc.url.hostname === IDP_HOST && doc.status === 200).map((doc) => doc.url.pathname),
      'D8: no Keycloak document after Sign out has status 200',
    ).toEqual([]);

    // Measured, not assumed: Next can answer /iam/ with a trailing-slash redirect to /iam.
    const home = documents.filter((doc) => doc.url.hostname === CONSOLE_HOST && /^\/iam\/?$/.test(doc.url.pathname));
    test.info().annotations.push({
      type: 'post-logout documents',
      description: [
        `${new URL(endSessionRequest.url()).pathname} (no confirmation page, SMA-682 D8)`,
        ...home.map((doc) => `${doc.url.pathname}${doc.url.search} ${String(doc.status)}`),
      ].join(' -> '),
    });
    expect(home.at(-1)?.status, 'the public page after logout').toBe(200);
    // The chart sets no PAIGASUS_OIDC_POST_LOGOUT_REDIRECT_URI, so the IdP returns to `${origin}/iam/`.
    expect(
      requested.filter((path) => path === '/iam/auth/logout/callback'),
      '/iam/auth/logout/callback is not requested',
    ).toEqual([]);
    expect(
      (await context.cookies()).filter((cookie) => cookie.name === SESSION_COOKIE),
      'the session cookie is gone',
    ).toEqual([]);
  });

  await test.step('the old sid is refused by both zones', async () => {
    for (const zone of ZONES) {
      // One new context per URL (spec § 5 step 5): each sees the old sid exactly once. The chain
      // is NOT stopped (see the header comment): it runs to the real IdP form, so only the first
      // two hops are asserted here, never the final URL.
      const replay = await replayContext(browser, sid);
      const replayPage = await replay.newPage();
      const response = await replayPage.goto(zone.page);
      if (response === null) throw new Error(`page.goto(${zone.page}) returned no response`);
      const chain = await redirectChain(response);
      const hops = chain.map((hop) => `${hop.url.hostname}${hop.url.pathname} ${String(hop.status)}`);
      const cookieHeader = (await firstRequestOf(response).allHeaders())['cookie'] ?? '';
      expect(cookieHeader.split(/;\s*/), `${zone.page}: the first request must carry the old sid`).toContain(`${SESSION_COOKIE}=${sid}`);
      const first = chain[0];
      expect(first?.status, `${zone.page}: the first hop must be a redirect: ${hops.join(' -> ')}`).toBeGreaterThanOrEqual(300);
      expect(first?.status, `${zone.page}: the first hop must be a redirect: ${hops.join(' -> ')}`).toBeLessThan(400);
      // The proxy checks only that the cookie exists (middleware.ts:122), so a redirect to login
      // here can come only from requireSession()'s store lookup: the Redis record is gone.
      expect(chain[1]?.url.pathname, `${zone.page}: the next hop must be ${zone.base}/auth/login: ${hops.join(' -> ')}`).toBe(`${zone.base}/auth/login`);
    }
  });

  await test.step('the IdP session is dead: prompt=none returns login_required', async () => {
    // Step 4 is the control: the same cookie values (captured before logout) and the same request
    // returned a code. The values are replayed in a new context, so this result cannot come from
    // the cookie clear in context A. It can come only from the server-side end of the SSO session.
    const location = await silentAuthorize(browser, authorizationEndpoint, idpCookies);
    expect(location.searchParams.get('code'), 'prompt=none after logout: no code').toBeNull();
    expect(location.searchParams.get('error'), 'prompt=none after logout').toBe('login_required');
  });
});
```

**Branch B only.** Replace these four header lines:

```ts
// handleLogout revokes the refresh token BEFORE it builds the end-session redirect. Measured
// (SMA-682 spec § 13, row M6): that revoke leaves the SSO session alive. So the SSO session is live
// at end-session, and D8 is the end-to-end proof of SMA-681 AC 1: with the hint, Keycloak
// redirects at once. Step 7 proves that end-session ends the IdP session.
```

with:

```ts
// handleLogout revokes the refresh token BEFORE it builds the end-session redirect. Measured
// (SMA-682 spec § 13, row M6): that revoke already ends the SSO session. So D8 proves only that the
// console's real logout sequence shows no confirmation page. It is not a proof of SMA-681 AC 1 with
// a live session. Step 7 proves that the logout sequence (revoke, then end-session) ends the IdP
// session, not that end-session alone does (SMA-682 residual R7).
```

and replace this step 5 comment:

```ts
    // D8 held with a live SSO session (SMA-682 spec § 13, row M6), so the hint is what made
    // Keycloak redirect at once.
```

with:

```ts
    // The revoke already ended the SSO session (SMA-682 spec § 13, row M6), so D8 above does not
    // prove what the hint does (SMA-682 residual R7).
```

- [ ] **Step 6: Format, then run every local check**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso
pnpm -C ts exec prettier --write apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts
grep -nE '\.(skip|fixme|fail|only)[[:space:]]*\(' ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts || echo "skip scan ok"
node --test ci/kind/journeys-report.test.mjs
node ci/kind/journeys-report.mjs sources ts/apps/iam-console/tests/cluster/journeys
moon run iam-console-ts:typecheck ts:lint ts:fmt
```

Expected: `skip scan ok`; every `node:test` row passes; `ok [journeys sources]: both spec files hold the expected step titles`; the three Moon tasks pass. If `iam-console-ts:typecheck` fails in `contracts:generate` with a BSR rate limit, see the memory note "BSR rate limit deletes generated file": restore the deleted generated file, do not change the journey.

The real J1 run is the `chart` workflow's `kind` job on the pull request (spec AC 5). No local Moon task runs J1.

- [ ] **Step 7: Commit**

```bash
git branch --show-current
git add ci/kind/journeys-report.mjs ci/kind/journeys-report.test.mjs ci/kind/fixtures/journeys-report/pass.json ci/kind/fixtures/journeys-report/skipped.json ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts
git commit -m "test(ts): prove the Keycloak SSO session and its end at logout in J1 (SMA-682)" -m "J1 gets steps 4 and 7, a prompt=none probe in a new context with the two captured
Keycloak cookies, and the D8 assertions in step 5. EXPECTED_STEPS, the pass and
skipped fixtures and the checker test follow the seven titles.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Documentation

**Files:**
- Modify: `ts/packages/paigasus-auth/README.md:37` (table row) and `:61-77` (the section), insert after line 77
- Modify: `docs/ops/RUNBOOK-chart.md:37`, `:157`, `:253-265`, `:457-460` (and insert the § 3.5.1 paragraph after line 460)
- Modify: `charts/paigasus/values.yaml:184-195`
- Modify: `charts/paigasus/README.md:278-281`
- Modify: `ts/packages/paigasus-auth/tests/e2e/logout.spec.ts:31-33`, `ts/packages/paigasus-auth/tests/e2e/constants.ts:35-37`

**Interfaces:**
- Consumes: Task 1's M3 result (cited), M8 result and M10 result (they select two runbook sentences).
- Produces: the runbook heading text `**Keycloak: online tokens and session length (SMA-682).**`, which the package README names.

All prose is ASD-STE100 Simplified Technical English, like the existing runbook.

- [ ] **Step 1: Prove that the wrong claim is in the repository (red)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso
grep -n 'Keycloak, for example' ts/packages/paigasus-auth/README.md
grep -n 'and the .offline_access. role\|complete example of this setup\|or the IdP issues no refresh token' docs/ops/RUNBOOK-chart.md
```

Expected: one README line (63) and three runbook lines (37, 459, 460). Step 7 runs the same commands and expects no output.

- [ ] **Step 2: The package README**

In `ts/packages/paigasus-auth/README.md` line 37, in the last cell, replace:

```
Keep `offline_access` unless the IdP issues a refresh token without it — removing it can make every access-token expiry log the user out (see "Why `offline_access` is a default scope" below).
```

with:

```
Keep `offline_access`, except on Keycloak when you want SSO (see "Keycloak: offline or online tokens" below). On other IdPs, removing it can make every access-token expiry log the user out (see "Why `offline_access` is a default scope" below).
```

Replace lines 63-67:

```
Without `offline_access`, some identity providers (Keycloak, for example) never issue a refresh
token for the code flow. This package deletes the session and logs the user out the moment the
access token expires if no refresh token is available — which, on some providers, is as often as
every five minutes. Keep `offline_access` in `PAIGASUS_OIDC_SCOPES` unless you have verified your
IdP issues refresh tokens without it.
```

with:

```
Without `offline_access`, Entra ID, Auth0 and Okta never issue a refresh token for the code flow.
This package deletes the session and logs the user out the moment the access token expires if no
refresh token is available — which, on some providers, is as often as every five minutes. Keep
`offline_access` in `PAIGASUS_OIDC_SCOPES` unless you have verified your IdP issues refresh tokens
without it, or you use Keycloak and want SSO (see the next section).
```

Insert after line 77 (`deployment signs out together, at whatever interval the IdP's access-token lifetime is.`) and before `### The session store: \`redis\` vs \`memory\``:

```markdown

#### Keycloak: offline or online tokens

Keycloak issues a refresh token for the code flow also without `offline_access`. That token is an
online token (`typ: Refresh`). With `offline_access`, Keycloak issues an offline token
(`typ: Offline`), and it keeps no SSO session after the console login (SMA-682). Then other
applications of the realm get no SSO from a console login, and a new authorization request shows
the login form again.

To keep the SSO session, do the two steps. Set `PAIGASUS_OIDC_SCOPES` to `openid profile email`.
Put `offline_access` in the client's optional client scopes, not in its default client scopes.
Keycloak grants a default client scope also when the console does not request it.

With online tokens, each refresh needs a live Keycloak SSO session. So the console session ends
when the SSO session ends. On the Keycloak defaults (`SSO Session Idle` 30 min, `SSO Session Max`
10 h), this is shorter than the session defaults of this package (8 h idle, 24 h absolute). Read
the trade-off and the three setups in
[RUNBOOK-chart § 6, "Keycloak: online tokens and session length"](../../../docs/ops/RUNBOOK-chart.md#6-the-idp-contract)
before you change the scopes.
```

Then run `pnpm -C ts exec prettier --write packages/paigasus-auth/README.md`. Prettier pads the table again. Check with `git diff --stat` that only this README changed in this step.

- [ ] **Step 3: RUNBOOK-chart, the values table and the scope paragraphs**

In `docs/ops/RUNBOOK-chart.md` line 37, replace `Keep \`offline_access\`, or the IdP issues no refresh token.` with:

```
Keep `offline_access`, except on Keycloak when you want SSO (§ 6, "Keycloak: online tokens and session length"). Without it, Entra ID, Auth0 and Okta issue no refresh token.
```

Line 157: replace `(\`oidc.scopes\`, default \`openid profile email offline_access\`). The console sends an` with:

```
   (`oidc.scopes`, default `openid profile email offline_access`; `openid profile email` on
   Keycloak with SSO). The console sends an
```

Keep the line after it (`\`audience\` parameter only when …`) as it is.

Replace lines 253-258:

```
By default the console requests the scopes `openid profile email offline_access`. Set
`oidc.scopes` to request a different list. The list must contain `openid`. Keep `offline_access`.

Without it, the IdP issues no refresh token. Every user must log in again when the access token
expires. When `oidc.scopes` is set, the console also sends the list as the `scope` of each
refresh request. When it is empty, a refresh request has no `scope`, as before SMA-692.
```

with:

```
By default the console requests the scopes `openid profile email offline_access`. Set
`oidc.scopes` to request a different list. The list must contain `openid`. Keep `offline_access`,
except on Keycloak when you want SSO (see "Keycloak: online tokens and session length" below).

Without it, Entra ID, Auth0 and Okta issue no refresh token. Every user must log in again when the
access token expires. Keycloak issues a refresh token also without it. When `oidc.scopes` is set,
the console also sends the list as the `scope` of each refresh request. When it is empty, a
refresh request has no `scope`, as before SMA-692.
```

Replace lines 260-265:

```
**Set `oidc.scopes` only when your IdP needs it.** This applies to any IdP, not only Entra ID
(see "Entra ID moving to a new scope list" below). RFC 6749 § 6 lets an authorization server
refuse a refresh `scope` that is not a subset of the originally granted scope. So a refresh with
this list can fail on an IdP that enforces that rule. The user is then signed out at each
access-token expiry, not only at the next login. After you set or change `oidc.scopes`, read the
`oauthError` field of the `session.refresh_failed` log line to check for this.
```

with:

```
**Set `oidc.scopes` only when your IdP needs it.** This applies to any IdP, not only Entra ID
(see "Entra ID moving to a new scope list" below). Keycloak with SSO also needs it (see Keycloak
example 1). RFC 6749 § 6 lets an authorization server refuse a refresh `scope` that is not a
subset of the originally granted scope. So a refresh with this list can fail on an IdP that
enforces that rule. The user is then signed out at each access-token expiry, not only at the next
login. After you set or change `oidc.scopes`, read the `oauthError` field of the
`session.refresh_failed` log line to check for this. The Keycloak list `openid profile email` is
the list that the console requested, so it is not wider than the granted scope. Keycloak 26.7
accepts it on a refresh (SMA-682 spec § 13, row M3).
```

- [ ] **Step 4: RUNBOOK-chart, Keycloak example 1 and the § 3.5.1 paragraph**

Replace lines 457-460:

```
Put `basic`, `profile`, `email` and `offline_access` in the client's default client scopes. In
Keycloak 25 and later the `sub` claim comes from the `basic` scope. Give each user an email
address and the `offline_access` role. The kind job's realm, `ci/kind/realm/paigasus-realm.json`,
is a complete example of this setup.
```

with the text below. Select the two marked sentences by the Task 1 results, and delete the markers and the sentence that does not apply.

```
Put `basic`, `profile` and `email` in the client's default client scopes, and `offline_access` in
the optional client scopes. Set `oidc.scopes=openid profile email`. Keycloak then issues an online
refresh token and keeps the SSO session after the console login. In Keycloak 25 and later the
`sub` claim comes from the `basic` scope. Give each user an email address. This setup does not use
the `offline_access` role. The kind job's user keeps the role (SMA-682 D4). The kind job's realm,
`ci/kind/realm/paigasus-realm.json`, with `ci/kind/values/a.yaml`, is an example of this scope
setup. It keeps the Keycloak default SSO timeouts.

**Keycloak: online tokens and session length (SMA-682).** Read these facts before you select the
scopes for Keycloak:

1. With the setup of example 1, each console refresh needs a live Keycloak SSO session. So the
   console session ends when the SSO session ends.
2. The Keycloak defaults are `SSO Session Idle` 30 min and `SSO Session Max` 10 h. The console
   session is 8 h idle and 24 h absolute. The chart does not expose the two console values, so
   you cannot change them with the chart.
3. So on the Keycloak defaults, the setup of example 1 makes console sessions shorter. Keycloak
   signs out an idle user after 30 min, and every user after 10 h. The setup gives SSO to the
   other applications of the realm, and nothing more. It does not remove the password prompt when
   the console session ends, because the SSO session has already ended at that time.
4. `SSO Session Idle` and `SSO Session Max` apply to all clients of the realm. If you increase
   them to the console values, the SSO session of every application in the realm becomes longer.
   A stolen `KEYCLOAK_IDENTITY` cookie and an unattended SSO session then also stay valid for a
   longer time.
5. `Client Session Idle` and `Client Session Max` (at the realm or the client level) can make the
   console session shorter than the SSO session. Do not set them, or set them to values that are
   not lower than the SSO values.
6. Online sessions stay after a Keycloak restart only with persistent user sessions. This is the
   default since Keycloak 26, and you can turn it off. Without persistent user sessions, a restart
   signs out every console user at the next refresh.
7. A logout in another application of the realm, or an administrator "sign out" of the user, ends
   the console session at the next refresh. With offline tokens, these events do not end the
   console session.
[M8 = login_required ONLY:]
8. A console logout on one device also ends the SSO session of the same user on the other devices
   (SMA-682 spec § 13, row M8).

You can use one of these three setups:

- **(i) Example 1 on the Keycloak default timeouts.** Other applications of the realm get SSO.
  Console sessions end after 30 min idle and after 10 h.
- **(ii) Example 1, and increase `SSO Session Idle` to 8 h and `SSO Session Max` to 24 h for the
  realm.** Console sessions are as long as with offline tokens, and all applications get SSO.
  Every application of the realm then has a longer SSO session (item 4). This is a security
  trade-off.
- **(iii) Keep `offline_access`.** Keep the default scopes, and keep `offline_access` as a default
  client scope. Console sessions last 8 h idle and 24 h absolute, and the SSO timeouts have no
  effect on them. Keycloak keeps no SSO session after a console login, so other applications get
  no SSO from it.
  [M10 = login_required:] A console login also ends an SSO session that another application of
  the realm started before in the same browser (SMA-682 spec § 13, row M10).
  [M10 = code:] An SSO session that another application of the realm started before in the same
  browser stays (SMA-682 spec § 13, row M10).

**Recommendation.** If no other application of the realm needs SSO, use (iii). If another
application needs SSO, use (i) or (ii). Use (ii) only when you accept item 4.
```

If M8 returned a code, delete item 8 and its marker. Keep the blank line between item 7 and "You can use".

- [ ] **Step 5: The chart values comment and the chart README**

In `charts/paigasus/values.yaml`, replace lines 184-195 (the `scopes:` line and its 11 comment lines) with:

```yaml
  scopes: ""             # NOT required. The scopes that both consoles request (SMA-692). Empty:
                         # the console default, openid profile email offline_access. Entra ID
                         # needs it: add exactly one scope of the API app registration, for
                         # example api://paigasus-api/access. The list must contain openid (the
                         # render fails without it). It should contain offline_access: without
                         # it Entra ID, Auth0 and Okta issue no refresh token, and every user
                         # must log in again at each access-token expiry. On Keycloak, leave out
                         # offline_access to keep SSO (RUNBOOK-chart.md § 6). When set, the
                         # consoles also send it on each refresh. Set it only when your IdP
                         # needs it, or on Keycloak to keep SSO: on any IdP, a refresh scope
                         # wider than the granted scope can fail (RFC 6749 § 6) and sign the
                         # user out at each access-token expiry. Read the oauthError field of
                         # the session.refresh_failed log after a change. A change restarts both
                         # consoles, not IAM. See RUNBOOK-chart.md § 6.
```

Each comment line starts with 25 spaces and then `#`, like the lines around it.

In `charts/paigasus/README.md`, replace lines 278-281:

```
- `oidc.scopes` renders `PAIGASUS_OIDC_SCOPES` into the `console-env` ConfigMap. When empty, the
  console uses its default scopes: `openid profile email offline_access`. When set, the consoles
  also send this list as the `scope` of each refresh request. Entra ID needs one scope of its
  API.
```

with:

```
- `oidc.scopes` renders `PAIGASUS_OIDC_SCOPES` into the `console-env` ConfigMap. When empty, the
  console uses its default scopes: `openid profile email offline_access`. On Keycloak, leave out
  `offline_access` to keep SSO (`docs/ops/RUNBOOK-chart.md` § 6). When set, the consoles also send
  this list as the `scope` of each refresh request. Entra ID needs one scope of its API.
```

- [ ] **Step 6: The D5 clauses in the package e2e suite**

In `ts/packages/paigasus-auth/tests/e2e/logout.spec.ts`, replace lines 31-33:

```ts
  // when an SSO session is live (SMA-681 spec § 3). This realm grants `offline_access` as a default
  // scope, so no SSO session exists here and no page appears either way (SMA-682): this test proves
  // the hint is SENT, and that Keycloak still completes the redirect with it (§ 3.1 row M-i1).
```

with:

```ts
  // when an SSO session is live (SMA-681 spec § 3). This realm grants `offline_access` as a default
  // scope on purpose: this suite tests the package default scope list (SMA-682 D5). So no SSO
  // session exists here and no page appears either way (SMA-682): this test proves the hint is
  // SENT, and that Keycloak still completes the redirect with it (§ 3.1 row M-i1). J1 on the kind
  // stack checks the live-session case.
```

In `ts/packages/paigasus-auth/tests/e2e/constants.ts`, replace lines 35-37:

```ts
 * above. `offline_access` is promoted from Keycloak's default OPTIONAL client scope to a DEFAULT
 * one on that client so the default scope list (`src/runtime.ts`'s `DEFAULT_OIDC_SCOPES`, `openid
 * profile email offline_access`) issues a refresh_token without any extra consent-flow wiring.
```

with:

```ts
 * above. `offline_access` is promoted from Keycloak's default OPTIONAL client scope to a DEFAULT
 * one on that client so the default scope list (`src/runtime.ts`'s `DEFAULT_OIDC_SCOPES`, `openid
 * profile email offline_access`) issues a refresh_token without any extra consent-flow wiring.
 * This realm keeps that default on purpose (SMA-682 D5): it tests the package default scope list,
 * so no SSO session exists after a login here. The kind realm (ci/kind/realm/) uses online scopes.
```

- [ ] **Step 7: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso
grep -n 'Keycloak, for example' ts/packages/paigasus-auth/README.md
grep -n 'and the .offline_access. role\|complete example of this setup\|or the IdP issues no refresh token' docs/ops/RUNBOOK-chart.md
grep -n '\[M8\|\[M10' docs/ops/RUNBOOK-chart.md
pnpm -C ts exec prettier --check packages/paigasus-auth/README.md packages/paigasus-auth/tests/e2e/logout.spec.ts packages/paigasus-auth/tests/e2e/constants.ts
moon run ts:fmt ts:lint repo:helm-render --force
```

Expected: the three greps print nothing (no wrong claim, no marker left); Prettier reports all files formatted; the three Moon tasks pass. `ts:fmt` does not hash `README.md` (`.moon/tasks/typescript.yml:36-42`), so `--force` is necessary.

- [ ] **Step 8: Commit**

```bash
git branch --show-current
git add ts/packages/paigasus-auth/README.md docs/ops/RUNBOOK-chart.md charts/paigasus/values.yaml charts/paigasus/README.md ts/packages/paigasus-auth/tests/e2e/logout.spec.ts ts/packages/paigasus-auth/tests/e2e/constants.ts
git commit -m "docs(repo): document Keycloak online scopes and the SSO session length (SMA-682)" -m "Keycloak issues a refresh token without offline_access. The README, the chart runbook,
values.yaml and the chart README now say so, describe the D2 setup, and state the three
session-length setups with the recommendation.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Bite proofs, the full gate run and the M11 record

**Files:**
- Modify (temporarily, never committed on the feature branch): `ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts`, `ci/kind/realm/paigasus-realm.json`, `ci/kind/values/a.yaml`, `ts/packages/paigasus-auth/src/http/routes.ts:563`
- Modify: `docs/superpowers/specs/2026-09-27-sma-682-keycloak-sso-online-scopes-design.md` (§ 13: the bite results and M11)

**Interfaces:**
- Consumes: Tasks 1-4. The M6 branch selects whether the D8 bite runs.
- Produces: the § 6.2 records and the M11 record in § 13.

A J1 run needs the kind stack. There are two ways, and no third one:

- **Local, best effort** (`ci/kind/README.md`, "Local run"): only if `command -v kind kubectl docker helm` finds all four, `kind version` prints v0.31.0, and host ports 80 and 443 are free. Do not install a missing tool. Docker Desktop's image store can make `run.sh images` fail here and pass in CI.
- **CI on a scratch branch**: push a branch named `feature/sma-682-keycloak-sso-bite-<n>-scratch` and run `gh workflow run chart.yml --ref <branch>`. **Ask Sven for his OK before the first push of any scratch branch** (memory: pre-push branch names for scratch runs). One run takes up to about 3 h (`chart.yml` `timeout-minutes: 182`).

If neither way is available, record each bite as "not run" in § 13, with the reason.

The CI way, for bite `<n>` (1, 2 or 3). Each bite gets its own scratch branch from the feature branch head. The mutation is committed ONLY on the scratch branch:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso
git switch -c feature/sma-682-keycloak-sso-bite-<n>-scratch
# apply the mutation of the bite with the Edit tool (or the git checkout of bite 2)
git commit -am "test(ts): SMA-682 bite <n> scratch run, do not merge" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u origin feature/sma-682-keycloak-sso-bite-<n>-scratch   # only after Sven's OK
gh workflow run chart.yml --ref feature/sma-682-keycloak-sso-bite-<n>-scratch
git switch feature/sma-682-keycloak-sso-online-scopes
git diff --exit-code
```

Read the result with `gh run list --workflow chart.yml --branch feature/sma-682-keycloak-sso-bite-<n>-scratch --limit 1` and `gh run view <run id> --log-failed`. After the result is recorded, ask Sven before you delete the remote scratch branch (`git push origin --delete <branch>`). Then delete the local branch with `git branch -D <branch>`. With the CI way, the "remove the mutation" sub-steps below do not apply: the feature branch never had the mutation.

- [ ] **Step 1: Bite 1, step 7 bites (step 7's assertion at the end of step 4)**

Add this line in step 4, right before `return cookies;`:

```ts
    expect(location.searchParams.get('error'), 'BITE 1: step 7 assertion copied into step 4').toBe('login_required');
```

Run J1. Local: `bash ci/kind/run.sh up`, `bash ci/kind/run.sh images`, `bash ci/kind/run.sh install a`, `bash ci/kind/run.sh stub up`, `bash ci/kind/run.sh specs journeys`. CI: commit the line on the scratch branch only, push with Sven's OK, `gh workflow run chart.yml --ref feature/sma-682-keycloak-sso-bite-1-scratch`.

Expected: `specs journeys` exits 1. J1 fails in step 4 with `BITE 1: step 7 assertion copied into step 4`, received `null` (the Location holds a `code`). Remove the line with the Edit tool. Run `git diff --exit-code ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts` (exit 0). For a local run, keep the cluster for bite 3 if it runs, else run `bash ci/kind/run.sh down`.

- [ ] **Step 2: Bite 2, step 4 bites (the old realm and no `oidc.scopes`)**

On a scratch state, restore only the realm and the values to `origin/main`:

```bash
git checkout origin/main -- ci/kind/realm/paigasus-realm.json ci/kind/values/a.yaml
```

This is safe on the feature branch only because Task 2 committed both files. Run J1 on a NEW cluster (local: `down`, then the five commands of Step 1; CI: commit on `feature/sma-682-keycloak-sso-bite-2-scratch`, push with Sven's OK, dispatch).

Expected: J1 fails in step 4, at the cookie check (`Keycloak keeps KEYCLOAK_IDENTITY after the console login (SMA-682)`) or at the `code` check. Record which one. This is the red-first run of the defect. Then restore the two files to the branch head:

```bash
git checkout HEAD -- ci/kind/realm/paigasus-realm.json ci/kind/values/a.yaml
git diff --exit-code
```

Expected: exit 0.

- [ ] **Step 3: Bite 3, D8 bites (branch A only)**

Branch B: do not run it. Record "M6 returned `login_required`; the D8 mutation cannot bite (§ 3.3.1)" in § 13.

Branch A: in `ts/packages/paigasus-auth/src/http/routes.ts`, delete line 563:

```ts
      ...(idToken !== undefined ? { idTokenHint: idToken } : {}),
```

In the journey, step 5, comment out the six hint lines (from `const hint = …` to `expect(audiences.includes(…`) with `//` at the start of each line. The console image is built from the working tree, so run J1 on a NEW cluster with `images` (local) or on `feature/sma-682-keycloak-sso-bite-3-scratch` (CI, with Sven's OK).

Expected: J1 fails in step 5 with `D8: Keycloak answered end-session with a page, not a redirect (a confirmation page?)`. Not a timeout. Not the hint message. Then remove both mutations with the Edit tool and run `git diff --exit-code` (exit 0).

- [ ] **Step 4: Record the bites in § 13**

Append to § 13 of the spec:

```markdown
### 13.1 Bite runs (§ 6.2)

| Bite | Where it ran | Result |
|---|---|---|
| Step 7 (assertion copied to step 4) | <local kind / CI run URL / not run, and why> | <the failing step and message> |
| Step 4 (old realm, no `oidc.scopes`) | <…> | <cookie check or code check, and the message> |
| D8 (no hint, hint assertions off) | <…, or "not run: branch B"> | <the failing step and message> |
```

- [ ] **Step 5: The full gate run**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso
git fetch origin main
mkdir -p "$WORK/bashshim" && ln -sf /bin/bash "$WORK/bashshim/bash"
PATH="$WORK/bashshim:$PATH" moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :test-e2e \
  --base origin/main \
  --include-relations
```

This run uses system bash 3.2 through a bash-only shim (root `CLAUDE.md`, "This development Mac only"). `repo:affected-smoke` needs 3.2. Do not put `/bin` first in `PATH`: it also downgrades `python3`.

The bash-4+ gates fail under 3.2 as a bash artifact, not as a finding: `repo:ruff-ci`, `repo:next-public-free` and `repo:publish-metadata` (`mapfile`, `declare -A`), and `repo:version-lockstep` and `repo:nats-permissions` (memory "Gate failures from the wrong bash"). For each of these that the run selected (read `.moon/cache/ciReport.json`), run the exact lines of its `script:` in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-682-keycloak-sso/moon.yml` with `/opt/homebrew/bin/bash` in place of `bash`. For example `/opt/homebrew/bin/bash ci/ruff/run.sh --self-test`, then `--negative-control`, then the plain run. Read those results, not the `moon ci` verdict, for these gates.

`repo:actionlint` gives a local verdict only under `/opt/homebrew/bin/bash ci/actionlint/run.sh` when its preflight prints a pipe capacity of at least 8192 bytes. If it exits rc 2 with a `small` message, the host is in the 512-byte-pipe state: CI is the verdict. A local `version-lockstep --negative-control` with `tar: Write error` is a host artifact (memory note, SMA-686).

If any other task fails, follow the "Diagnosing an unattributed `moon ci` failure" procedure of the root `CLAUDE.md` (Step 0 first: copy `.moon/cache/ciReport.json` and the task state directory before any re-run).

Expected: every selected task passes, with the bash rules above applied. `paigasus-auth-ts:test-e2e` passes with no realm change (spec AC 8).

- [ ] **Step 6: Commit the bite records**

```bash
git branch --show-current
git status --short
git add docs/superpowers/specs/2026-09-27-sma-682-keycloak-sso-online-scopes-design.md
git commit -m "docs(ci): record the SMA-682 bite runs" -m "Section 13.1 records where each bite of spec section 6.2 ran and how it failed.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

`git status --short` must show only the spec before the `git add`. Any other changed file is a mutation that was not removed: remove it first.

- [ ] **Step 7: After the pull request is open, read the `chart` run and record M11**

The pull request starts the `chart` workflow, because it changes `ci/kind/**` and `ts/apps/iam-console/tests/cluster/**` (`.github/workflows/chart.yml:27-43`). It is not a required check, so read it by hand (spec § 6.3):

```bash
gh run list --workflow chart.yml --branch feature/sma-682-keycloak-sso-online-scopes --limit 1
gh run view <run id> --log | grep -E 'ok \[journeys report\]|annotation \[auth-roundtrip.spec.ts\]'
```

Expected: `ok [journeys report]: 2 tests passed, 0 skipped, 0 flaky, every step ran`, two `"prompt=none probe"` annotation lines, and one `"post-logout documents"` line. The first probe line ends with `(code)`, the second with `(error=login_required)`. Each shows the chain `idp.paigasus.test/realms/paigasus/protocol/openid-connect/auth 302 -> console.paigasus.test/kind-sso-probe <status>`.

Copy the two probe lines into the § 13 table, row M11, with the run URL and the probe URI status (404 expected). Commit with `docs(ci): record the SMA-682 M11 probe chain from the kind job` and the Co-Authored-By line. If the run fails, use the `kind-evidence` artifact (`ci/kind/README.md`, "Reading the evidence"), and do not record M11.
