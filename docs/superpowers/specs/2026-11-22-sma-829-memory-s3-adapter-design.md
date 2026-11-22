# SMA-829 — Memory store: the S3-compatible `MemoryStore` adapter

- Linear: SMA-829. Project: Paigasus Agent Host.
- Blocked by: SMA-789 (merged, PR 514) and SMA-843 (merged, PR 522, `3617d4dd`).
- Related: SMA-790 (the run loop uses `MemoryCommit::External`).
- Design input: §6.8 (the S3 part), §6.12 and §6.13 of
  [the SMA-789 spec](./2026-11-20-sma-789-memory-store-design.md), and §6.2 and §6.6 of
  [the SMA-843 spec](./2026-11-21-sma-843-memory-dir-guard-design.md). This document records the
  changes to that input and the decisions that the input left open.
- Status: approved at Gate 1 on 2026-11-22. Sven picked `rustfs` as the primary test image (§7).
- Measured after Gate 1: (1) `rustfs` 1.0.6 holds a file and a directory of one name, so probe step 7
  is true (§6.2). `versitygw` v1.11.0 posix does not. (2) A folder marker `…/notes/` is not removed
  by `delete_scope` (§6.6, §7). (3) The fallback switch is not one constant (§7). (4)
  `conformance::run_all` has 34 checks, not 35. (5) A timed-out write that did not land is unknown
  (§6.5, ruling F1).

## 1. Intent

The memory store has one adapter today: Postgres. Some operators keep agent state in object storage,
not in the database. SMA-829 adds a second adapter over an S3-compatible store. The port, the six
commands, the caps and the run buffer do not change. The adapter must pass the SMA-789 conformance
suite **unchanged**. Postgres stays the default backend.

S3 has no transactions. So the S3 adapter gives weaker guarantees than Postgres: a change set can be
partly applied, and some checks are best effort. This document names each weak point (§6.7). The
adapter makes the sequential case correct and documents each race.

## 2. Goals

- G1. `S3MemoryStore` implements `MemoryStore` and passes `conformance::run_all` with no change to
  `conformance.rs`.
- G2. A version never repeats for a path (SMA-789 G1), also when the same content is written again.
- G3. A boot probe refuses to start the service on a store that ignores conditional writes, that
  gives two different contents one ETag, or that keeps old object versions.
- G4. `apply` writes every `Put` before any `Delete`. A failure after a write can return `Partial`.
  A failure never loses a file that the set did not delete.
- G5. `memory.backend = "s3"` config, with credentials only from an allowlist of AWS environment
  variables. No other environment variable changes the client, except these, which the README names:
  `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY` and their lower-case forms (`reqwest` reads
  them); `REQUEST_METHOD` (when `REQUEST_METHOD` is set, the client uses no proxy, a guard for
  CGI); and, on Linux,
  `SSL_CERT_FILE` and `SSL_CERT_DIR`, which change the system trust roots (through
  `rustls-native-certs`). `AWS_ENDPOINT_URL_STS` sends the web-identity token to the host that it
  names. Sven decided on 2026-11-22 to keep the proxy variables and to document them. The
  `SSL_CERT_*` roots are documented, not locked: Sven did not decide them.
- G6. The new dependency passes `:deny`, `:osv` and `:machete`, with no `aws-lc-rs`, no second
  `reqwest` major and no second `rustls` major.
- G7. A pinned S3-compatible test image, a fallback image, and a negative image for the boot probe.
- G8. `apply` ends within a deadline that is shorter than the lease TTL.

## 3. Non-goals

- The run loop and `RunCommitter` (SMA-790). This issue adds `MemoryCommit::External` and builds it
  at boot. SMA-790 calls `apply` in the run commit (SMA-789 §6.12).
- Data migration between backends. A backend switch shows an empty store (SMA-789 §6.8).
- Atomic or concurrent-safe guards and caps on S3. They are best effort (§6.7).
- A conditional `DELETE` request. `object_store` has none (§5). Delete is read-then-delete.
- Multipart upload. A memory file is at most 64 KiB by default.
- Versioned buckets. The probe refuses them (§6.8).
- Bucket creation and lifecycle rules. The operator makes the bucket.
- The state of the S3 store on `/readyz`. The boot probe is the only check.
- Shared-credential files (`~/.aws/credentials`, `AWS_PROFILE`). `object_store` does not read them.

## 4. Anti-goals

The design fails, even if it works, when any of these is true:

- AG1. A change to `conformance.rs` is needed to make S3 pass.
- AG2. A stale `Put` overwrites a newer file on S3 (a lost update). This includes the ABA case:
  delete, then re-create with the same content. **Limit:** a stale `Delete` can remove a newer file
  in the race of §6.7 item 3. This is accepted and documented.
- AG3. A partial failure deletes a file whose new copy was not written (for example, a `rename` that
  loses its file). **Limit:** the same race of §6.7 item 3.
- AG4. The service starts on a store that ignores `If-None-Match` or `If-Match`.
- AG5. A credential is read from the config file, or is written to a log or an error text.
- AG6. A key of one scope is visible in another scope, or a listing of `/a` returns a key under `/ab`.
- AG7. `aws-lc-rs` enters the dependency tree.
- AG8. A log line, a metric label or a `Store` error text holds a memory path, an object key or
  memory content (SMA-789 §6.13).
- AG9. `delete_scope` returns `Ok` while an object of the scope remains.

## 5. Approaches

**The client crate** (Sven picked 1 at Stage 1, 2026-11-22):

| | Approach | Trade-off | Result |
|---|---|---|---|
| 1 | `object_store` 0.14.2, `default-features = false`, features `aws-base`, `reqwest`, `ring`. | Gives SigV4, credential providers, `ListObjectsV2`, retries, `PutMode::Create` and `PutMode::Update(e_tag)`. No conditional delete. | **Chosen.** |
| 2 | `rusty-s3` (signing only) plus our own `reqwest` calls. | Can send `If-Match` on `DELETE`. We must write the HTTP calls, the XML parser, retries, error mapping and the credential chain. | Rejected: twice the code to close one documented race. |
| – | `aws-sdk-s3`. | Brings `aws-lc-rs` and `rustls` 0.21 next to 0.23. | Rejected by the hazards. |

**Correction to SMA-789 §6.8:** it names `object_store` "feature `aws`". The `aws` feature hard-wires
`reqwest/rustls` and `aws-lc-rs` (`object_store-0.14.2/Cargo.toml` lines 44-49), so it fails
`:deny`. The features above are the measured ones.

**The ABA fix** (Sven picked A at Stage 1, 2026-11-22):

| | Approach | Trade-off | Result |
|---|---|---|---|
| A | Each object starts with a fixed-length header line that holds a random write ID. Each write then gets a new ETag. | The suite stays unchanged. A version never repeats. A write with an unknown outcome can be checked exactly. Objects in the bucket are not plain Markdown. | **Chosen.** |
| B | Change `aba_never_matches` to skip S3. | Breaks "unchanged". A version can repeat on S3. | Rejected. |

### 5.1 The hazard results (measured 2026-11-21)

Measured in a scratch crate with the workspace `reqwest` 0.13 (`rustls-no-provider`, `json`, `form`)
and `rustls` 0.23 (`ring`, `std`), `cargo tree` and `cargo-deny` 0.21.0. It resolved to `reqwest`
0.13.5, `rustls` 0.23.46 and `hyper` 1.12.0.

| Hazard | `object_store` 0.14.2 with `aws-base`, `reqwest`, `ring` |
|---|---|
| 1. `aws-lc-rs` (banned in `rs/deny.toml`) | Absent. `cargo tree -i aws-lc-rs` finds nothing. |
| 2. A second `reqwest` or `rustls` major | None. It uses `reqwest` 0.13.5 and `rustls` 0.23.46. |
| 3. Private-CA roots | `ClientOptions::with_root_certificate` **ADDs** to the platform roots (`client/mod.rs:895-905`, `tls_certs_merge`), unless `disable_system_certificates` is set. §6.8 never sets it. |
| Licenses (`cargo deny check licenses`) | Pass. |

The scratch crate had `reqwest` as a normal dependency. The agent host has it only in
`[dev-dependencies]`, and `object_store`'s `reqwest` feature turns on no TLS feature. So §8 makes
`reqwest` a normal dependency, and the plan re-measures hazards 1 and 2 with
`cargo tree -p paigasus-agent-host -e normal` in the real workspace.

## 6. Design

### 6.1 Where the code goes

- `rs/crates/services/paigasus-agent-host/src/adapters/memory_s3/`, a new adapter module. The
  builder picks the files inside it. A sensible split: `mod.rs` (the store), `keys.rs`, `envelope.rs`,
  `plan.rs` (the pure pre-check), `errors.rs` (the error classes of §6.4), `probe.rs` and
  `client.rs` (the `AmazonS3` build).
- `S3MemoryStore` holds an `Arc<dyn object_store::ObjectStore>`, the validated key prefix and the
  `apply` deadline. Production passes an `AmazonS3`. Tests pass `object_store::memory::InMemory` or a
  wrapper over it (§7). The adapter calls only `ObjectStore` methods outside `client.rs`.
- The core crate does not change. `conformance.rs` does not change (AG1).

### 6.2 Keys

- A key is `<prefix>/<org>/<agent>/<principal>/<path without the leading />`. UUIDs are hyphenated
  and lower case. Example: `mem/0b1e…/7c2d…/a91f…/tasks/t1.md`.
- **The prefix** is validated when the config is validated (§6.9), so a bad prefix stops the boot:
  1 to 8 segments separated by `/`; each segment matches `[A-Za-z0-9._-]{1,64}` and does not start
  with `.`; no leading `/`, no trailing `/`, no empty segment; at most 128 bytes. So `..`, `.` and
  `%` cannot occur. **The prefixes of two deployments on one bucket must not nest** (for example
  `mem` and `mem/<uuid>`). The README says this.
- **Listing.** The adapter lists an `object_store::Path` of the scope or the directory. `object_store`
  adds the `/` itself (`client/list.rs:70-72`), so a listing of `/a` asks for `…/a/`. The adapter
  does not trust that: it requires each listed key to start with `<scope prefix>/` (and, for a
  directory, `<dir prefix>/`) and strips it with `strip_prefix`. A key that does not start with it is
  a store fault: a `Store` error with a fixed text (AG6).
- **Key to path.** The rest of the key, with a leading `/`, must parse as a `MemoryPath`. A key that
  `object_store::Path` accepts and `MemoryPath` refuses (for example a segment of 65 characters, or a
  `~`) is skipped and counted with `reason = key` (§6.11). So it cannot break `view`.
- **A key that `object_store::Path` refuses** (an empty segment `a//b`, a `.` or `..` segment, a
  control character) fails the whole listing page inside `object_store` (`client/s3.rs:82`), and the
  stream ends. The adapter cannot skip it. So such a key makes the scope unreadable: `list`, `usage`,
  `apply` and `delete_scope` return `Store` with a fixed text, and the error is counted with
  `reason = unparsable`. The README gives the repair step: remove the key with an S3 tool.
- **A folder marker** such as `notes/` arrives as `…/notes`, because `Path::parse` removes a trailing
  `/` (`path/mod.rs:186`). Its object is empty, so it is skipped with `reason = size` (§6.3).
- `list` sorts its result in byte order of the path. It does not trust the order of the store.
- **The probe keys** are under `<prefix>/_probe/<random UUID>`. A scope key has a UUID in that
  segment, so the two never meet.
- **Assumption: a flat key namespace.** The port lets one set hold a file `/notes` and a file under
  `/notes/` at the same time, for a short time (for example `[Put /notes/x, Delete /notes]`, the puts
  first). A store over a file system (the `versitygw` posix backend, MinIO-like stores)
  cannot hold both. Measured: `rustfs` 1.0.6 does hold a file and a directory of one name (probe
  step 7 is true, and `[Put /notes/x, Delete /notes]` gives `Ok`). `versitygw` v1.11.0 posix does
  not (`file_and_dir_of_one_name` is false). On such a store that set fails at the `Put`, and nothing is lost (the `Delete` is not sent).
  The set then fails on every retry. The probe tests this (§6.8 step 7) and logs a `warn` with the
  known limit. It does not refuse the boot, because the case is rare and loses no data.
- **Assumption: strong read-after-write consistency** of `GET`, `HEAD` and `LIST`. AWS S3 gives it.
  The pre-check depends on it. The README says this.

### 6.3 The object format (the envelope)

- An object is a header line, then the file content, as UTF-8 bytes:
  `paigasus-memory/1 <write-id>\n<content>`. The write ID is a new random UUID v4 in the 32-character
  simple form (lower-case hex, no hyphens).
- The header is always 51 bytes: 18 for `paigasus-memory/1 `, 32 for the ID, 1 for `\n`. The
  constant `HEADER_LEN = 51` is asserted in a test. **It is a stored-format constant:** `list`
  computes the size without a `GET`, so a later format version must keep 51 bytes.
- Each write has a new write ID, so the bytes differ, so the ETag differs. **The version is the
  ETag**, as the store returns it. It is opaque. The same content written twice gets two versions
  (G2, AG2). The adapter needs only "different bytes give a different ETag". The probe checks it on
  every boot (§6.8 step 3).
- `size_bytes` of a file is the object size minus `HEADER_LEN`. An object of fewer than 51 bytes is
  not a memory file: `list` skips it and counts it with `reason = size`.
- `get` checks the header: the exact prefix, 32 hex characters, then `\n`, and UTF-8 content.
- **A foreign object** is an object under a scope prefix that the adapter did not write (an operator
  upload of a plain `instructions.md`, for example). The rule is one rule for every operation: **a
  foreign object is an operator fault.** `get` returns `Store` with the fixed text "object is not a
  memory file" and counts `reason = header`. `list` cannot see the header and shows an object of 51
  bytes or more with a wrong size. `apply` on that path gets a refusal, then the write-ID `GET`
  (§6.5) finds a bad header, and the result is `Store`. The README says: do not write objects under
  the prefix; to repair, delete the object with an S3 tool. A run whose run-start file is foreign
  fails before the model starts (SMA-789 §6.10). This is accepted: the alternative rules make `list`,
  `get` and the pre-check disagree.
- `Content-Type` of each object is `text/plain; charset=utf-8`. This is for operators only.
- **Version form.** The plan proves on the test image that the version from `list`, `get` and
  `head` is byte-equal for one object. If a store differs, the adapter normalizes in one function. A
  test pins the chosen form. Probe step 4 checks the listed form against `If-Match` at every boot.

### 6.4 Reads and errors

- `get(scope, path)`: `GET` the key. Not found is `Ok(None)`. The version is the ETag of the
  response. A missing ETag is `Store`.
- `list(scope, dir)`: list the directory (§6.2), at every depth. Each entry gives
  `MemoryEntry { path, size_bytes, version }` from the listing metadata, with no `GET` per file.
  A missing ETag in the listing is `Store`.
- `usage(scope)`: list the scope. `files` is the count and `bytes` is the sum of `size_bytes` of the
  entries that `list` would return. It is O(files). The default cap of 512 files keeps it to one
  listing page of 1,000 keys.
- **Error texts (AG8).** The `Display` text of an `object_store::Error` holds the object path, and a
  request error holds the request URI (`aws/client.rs:479-481`, `client/retry.rs:52-56`). The memory
  tool logs every `Store` cause (`adapters/helikon/memory_tool.rs:123-125`). So the adapter never
  uses that text. One function maps an `object_store::Error` to a fixed class text: `not_found`,
  `precondition`, `already_exists`, `permission_denied`, `unauthenticated`, `not_implemented`,
  `status=<NNN>`, `timeout`, `transport`, `unparsable_key`, `other`. Every `Store(String)` that the
  adapter returns holds only such a class text and a fixed sentence.
- **Refusal variants.** A refused `Create` arrives as `Error::AlreadyExists` (412, and 409,
  `aws/mod.rs:265-270`, `client/retry.rs:171`). A refused `Update` arrives as `Error::Precondition`
  (412; a 404 also maps to it, `aws/mod.rs:295-296`), or as `AlreadyExists` when the 409 retries run
  out (`client/retry.rs:171, 411`). The adapter treats all three as "refused".

### 6.5 `apply`

`apply(set, caps)` has three phases: refusals with no IO, a pre-check, then the writes. The whole
call runs under one deadline (§6.9, `apply_deadline_secs`).

**Phase 0: the empty set.** `set.is_empty()` returns `Ok(())` with no request.

**Phase 1: the pre-check.** One listing of the scope gives a snapshot: path to
`(size_bytes, version)`. Then a **pure function** checks the set against the snapshot, in the order
of the fake (`fake.rs:94-141`), so every adapter reports the same refusal:

1. **Guards** (SMA-843): for each guard in `BTreeSet` order, the snapshot paths that the guard
   covers (`MemoryDir::covers`), in byte order. The first one that no change of the set names is
   `Conflict { path }`. So a guard conflict wins over every other refusal.
2. **Each change, in the order of the set, all its checks before the next change.** For a `Put`,
   the order is the order of `fake.rs`: first the NUL check (U+0000 is `Store`, as
   `ports.rs:177-180` asks), then the version check, then `caps.check_file(path, size)`. A
   `Delete` has only the version check. The version check: `Put { Absent }` conflicts when the
   snapshot holds the path; `Put { Version(v) }` and `Delete { v }` conflict when the snapshot
   version is not `v` or the path is absent. A test pins the order (for
   `[Put /a conflict, Put /b NUL]` the result is `Conflict { /a }`, as in the fake and Postgres).
3. **The store caps:** the function computes `after` from the snapshot and the set (an overwrite
   subtracts the old size, a `Delete` subtracts the file) and runs `caps.check(before, after)`.

The function returns the write plan or the error. It does no IO, so unit tests cover it with
tables. A pre-check refusal writes nothing. This is why `apply_is_all_or_nothing` and every "nothing
changed after a refusal" check of the suite pass on S3.

**Phase 2: the writes.** First the `Put` phase, then the `Delete` phase. Inside one phase the writes
touch distinct paths, so they run with a bounded concurrency of 8 (a constant). The `Delete` phase
starts only after every `Put` succeeded.

- `Put { Absent }`: `PUT` with `PutMode::Create` (`If-None-Match: *`).
- `Put { Version(v) }`: `PUT` with `PutMode::Update(UpdateVersion { e_tag: Some(v), version: None })`
  (`If-Match: v`).
- **Every `Put` error is checked** (not only a refusal). `object_store` sends a conditional PUT as not
  idempotent (`aws/client.rs:518`), so it does not retry it after a timeout (`client/retry.rs:440-
  446`), and a store may have written it. It does retry a PUT after a 5xx, a 429 or a 408, and the
  first attempt may have landed. So after any `Put` error the adapter `GET`s the object:
  - Its header holds the write ID of this `Put`: the `Put` **succeeded**.
  - It holds another write ID, or the object is absent, and the error was a refusal: a real
    `Conflict`. The store did not write this `Put`.
  - It holds another write ID or is absent, and the error was not a refusal: the `Put` **failed**,
    and the store did not write it. **Ruling F1:** when the class is `timeout` or `transport` and
    the object is absent, the outcome is **unknown**, not failed. The server can commit the write
    after the client gave up. The result is `Partial { applied: 0 }`. **Ruling F3:** an update
    (`Put { Version(v) }`) of class `timeout` or `transport` whose object holds another write ID is
    also **unknown**, because its `If-Match` `PUT` can still land on that object.
  - The `GET` itself fails, or the object is foreign: the outcome is **unknown**.
- `Delete { v }`: `HEAD` the key. Absent, or an ETag that is not `v`, is `Conflict`. Else `DELETE`
  (a single-object `DELETE`; §6.8 turns off bulk delete). A `DELETE` error is checked with a
  `HEAD`: absent means the delete succeeded; present with `v` means it failed; a failed `HEAD` is
  unknown. **Ruling F1:** when the class is `timeout` or `transport` and the `HEAD` shows `v`, the
  outcome is unknown, not failed.
- **The result.** `confirmed` is the count of writes that succeeded.
  - When every write succeeded: `Ok(())`.
  - When a write failed and `confirmed` is 0 and no outcome is unknown: the error itself
    (`Conflict { path }` or `Store`). **The store holds nothing of the set.**
  - Otherwise: `Partial { applied: confirmed, failed_at, cause }`. `failed_at` is the path of the
    first failed or unknown change in the order of the set. **`Partial` means "the store may hold
    part of the set".** It also covers `confirmed == 0` with an unknown outcome. `apply` counts it
    (§6.11). It does not try to undo the writes.
  - With concurrency, more than one write can fail. `failed_at` and `cause` are those of the first
    in the order of the set. The other writes of the phase still finish; no new write starts.
- **The deadline.** When the deadline expires, no new write starts, the writes in flight are
  dropped, and their outcome is unknown. The result follows the rules above (the cause is a `Store`
  error with the class `timeout`). Before the first write, an expired deadline is a bare `Store`.
- **Puts first.** A `rename` is a `Put` of the new path and a `Delete` of the old path. When the
  `Delete` fails, both files exist. When the `Put` fails, the `Delete` is not sent. So a partial
  failure can leave a duplicate, but it never loses a file (G4, AG3). This is at-least-once, as
  ADR-0028 (b) allows for S3.

**The contract for SMA-790** (SMA-789 §6.12, refined): a bare `Conflict`, `CapExceeded` or `Store`
means that the store holds nothing of the set. `Partial` means that it may hold part of it. In both
cases SMA-790 logs, counts and commits the run without the rest of its memory changes, as §6.12
says. The worst case of one `apply` is `apply_deadline_secs`.

### 6.6 `delete_scope`

1. List every key under the scope with a raw listing, **including the keys that `list` skips**.
2. Delete each key with no condition (single-object `DELETE`, concurrency 8).
3. List the scope again. If any key remains, return `Store` (AG9). This also catches a write that
   came during the erasure.

A key that `object_store::Path` refuses makes step 1 fail, so `delete_scope` returns `Store` and the
operator must remove the key (§6.2). A folder marker `…/notes/` also stays: `object_store` strips
the trailing `/`, so the delete misses it, and step 3 returns the "keys remain after the scope
delete" `Store` error (measured on `rustfs` 1.0.6). The README repair step applies. The operation is not atomic. A second call finishes it,
because a delete of a missing key succeeds, except for a folder marker.

### 6.7 The races (stated in the adapter doc and the README)

The adapter is correct for one writer at a time. These races remain when two writers (a run commit
and the console, for example) write one scope at the same moment:

1. **The guard rule** (SMA-843 §6.6). A writer can add a file under a guarded directory between the
   pre-check and the writes. The set then succeeds, and the new file stays.
2. **The store caps.** Two writers can both pass the cap check. The store can then go over a cap.
   A later change that makes the store larger is refused; a change that makes it smaller passes.
3. **The delete.** The `HEAD` and the `DELETE` are two requests. A writer that changes the object
   between them loses its change: the delete removes the newer object. In a `rename`, the new path
   then holds the old content. This is the limit of AG2 and AG3.
4. **Partial sets.** A conflict between the pre-check and the writes gives `Partial`.
5. **The erasure.** A write that lands after the last listing of `delete_scope` survives. Step 3 of
   §6.6 catches a write that lands before the second listing.

A `Put` has no race: the store checks its condition. So a stale `Put` never overwrites (AG2).

### 6.8 The client and the boot probe

**The client** (`client.rs`). It is one function that takes the config and the environment as a
parameter (`impl IntoIterator<Item = (String, String)>`), so tests can pass an environment without
`std::env::set_var` (which is `unsafe` in edition 2024). Production passes `std::env::vars_os()`,
filtered to UTF-8 pairs (`std::env::vars()` panics on a non-UTF-8 variable).

- It installs the `ring` provider (`rustls::crypto::ring::default_provider().install_default()`,
  the result ignored) before it builds the client, as the workspace requires.
- **It does not call `AmazonS3Builder::from_env()`.** `from_env` copies every `AWS_*` variable into
  the builder (`aws/builder.rs:606-618`), so `AWS_ALLOW_INVALID_CERTIFICATES`,
  `AWS_DISABLE_SYSTEM_CERTIFICATES`, `AWS_SKIP_SIGNATURE`, `AWS_ENDPOINT_URL_S3` (which wins over a
  set endpoint, `aws/builder.rs:1288`) and others would change the client in silence. The function
  starts from `AmazonS3Builder::new()` and copies **only** this allowlist from the environment:
  `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_WEB_IDENTITY_TOKEN_FILE`,
  `AWS_ROLE_ARN`, `AWS_ROLE_SESSION_NAME`, `AWS_ENDPOINT_URL_STS`,
  `AWS_CONTAINER_CREDENTIALS_RELATIVE_URI`, `AWS_CONTAINER_CREDENTIALS_FULL_URI` and
  `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE`. Every other setting comes from the config (§6.9).
- It sets `with_conditional_put(S3ConditionalPut::ETagMatch)` explicitly. This is also the default
  of `object_store`; the explicit call documents it.
- It sets `disable_bulk_delete = true`, so a delete is a single `DELETE /key`. Some S3-compatible
  stores do not have `POST /?delete` (`aws/mod.rs:340-350`).
- It builds one `ClientOptions` from nothing and calls `with_client_options` once: request timeout
  10 seconds, connect timeout 5 seconds, `allow_http` from the config, and the CA bundle. The retry
  config is set explicitly: at most 3 retries, a retry timeout of 10 seconds. These are constants.
- The CA bundle (§6.9) is read with `object_store::Certificate::from_pem_bundle`. An empty bundle is
  a boot error. Each certificate goes to `ClientOptions::with_root_certificate`, which ADDs it.
- **Credentials.** With none of the allowlisted variables, `object_store` uses the instance metadata
  endpoint (`aws/builder.rs:1242-1256`). Off AWS, the probe then fails after the request timeout with
  an `unauthenticated` or `timeout` class. The README says this.

**The boot probe** (`probe.rs`) runs once, when the backend is `s3`, before the service installs
its router (`main.rs` `boot()`, the step after the NATS checks). The key `K` is
`<prefix>/_probe/<random UUID>`. Contents `A` and `B` have the same length.

1. `PUT K = A` with `PutMode::Create`. It must succeed. **If its result holds a version ID**
   (`PutResult::version`, from `x-amz-version-id`, `aws/client.rs:61, 488`), the bucket keeps old
   versions: refuse (G3). A delete in a versioned bucket keeps the data, so `delete_scope` could not
   erase it.
2. `PUT K = A` with `PutMode::Create`. It must be refused. A success means that the store ignores
   `If-None-Match`.
3. `PUT K = B` with `PutMode::Update(etag(A))`. It must succeed, and `etag(B)` must differ from
   `etag(A)`. Equal ETags for different content of one length mean that the version can repeat (an
   `mtime-size` ETag, for example).
4. `PUT K` with `PutMode::Update(etag(A))` again. It must be refused. A success means that the
   store ignores `If-Match`. (A real stale ETag, not a made-up one: a store can refuse a made-up
   ETag for its form and still accept a stale one.)
5. List `<prefix>/_probe` and find `K`. Its listed ETag, its `GET` ETag and its `HEAD` ETag must be
   equal to `etag(B)` after the normalization of §6.3. Then `PUT K` with `PutMode::Update` and the
   listed ETag. It must succeed.
6. `DELETE K`, then `HEAD K`. It must be absent. A failure is a boot error: the delete path and the
   `s3:DeleteObject` permission must work.
7. `PUT <K>/x` and `PUT <K>` (both create-only), then delete both. If the second `PUT` fails, log a
   `warn`: "The memory store at {endpoint} cannot hold a file and a directory of one name. A memory
   change set that replaces a file with a directory of the same name will fail." This step does not
   refuse the boot (§6.2).

Every refusal of steps 1 to 6 fails the boot. The error text names the endpoint, the bucket and the
step, with a sentence for each step. For steps 2 and 4: "The memory store at {endpoint}, bucket
{bucket}, does not support conditional writes: it accepted a {create-only | conditional} put that it
must refuse. The S3 memory backend needs If-None-Match and If-Match on PutObject." A 501
(`not_implemented`) at step 1, 3 or 4 gets the same text. Any other failure (network, 403, no
bucket) fails the boot with the endpoint, the bucket, the step and the error class of §6.4. No text
holds a credential (AG5), and §6.9 refuses an endpoint with userinfo.

The probe proves the store's behavior once, at boot. A store that changes later is not detected.

### 6.9 Config

`MemoryBackend` gets `S3` (`"s3"`). It stays a `Copy` enum. The S3 settings are a new optional
section, `memory.s3`:

| Key | Type | Default | Notes |
|---|---|---|---|
| `memory.s3.bucket` | string | required | The S3 bucket-name rule: 3 to 63 characters of `[a-z0-9.-]`, starts and ends with a letter or digit, no `..`. |
| `memory.s3.prefix` | string | required | The rules of §6.2. |
| `memory.s3.region` | string | `"us-east-1"` | |
| `memory.s3.endpoint` | URL | none (AWS) | `http` or `https`, no userinfo, no query, no fragment. `http` needs `allow_http`. |
| `memory.s3.path_style` | bool | `true` when `endpoint` is set, else `false` | With `false` and an `endpoint`, the endpoint must already hold the bucket (`aws/builder.rs:1294`). The doc says this. |
| `memory.s3.allow_http` | bool | `false` | For local tests and dev only. |
| `memory.s3.extra_ca_bundle_path` | path | none | A PEM bundle. It **ADDs** to the platform roots. The `extra_` prefix marks the ADD side, per `rs/CLAUDE.md`. |
| `memory.s3.apply_deadline_secs` | integer | `10` | 1 or more, and less than `lease.ttl_secs` (default 30). |

- `Config::validate` refuses: `backend = "s3"` with no `memory.s3` section; a `memory.s3` section with
  `backend = "postgres"` (a silent ignore hides a typo); each rule of the table; an empty or
  whitespace-padded CA path (the IAM rule, `paigasus-iam/src/config.rs:1343-1348`). Each refusal is
  its own `ConfigError` variant.
- **No credential key exists in the config.** Credentials come only from the allowlist of §6.8.
- The figment chain is unchanged, so `AGENT_HOST_MEMORY__S3__BUCKET` and the other keys work from
  the environment. An `AWS_*` variable never changes a setting of this table.
- Docs: the `memory.*` rows of `services/paigasus-agent-host/README.md`, the `[memory]` block of
  `agent-host.toml.example`, and the CA knob list in `rs/CLAUDE.md` (it gets this knob as an ADD
  knob). The README says: a backend switch does not move data; the bucket must not keep versions;
  objects are not plain Markdown and an operator must not write under the prefix; prefixes must not
  nest; the store must be strongly consistent; which credential variables are read; the repair steps
  of §6.2 and §6.3; and the five races of §6.7 in one paragraph.
- The test `memory_backend_s3_is_refused_for_now` (`config.rs:858`) is replaced by tests of the new
  rules.

### 6.10 `MemoryCommit` and boot

- `enum MemoryCommit { InTransaction(PgMemoryStore), External(Arc<dyn MemoryStore>) }`
  (`adapters/memory_commit.rs`). `store()` returns the `Arc` for both. `Arc<dyn MemoryStore>` has no
  `Debug`, so `MemoryCommit` gets a manual `Debug` that prints only the variant name.
- `from_config` becomes `async fn build(cfg: &MemoryConfig, lease: &LeaseConfig, db:
  DatabaseConnection, env: impl IntoIterator<…>) -> Result<MemoryCommit, MemoryBootError>` (the
  builder picks the exact shape). For `s3` it builds the client, runs the probe and returns
  `External`. For `postgres` it does what `from_config` does now.
- `boot()` calls `MemoryCommit::build` and keeps the result in `AppState` (a new field), so the
  value has an owner and SMA-790 can take it from there. No route reads it in this issue. The
  builder makes sure that this adds no `dead_code` warning (the workspace denies warnings).
- **Merge risk:** SMA-790 is in progress in another worktree and may change `memory_commit.rs` and
  `AppState`. The plan rebases on `origin/main` before the PR.

### 6.11 Observability

Two counters in `paigasus-observability/src/names.rs`, added to `names::ALL` and described in
`describe_agent_host_metrics()`:

- `agent_host_memory_s3_partial_applies_total`: one per `Partial` result.
- `agent_host_memory_s3_skipped_objects_total`, label `reason` = `key` (a key that `MemoryPath`
  refuses), `size` (shorter than the header), `header` (a foreign object at `get`) or `unparsable`
  (a key that `object_store::Path` refuses). The HELP text says that it counts events at each read,
  not distinct objects.

A `Partial` result is logged at `warn` with `applied` and the error class of the cause. No log line,
label or error text holds memory content, a path or a key (AG8). The boot-probe error holds the
endpoint and the bucket, which are operator config, not principal data.

## 7. Tests

**Unit, no IO** (in the adapter module):

- The prefix table: valid; `..`; `.x`; a leading or trailing `/`; `a//b`; `%2e`; a space; 129 bytes;
  9 segments.
- The bucket and endpoint rules of §6.9, with userinfo, a query and `http` without `allow_http`.
- Keys: path to key and back; root; a listed key without the scope prefix is `Store`; a key that
  `MemoryPath` refuses is skipped; the probe key cannot parse as a scope key.
- The envelope: `HEADER_LEN == 51`; encode then decode; a short object; a bad prefix; non-hex ID; a
  missing `\n`; non-UTF-8 content.
- The pure pre-check: each row of §6.5 phase 1; the order (a guard conflict first; per change, the
  NUL check before the version check before the file cap; the store caps last), with
  `[Put /a conflict, Put /b NUL]`; the `after` usage of an overwrite, a delete and a create.
- The error classes: each `object_store::Error` kind, built with a path and a URI that hold
  `secret-name`, maps to a text that does not hold `secret-name` (AG8).
- The client: with an injected environment of `AWS_ENDPOINT_URL_S3`, `AWS_ENDPOINT`,
  `AWS_ALLOW_INVALID_CERTIFICATES=true`, `AWS_SKIP_SIGNATURE=true`,
  `AWS_DISABLE_SYSTEM_CERTIFICATES=true` and `AWS_CONDITIONAL_PUT=disabled`, the built client's
  `get_config_value` shows the config values, not the environment values. The allowlisted
  credential variables do reach the builder.
- Config: `s3` loads from TOML and from the environment; each refusal of §6.9; the `path_style`
  default; `apply_deadline_secs` against `lease.ttl_secs`.

**Adapter over `InMemory`, with test wrappers** (fast, no Docker):

- `conformance::run_all` passes over `InMemory`. This runs on every `cargo nextest` call.
- **A content-hash wrapper** gives each object an ETag that is a hash of its bytes, as AWS does.
  (`InMemory` takes each ETag from one counter, `memory.rs:209-217`, so it hides an ABA bug.)
  `conformance::run_all` passes over it too. This is the no-Docker proof of the write ID.
- **An interleaving wrapper** runs a hook just before it forwards the first `PUT` or `DELETE`: in the
  hook, "another writer" writes the same path. For `Put { Absent }`, `Put { Version }` and `Delete`:
  the result is `Conflict` (or `Partial` for a later write), and the other writer's content is still
  there for both `Put` forms. For `Delete` the test documents race 3 of §6.7: the newer file is
  deleted. This proves that the store-side condition is sent.
- **A fault wrapper** that fails or lies on chosen calls:
  - The second write of `[Put /a, Put /b, Delete /c]` fails: `Partial { applied: 1, failed_at: /b }`,
    `/a` is new, `/c` still exists, and no `Delete` was sent.
  - A rename `[Put /new, Delete /old]`: with the `Put` refused, the error is bare and `/old` still
    exists. With the `Delete` failing, `Partial`, and both exist.
  - A `Put` that writes and then returns a refusal (a retried request): `Ok`.
  - A `Put` that writes and then returns a timeout-class error: `Ok` (the `GET` finds the write ID).
  - A `Put` that writes, returns a timeout, and then the check `GET` fails: `Partial` with
    `applied: 0` (unknown outcome).
  - A `Put` that does not write and returns a timeout: `Partial` with `applied: 0` (ruling F1,
    unknown outcome). A `Put` that does not write and returns a definite failure (class `other`):
    the bare `Store` error. A `Delete` that returns a timeout and whose check `HEAD` shows the
    expected ETag: `Partial` with `applied: 0`.
  - The deadline: a wrapper that stalls the second `PUT`; with a short deadline the result is
    `Partial` with the class `timeout`, within the deadline plus a small margin.
  - The probe: a store that accepts every conditional put fails at step 2; a store that refuses only
    a wrong `Create` fails at step 4; a store whose ETag is `size` only fails at step 3; a store that
    returns a version ID fails at step 1; a store that refuses `Update` with its listed ETag fails at
    step 5; a store whose delete does nothing fails at step 6; a store that cannot hold `K` and `K/x`
    passes with the `warn`.
  - `Partial` increments `agent_host_memory_s3_partial_applies_total`, and a skipped key increments
    the skipped counter with its `reason` (a `DebuggingRecorder` test that reads the number, not
    the `# TYPE` line).

**Container, through `paigasus_test_docker::start_or_skip`** (the `iam-docker-policy-single-site`
rules: no `AsyncRunner` import, no `CI` read). The new test binary goes into the `docker-containers`
group and the agent-host filter of `rs/.config/nextest.toml` (line 174).

- **Primary image: `rustfs/rustfs:1.0.6`** (Apache-2.0, about 433 MB, multi-arch index digest
  `sha256:ee717072290bec9b11abfc0365f8310b00df1cd9b3639943edae47bdef05044a`). Sven chose it at
  Gate 1. Measured 2026-11-21 on Docker Desktop: `If-None-Match: *` and a wrong `If-Match` both give
  412. The plan records the start command, the credential variables and the readiness check. The
  readiness check is a successful signed `ListBuckets`, with the `mapped_port` budget. The test
  makes the bucket.
  - `S3MemoryStore` over the real client passes `conformance::run_all`.
  - The boot probe passes. The test records the result of step 7 (a file system store is expected
    to log the `warn`).
  - The raw storage check: after a `create` of `/tasks/t1.md`, the object key is
    `<prefix>/<org>/<agent>/<principal>/tasks/t1.md`, and its bytes start with the header.
  - The version from `list`, `get` and `head` is equal for one object (§6.3).
  - Raw keys written with a signed request: a folder marker `…/notes/` is skipped by `list`. It is NOT
    removed by `delete_scope` (`object_store` strips the trailing `/`). `delete_scope` returns the
    "keys remain after the scope delete" `Store` error, and the README repair step applies
    (measured on `rustfs` 1.0.6); a key `…/a//b` (if the store accepts it) makes `list` return the
    `Store` error of §6.2. The test records what the store does.
  - `[Put /notes/x, Delete /notes]` with `/notes` present: the documented result of §6.2.
  - The credentials reach the client through the injected environment, not `set_var`.
- **Negative image: `versity/versitygw:v1.0.17`** (digest
  `sha256:a13b9adbda0ea9d4b1d929e4bed327e4eccc18d13c23f7e5d192d987dde5651d`). Measured: it answers
  200 to a second create-only put and to a wrong `If-Match`. Started as `--port :7070 posix <dir>`
  with `ROOT_ACCESS_KEY` and `ROOT_SECRET_KEY`. The test builds `MemoryCommit` for `s3` against it
  and asserts the boot error of §6.8 step 2. No `rustfs` version that ignores the conditions is
  known, so the negative test stays on `versitygw`.
- **Fallback image: `versity/versitygw:v1.11.0`** (Apache-2.0, posix backend, about 112 MB, digest
  `sha256:8319a530d08d537426e2f855bae5280709795dc73a3f0c347973ef915d6343c5`). Started as
  `--port :7070 posix <dir>` with `ROOT_ACCESS_KEY` and `ROOT_SECRET_KEY`, with the directory inside
  the container (for xattrs). Measured: `If-None-Match: *` gives 200 then 412, `If-Match` gives 412
  on a wrong ETag and 200 on the right one. One test runs the conformance suite and the probe on it
  **only when** `PAIGASUS_S3_FALLBACK=1` is set. When the variable is not set, the test prints that
  it skipped and passes. The plan runs it once locally and records the result. If `rustfs` breaks,
  the switch is not one constant: change `start_rustfs()` to `start_versitygw(...)` and replace the
  `rc`-based raw-key helper (the module doc of the test file says how).
- **The CI runner is not measured yet.** The image measurement ran on Docker Desktop only, and the
  repo knows a storage difference between Docker Desktop and a runner (memory
  `docker-desktop-store-vs-ci-runner`). The first CI run of the PR is that measurement. If `rustfs`
  fails there, the plan switches to the fallback. The `rustfs` image is about 433 MB, so the first
  pull adds time to the CI job.
- MinIO is not used. `github.com/minio/minio` is archived (README: "THIS REPOSITORY IS NO LONGER
  MAINTAINED"), and `hub.docker.com/v2/repositories/minio/minio/` returned 404 on 2026-11-21.
- Images are pinned by tag in the Rust tests, as the repo does now. The digest is in a comment next
  to each tag.

**Mutation proof** (the repo rule "red-first is not proof — delete the feature"). Each row removes
one feature and names the test that must go red. Each mutation must compile (`-D warnings` makes
dead code an error; use `--no-fail-fast`).

| Removed or changed | Must fail |
|---|---|
| The write ID (a fixed header) | `aba_never_matches` over the content-hash wrapper and on `rustfs` |
| The pre-check | `apply_is_all_or_nothing` and the guard checks |
| The guard step of the pre-check | the SMA-843 guard checks |
| `PutMode::Update` → `Overwrite` | the interleaving test for `Put { Version }` |
| `PutMode::Create` → `Overwrite` | the interleaving test for `Put { Absent }` |
| The `HEAD` ETag compare of a delete | the stale-delete pre-check test over the fault wrapper (a delete whose `HEAD` shows another ETag) |
| Puts before deletes | the rename fault test |
| The write-ID check after a `Put` error | the retried-put and the timeout fault tests |
| Unknown → `Partial` | the "check `GET` fails" fault test |
| The deadline | the deadline fault test |
| Each probe step 1 to 6 | its probe fault test, and the `v1.0.17` test for step 2 |
| The env allowlist (`from_env()` instead) | the injected-environment client test |
| The error-class mapping (`e.to_string()` instead) | the `secret-name` test |
| The second listing of `delete_scope` | a fault test whose delete does nothing |

**Gates:** `:deny`, `:osv`, `:machete`, `:fmt`, `:lint`, `:test`, `:build` (it builds without the
dev features, which is where a missing `reqwest` would show), and the full `ci-targets` list of the
root `CLAUDE.md` before the push.

## 8. Constraints

- Edition 2024, the workspace `rust-version`, SPDX headers, the hexagonal layout of the service
  crate.
- New workspace dependency: `object_store = { version = "0.14.2", default-features = false,
  features = ["aws-base", "reqwest", "ring"] }`.
- `reqwest = { workspace = true }` and `rustls = { workspace = true }` move to the agent host's
  `[dependencies]`, **always**. `reqwest` turns on the rustls backend that `object_store` needs
  (`tls_certs_merge` exists only with `reqwest`'s `__tls`). If the code does not name `reqwest`, add
  it to `[package.metadata.cargo-machete] ignored` with a comment.
- `rs/deny.toml` is not changed. Its `aws-lc-rs` ban is the control against a later switch to the
  `aws` feature.
- `conformance.rs` and the core crate are not changed.

## 9. Left to the builder

- The file layout inside `adapters/memory_s3/`.
- The timeout, retry and concurrency constants of §6.5 and §6.8, with a reason if they change.
- How the ETag form is normalized, if the measurement of §6.3 shows a difference.
- How `AppState` holds `MemoryCommit` without a `dead_code` warning, and the exact signature of
  `MemoryCommit::build`.
- How the test makes the bucket and writes raw keys (a signed `reqwest` call, or a start option of
  the image).

## 10. Calls I made (for Sven to check)

- The probe has seven steps, not two create-only puts: it also tests `If-Match` with a real stale
  ETag, ETag uniqueness, the listed ETag form, the delete path and versioning (§6.8).
- A versioned bucket fails the boot (§6.8 step 1).
- A store that cannot hold a file and a directory of one name only logs a `warn` (§6.2, step 7).
- A foreign object under the prefix is an operator fault with a fixed `Store` error, not a skip
  (§6.3).
- `apply` returns the bare error only when the store holds nothing of the set for sure. An unknown
  outcome is `Partial`, also with `applied: 0` (§6.5).
- `apply` has a deadline (`memory.s3.apply_deadline_secs`, default 10 s, below the lease TTL) and a
  write concurrency of 8 inside each phase (§6.5).
- The client copies only an allowlist of AWS credential variables; `from_env()` is not used (§6.8).
- Bulk delete is off (§6.8).
- `delete_scope` deletes the keys that `list` skips and lists again at the end (§6.6).
- `path_style` defaults to `true` when an `endpoint` is set (§6.9).
- A `memory.s3` section with `backend = "postgres"` is a config error, not ignored (§6.9).
- The CA knob ADDs only. No REPLACE option is offered (§6.9).
- The fallback image test runs only with `PAIGASUS_S3_FALLBACK=1` (§7).
- The `InMemory` conformance runs are in the default test run, so the S3 logic has a test with no
  Docker (§7).
- A conditional delete through a tombstone (`PutMode::Update` to a short object) is not used. It
  would close race 3 of §6.7, but tombstones would stay until `delete_scope`, and `list`, `usage` and
  the caps would need a tombstone rule (§12).

## 11. Risks

- **A store with a different ETag form.** The probe step 5 checks the listed, `GET` and `HEAD` forms
  and an `If-Match` with the listed ETag at every boot.
- **`rustfs` on the CI runner** is not measured yet (§7). The fallback (`versitygw` v1.11.0) is ready.
- **`object_store` updates.** A later version can change its error mapping, its retry rules or
  `from_env`. The fault-injection and injected-environment tests pin the behavior that the adapter
  needs.
- **Objects are not plain Markdown.** An operator who reads the bucket sees the header line. The
  README says this.
- **The commit holds the lease row while `apply` runs** (SMA-789 §6.12). The deadline bounds it at
  10 seconds by default. SMA-790 must know this number.

## 12. Stage 2 challenge (2026-11-22)

The `spec-challenger` (Opus) gave **APPROVE WITH CHANGES**: 2 blockers, 9 majors, 11 minors and 5
questions. It walked all checks of `run_all` and found that each one passes with the pre-check, the
51-byte envelope and puts-first. I checked the main `object_store` claims in the source before I
changed the design.

**Folded in:**

| Finding | Change |
|---|---|
| BLOCKER: `from_env()` copies every `AWS_*` variable; the config does not win | §6.8: no `from_env()`, a credential allowlist, an injected environment, one `ClientOptions`; §7 test |
| BLOCKER: a key that `Path` refuses ends the listing; a folder marker is not erased | §6.2: `Store` with a fixed text and `reason = unparsable`; §6.6: a second listing; §7 raw-key tests |
| MAJOR: an unknown write outcome was reported as "nothing written" | §6.5: a check after every `Put` and `Delete` error; unknown is `Partial`; the SMA-790 contract |
| MAJOR: no test reaches the store-side condition | §7: the interleaving wrapper and three mutation rows |
| MAJOR: two mutation rows could not go red | §7: the content-hash wrapper; the injected-environment test |
| MAJOR: `reqwest` must be a normal dependency; the CA type was wrong | §5.1, §6.8, §8; `:build` in the gates |
| MAJOR: a `Store` cause can hold a key, and the tool logs it | §6.4: fixed error classes; AG8; §7 test |
| MAJOR: the probe misses three failure classes | §6.8: steps 3 to 6 |
| MAJOR: a versioned bucket defeats the erasure | §3, §6.8 step 1: refuse |
| MAJOR: a foreign object makes `list`, `get` and `apply` disagree | §6.3: one rule (an operator fault) |
| MAJOR: a posix store cannot hold a file and a directory of one name | §6.2 assumption; §6.8 step 7 (`warn`); §7 test |
| MAJOR: the delete race breaks AG2 and AG3 as worded | §4: the limit is stated; §6.7 item 3 |
| MAJOR: `apply` has no time limit inside the run transaction | §6.5 deadline and concurrency; §6.9 `apply_deadline_secs`; G8 |
| MINOR: name the refusal variants | §6.4 |
| MINOR: AG6 depends on `object_store` | §6.2: `strip_prefix`, a mismatch is `Store` |
| MINOR: probe step 4 listed nothing | §6.8 step 5 lists `<prefix>/_probe` |
| MINOR: `Debug` of `MemoryCommit` | §6.10 |
| MINOR: refusal order differs from the fake | §6.5 phase 1 and its test |
| MINOR: weak config validation | §6.9: the bucket rule, no userinfo, the `path_style` default |
| MINOR: credential gaps | §3, §6.8 |
| MINOR: a fifth race | §6.7 item 5 |
| MINOR: the counter counts reads | §6.11 HELP text, renamed `skipped_objects` |
| MINOR: the header length is frozen | §6.3 |
| MINOR: test setup | §7: nextest filter, readiness check, the posix directory |
| MINOR: cross-references | fixed |
| MINOR: nested prefixes | §6.2, README |

**Questions, answered in the design:**

- What does SMA-790 do with a bare error compared with `Partial`? §6.5 states the contract: bare
  means "nothing written", `Partial` means "maybe part". Both commit the run without the rest.
- Versioned buckets? Refused (§6.8 step 1). This is my call; see §10.
- Did the measurement run on a CI runner? No. §7 and §11 state it.
- `/readyz`? A non-goal (§3).
- Strong consistency? A stated assumption (§6.2).

**Not folded in:**

- The tombstone delete (an option inside the delete-race finding). Reason: it closes race 3 but adds
  a tombstone rule to `list`, `usage`, the caps and `delete_scope`, and objects that stay. §10
  records it.
