# Backend

SQL queries use SQLx macros (`query!`, `query_as!`, `query_scalar!`) to check SQL,
parameter types, result fields, and nullability at compile time.

The committed `.sqlx/` directory contains query metadata generated against a
PostgreSQL database initialized from `migrations/`. Cargo defaults to offline
checking through `.cargo/config.toml`, so normal builds do not need a running
database or database credentials:

```sh
cargo check --all-targets --locked
cargo build --release --bin backend --locked
```

Runtime connections still use `DATABASE_URL`. Offline checking only affects
compilation; it does not replace applying migrations to the runtime database.

## Model API

Both analysis layers default to `gpt-6-luna` through
`https://aiproxy.okhsunrog.ru/v1/chat/completions`. The base URL, model IDs, and
name of the credential environment variable are configured in `config.toml`.
The default credential variable is `MY_AI_PROXY`; do not put its value in TOML
or version control. Export it before launching the backend or Docker Compose.

Requests use JSON mode, a 120-second timeout, and the proxy's strict
compatibility mode. No token cap is sent: the subscription proxy does not
support that control. GPT-6 Luna uses `reasoning_effort: none` for this workload.

To check both analysis layers using a JPEG without writing to the database:

```sh
cargo run --bin test_vision -- /path/to/test.jpg
```

## Updating SQL or migrations

Install the CLI matching the SQLx dependency:

```sh
cargo install sqlx-cli --version 0.8.6 --locked --no-default-features --features rustls,postgres
```

Use a disposable PostgreSQL 17 database, explicitly set `DATABASE_URL` to that
database, then run from this directory:

```sh
cargo sqlx migrate run
SQLX_OFFLINE=false cargo sqlx prepare -- --all-targets --locked
SQLX_OFFLINE=false cargo sqlx prepare --check -- --all-targets --locked
cargo test --lib --test repository --locked
```

Commit the updated `.sqlx/` files together with the SQL changes. Add new migration
files when changing the schema; keep previously applied migrations unchanged.
CI recreates the schema from migrations and checks that the metadata is current.

The repository integration tests use `#[sqlx::test]`, which creates and migrates
separate test databases. The PostgreSQL role in `DATABASE_URL` needs `CREATEDB`.
These tests do not call listing sources, image storage, or model providers.
