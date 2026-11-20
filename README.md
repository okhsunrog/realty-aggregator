# realty-aggregator

I was looking for an apartment in Kabardino-Balkaria and got tired of opening
hundreds of listings only to find bare concrete walls in the photos. This
service collects the listings, stores the photos and asks a vision model what
condition each apartment is in.

- Listings come from [ads-api.ru](https://ads-api.ru), which aggregates Avito,
  CIAN, Yandex Realty and others. Collection is incremental: a checkpoint moves
  forward only after every page is saved, and requests overlap by an hour so
  nothing at the window boundary is lost. Price changes are kept as history.
- Photos go to S3-compatible storage (RustFS in the dev setup).
- Analysis runs in two passes. Each photo gets a room type, a condition score
  and a list of issues; then the listing text and the photo results produce a
  summary. Model output is validated, failures are retried with a delay, and
  concurrency and request rate are limited.
- Removed listings are detected through the ADS API check feed and direct
  checks of the original pages.
- The web UI has filters, favorites, notes and selection stages.

Rust (Axum, Tokio, SQLx), PostgreSQL, Vue 3 with Naive UI.

## Running

You need an ads-api.ru account and a key for any OpenAI-compatible API.
The default `vision.api_base_url` in `backend/config.toml` points to my
[LLM proxy](https://github.com/okhsunrog/claude-proxy-rs); change it and the
model names there.

```sh
direnv allow                # or `nix develop`; otherwise install Rust and Bun
docker compose up -d        # PostgreSQL and RustFS
cp backend/.env.example backend/.env   # fill in the credentials
cd backend && cargo run     # applies migrations, listens on 127.0.0.1:3000
cd frontend && bun install && bun run dev   # http://localhost:5173
```

After `bun run build` the backend also serves `frontend/dist` itself.
The search region, price and area limits are in `backend/config.toml`.
Collection runs every hour, bulk analysis is off by default
(`vision.auto_analyze`) and can be started per listing from the UI.

There is no authentication, so keep it on localhost.

## SQL

Queries are checked at compile time against the metadata in `backend/.sqlx`,
so building doesn't need a database. After changing SQL or adding a migration:

```sh
cd backend
cargo sqlx migrate run
cargo sqlx prepare -- --all-targets
cargo test   # integration tests create their own databases
```
