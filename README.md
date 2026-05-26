# paigasus-core

Public, Apache-2.0 polyglot monorepo for **Paigasus** — the open core of the
platform. It houses the shared proto contracts, the Rust behavioral kernel and
its language bindings, and the Python and TypeScript workspaces built on top of
them.

## Repository layout

```
paigasus-core/
├── contracts/   # Protobuf source of truth + codegen (buf)
├── rs/          # Rust: Cargo workspace — libs, FFI bindings, services
├── py/          # Python: uv workspace
├── ts/          # TypeScript: pnpm workspace — packages + apps
├── .moon/       # Moon task-runner configuration
└── .github/     # CI workflows and repo automation
```

Each workspace has its own `README.md` with more detail.

## Status

Bootstrapping. The directory shell and baseline docs are in place; the
individual workspaces are being scaffolded issue-by-issue — Moon configuration,
the Cargo / uv / pnpm workspaces, and the proto toolchain. Until those land
there is no unified build yet.

## Quickstart

Tooling is orchestrated by [Moon](https://moonrepo.dev). Once the workspaces
are scaffolded, the standard entry point will be:

```bash
# Available after the workspace-setup issues land:
moon ci          # run the affected build / test / lint graph
```

For now, clone the repo and read the per-workspace `README.md` files to see
what each area will hold.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md).

## License

[Apache License 2.0](./LICENSE).
