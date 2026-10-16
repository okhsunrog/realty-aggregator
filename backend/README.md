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

## Collection and selection

`config.toml` controls collection. The default search covers KBR, apartments from
33 m² and prices up to 5 million RUB. This collection ceiling is independent of
the certificate estimate in the interface. Change `api.query.price2` to collect
more expensive candidates. Changing any query setting triggers another scan of
`api.initial_load_days` (90 by default); older records remain in the database.

Collection saves listings and their ordered photo URLs transactionally. Older
source snapshots cannot overwrite newer ones. Changed listing content clears its
listing analysis; changed photo URLs also clear the affected photo analysis.
A checkpoint advances only after every page was saved, using the time captured
before the request. Requests overlap by one hour. Pagination includes records at
the boundary timestamp; an ambiguous full page fails without advancing the
checkpoint. Image downloading and analysis resume separately after import errors.

`vision.max_retries` bounds analysis attempts. Failures use delayed retries and
store the last error; manual reanalysis resets the relevant attempts. Only the
first `vision.filter.max_photos` images are analyzed when that limit is set.
Missing interior evidence produces nullable assessments, not a shell verdict.
Model output must contain all expected fields and valid score ranges. These
assessments are suggestions for selection, not proof of eligibility for a
certificate or of the condition of the entire apartment.

The UI saves favorites, hidden candidates, selection stages and notes in the
backend. Price history starts with the price present when migration 002 runs,
then records imported price changes. Older price history cannot be recovered
from existing rows. The certificate amount is an editable browser-local estimate;
0 RUB is an old user-provided estimate, not a confirmed current amount.

## Actuality

Actuality sync imports dated results from `/main/apigetcheckfeed`, with pagination
and Moscow time conversion. It can reactivate as well as deactivate records, but
cannot override a newer source observation. Missing IDs remain unchanged. These
are recorded checks, not a request to check a site immediately. Coverage and
freshness depend on the upstream service; see [ADS API documentation](https://ads-api.ru/api).

The command below previews recorded checks without changing the database:

```sh
cargo run --bin sync_actuality -- --days 60
# Add --apply to import dated results.
```

## Local server

The server listens on `127.0.0.1:3000` by default. Docker ports for the backend,
PostgreSQL and MinIO are also bound to loopback. `SERVER_BIND` overrides the
backend listener (Compose uses `0.0.0.0:3000` inside its container).

```toml
[server]
bind = "127.0.0.1:3000"
allowed_origins = ["http://localhost:5173", "http://127.0.0.1:5173"]
```

Browser writes and status WebSocket connections reject origins outside this
allowlist. This local deployment has no user authentication; use an authenticated
reverse proxy before exposing it remotely. The frontend uses relative `/api`
URLs and proxies WebSockets through Vite. Photos are served by database image ID
through `/api/images/{id}/content`; the MinIO bucket can stay private.

Only one background task runs at a time within the server process. Repeated
starts return 409, invalid concurrency (outside 1–16) or models outside
`vision.allowed_models` return 400. Failed tasks publish `last_error` in status.
Run one backend instance against the database. Collection and actuality sync use
the configured intervals; scheduled tasks skip ticks missed during long work.

## Source availability checks and direct access

On Linux, `SOURCE_NETWORK_INTERFACE` selects the physical interface used by ADS
API, original-page checks and source photo downloads. For example, put
`SOURCE_NETWORK_INTERFACE=wlan0` in the ignored `backend/.env`. It disables
application proxies for these clients and binds their sockets to that interface.
The model API client keeps ordinary routing. The configured interface must remain
available; change it and restart the service after switching physical networks.
The implementation does not change system routing or turn off the VPN.

`actuality` in TOML controls the original-page queue (20 records per hourly run,
24 hours between checks, 10 seconds between requests by default). Checks allow
only supported HTTPS sources and restrict redirects to those domains. They use
explicit removal notices or an Offer availability value attached to the original
listing in JSON-LD. A successful HTTP response alone does not establish activity.
Unqualified 404, captcha, access denial, redirects to other pages, timeouts and
ambiguous markup remain unknown and do not deactivate records. The last attempt
and the last definite observation are stored separately. Source observations
cannot override a newer source snapshot. Changes in upstream markup can reduce
coverage; open the original and contact the seller before relying on the status.

`GET /api/listings?verified_only=true` selects recent positive original-page
checks using `actuality.recheck_hours`. `POST /api/check-sources` accepts
`{"ids":[123]}`; an empty ID list checks the next configured queue batch.
`POST /api/listings/123/check-source` checks a single original card. Both use the
same job admission guard as collection and analysis.

The first collection is deliberately bounded with the maintenance CLI; it does
not move the full collection checkpoint:

```sh
cargo run --bin maintenance -- sample --limit 20 --days 7
cargo run --bin maintenance -- covers --ids 123,456
cargo run --bin maintenance -- check-sources --ids 123,456
cargo run --bin maintenance -- analyze 123
```

Automatic bulk model analysis is disabled by default. Per-listing analysis first
downloads selected photographs, then runs both layers with the configured model.
At most eight photographs per listing are selected. Regular collection makes at
most `api.max_downloads_per_run` download attempts (100 by default), so a broad
history import does not immediately download the entire photo archive.

`maintenance export --ids ... --directory /path` exports explicitly selected
records, cached analyses and downloaded photographs. `maintenance import
--directory /path` imports that sample into the database and object storage
selected by the current environment. This supports a separate staging database
and avoids repeating model calls when promoting a validated sample. The files
contain listing data, not credentials; protect them as diagnostic artifacts.
Run mutating maintenance commands while server jobs are idle; do not run them concurrently with collection.

## Local deployment

The former `minio/minio:latest` image is no longer available. The [community repository](https://github.com/minio/minio) is archived; the pinned source build retains compatibility with existing object data for this local deployment. Build the pinned
community source revision with the installed Go toolchain (build files and cache
are kept under the dedicated temporary disk root):

```sh
./deploy/build-minio.sh
# Produces realty-minio:7aac2a2c5b7c for docker-compose.yml.
```

The backend can serve the built frontend at the same address. From the frontend
directory run `vp install` and `vp run build`. The default
`server.frontend_dir = "../frontend/dist"` is resolved from the backend working
directory; without built assets the backend remains an API server. The Vite
server remains available for development using `vp run dev`.

For this host, build the backend into the dedicated build directory, install the
`backend` and `maintenance` binaries into
`~/.local/share/realty-aggregator/bin/`, and install
`deploy/realty-aggregator.service` into `~/.config/systemd/user/`. Run:

```sh
systemctl --user daemon-reload
systemctl --user enable --now realty-aggregator.service
```

The service starts PostgreSQL and MinIO through Compose, applies outstanding
migrations and serves the application on `http://127.0.0.1:3000`. The launcher
loads only `MY_AI_PROXY` from the user's shared environment file when needed.
Credentials and the physical-interface setting are kept in `backend/.env`.
Stop the native service before using the alternative container backend profile:
`docker compose --profile backend up --build backend`. It uses host networking
for interface binding and builds its binaries in a matching Debian environment.
The container backend is an API deployment unless frontend assets are mounted.
