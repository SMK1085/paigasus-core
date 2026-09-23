# paigasus-core — `contracts/`

<!-- Project memory. Claude Code loads this file only when it reads a file in
this directory, so it costs nothing in a session that stays out of contracts/.
The root CLAUDE.md holds the repo-wide rules and the two gate-checked blocks. -->

## Codegen and FFI bindings

- The **codegen-drift gate is an inline `ci.yml` step** (`.github/workflows/ci.yml:342-355`), NOT
  a `repo:*` Moon task — searching `moon.yml` for it finds nothing. That placement is deliberate
  and load-bearing: the step carries no `if:`, so it runs on EVERY CI run and cannot be
  deselected, where a `T`-array task would run only when affected and a wrong `inputs` list would
  switch it off silently. It delegates its freshness to `moon run contracts:generate`, so that
  task's `inputs` are what make the diff real: they now include `/.prototools` (which pins `buf`
  itself) and `/py/uv.lock` (which pins the `local:` betterproto2 plugin, run via `uv run
  --project ../py`), alongside `buf.gen.yaml` which pins the three REMOTE plugins. Before SMA-592
  the first two were absent, so a generator bump left the hash unchanged, Moon served a cached
  pass, `buf generate` never ran, and the diff compared the committed output against itself —
  vacuously green. `.moon/cache` is restored across CI runs (`ci.yml:113-119`), so that was a real
  CI hole, not a local-only one. `contracts:generate` still declares no `outputs:`; this makes its
  cache KEY honest, not its output restorable, which is the second reason the drift step stays
  unconditional. The inputs are pinned to exact equality by `CONTRACTS_GENERATE_INPUTS` in
  `ci/affected-graph/ci_targets.py` — reachable because `repo:affected-smoke` lists `*/moon.yml`.
  Cost of the two added inputs, measured on 2.5.3: one `buf generate` is ~0.7s warm, and over
  `main`'s 163 commits they select it on 32 commits (19%) that no old input would have selected.
- Two limits on that fix, neither closed. First, **`repo:input-liveness` cannot see
  `contracts:generate`.** `ci/affected-graph/task_inputs.py`'s `_repo_tasks` is keyed to
  `projects.get("repo")` by exact project id, so it liveness-checks `repo:*` tasks and nothing
  else. If `py/uv.lock` moved, `contracts:generate` would silently stop keying on the betterproto2
  pin while `CONTRACTS_GENERATE_INPUTS` stayed green — the SMA-553 failure class, on a task the
  liveness gate cannot reach. Second, the fix is a **CACHE-KEY fix, not an execution fix** — but
  NOT for the reason first recorded here (SMA-664). The old text said `uv run` executes a possibly
  stale `py/.venv`. That is wrong: a bare `uv run` locks and syncs the selected project before it
  invokes the command, and `contracts/buf.gen.yaml:46` invokes
  `uv run --project ../py protoc-gen-python_betterproto2` with no `--no-sync` and no `--frozen`.
  Neither `UV_NO_SYNC` nor `UV_FROZEN` is set anywhere in this repo (verified by grep). So the
  plugin that RUNS matches `py/uv.lock`, and only an explicit bypass could decouple them.
  What the cache key still cannot prove is that the task RAN: a Moon cache hit skips
  `contracts:generate` entirely, so no sync happens on that path — harmless today, because
  `py/uv.lock` is one of the task's inputs, so a generator bump busts the key and forces a real
  run. The residual worth watching is the opposite one: the invocation does not pass `--locked`,
  so a `py/pyproject.toml` change lets `uv run` UPDATE `py/uv.lock` mid-generate instead of
  failing. That last point follows from uv's documented `--locked` semantics; it is not measured
  here.
- A hand-written `.pyi` next to a PyO3 crate is an interface contract that basedpyright reads
  INSTEAD of the Rust, and it lives at the crate ROOT where `src/**/*` does not match it. A7 now
  demands every `{upstream}/*.pyi` found on disk, disk-conditional exactly like its `build.rs`
  clause. Do NOT read this as closing SMA-535: it makes a stub edit re-run the FFI smoke test, it
  does NOT make a stub that disagrees with the Rust fail. That needs a three-set drift gate
  (`#[pyfunction]` idents × `wrap_pyfunction!` registrations × stub `def` names), which is SMA-535
  proper and pairs with SMA-536.
- `wasm-pack build` **deletes `package.json`** in its `--out-dir`, even with `--no-pack`
  (`rs/crates/bindings/paigasus-wasm/.gitignore:4-10` records the measured behaviour). Never run
  it in the crate root. The release path builds into `.wasmpack-release-out`, a third scratch
  directory beside `generate-wasm`'s `.wasmpack-regen-out` and `test`'s `.wasmpack-test-out`
  (SMA-579; since SMA-634 the `build` task runs no wasm-pack and owns no out-dir).
- `wasm-pack` is **proto-pinned, not Moon-managed** — `moon setup` does not install it. Any job
  invoking `wasm-pack` needs an explicit `proto install wasm-pack` step first, the same class of
  gap the documented nextest trap already records for a different tool (SMA-579).
