# RAPID
Robust Asynchronous Processing for Image Data

 
## Description

RAPID is a web service that allows a user to upload an image to a server; upon upload, several concurrent operations will take place on such an image including optimizations, transformations and analysis. Clients should be notified of progress on all operations. The number of derived products from the initial image will be fixed for this project, and those resources will also be downloadable on their respective paths.

### Main features

* Allow upload of image files through a web browser
* Every file will have its mime type checked upon upload, rejecting files that are not images
* Backend is an HTTP web service following typical REST patterns
* There is no authentication for this service at this point
* The system should allow uploading large files (several GB). This project will explore implementing resumable uploads for this
* When creating a new resource (image upload), the system should offer a way to track status and progress of the different image transformations
* The initial image transformations targeted for this project are
    - Thumbnail and preview generation in 3 sizes: 80x80, 250x250 and 800x600 pixels
    - High resolution: if the original file is larger than 3000px, a high resolution preview is generated (6000x6000 pixels)
* All images including original are stored in AWS S3 compatible storage and can be retrieved and downloaded from a client
* Metadata for images (minimal set to start with) will be stored in a Postgres database

### Technology stack

### Build prerequisites

RAPID links against two native C libraries, so they must be present before
`cargo build` will succeed.

#### macOS

```sh
brew install vips libmagic file-formula
```

Three things about this are worth knowing, because the failure modes are not
obvious:

- **`vips` and `libmagic` live outside the default linker search path.**
  Homebrew installs them under `/opt/homebrew` (Apple Silicon) or `/usr/local`
  (Intel), neither of which the macOS linker searches. `build.rs` resolves each
  keg and emits the matching `rustc-link-search` directive; without them the
  build fails at link time with `ld: library 'vips' not found`.
- **`file-formula` is required, not optional.** The custom magic database in
  `magic-files/` is compiled at build time, and a compiled `.mgc` carries a
  format version that libmagic refuses to load unless it matches its own.
  Apple's `/usr/bin/file` (5.41) emits format version 16, while Homebrew's
  libmagic (5.46) only loads version 20. On a mismatch libmagic does not error —
  it falls back to parsing the binary `.mgc` as magic *source text*, printing
  `offset ... invalid` warnings and leaving a database that matches nothing. So
  `build.rs` compiles with Homebrew's `file`, which is built against the same
  libmagic that gets linked.
- **Apple's `file` also leaves a stray `magic.mgc` behind.** It compiles its own
  default system database into the working directory alongside the intended
  output. Since the custom database loader picks up every `.mgc` in that
  directory, `build.rs` prunes anything that is not the expected output.

`build.rs` emits a `cargo:warning` naming the missing formula if any of the
three is absent.

#### Linux (Debian/Ubuntu)

```sh
apt install libvips-dev libmagic-dev file
```


### Running the project

In addition to the native libraries above you need:

- **Rust 1.85 or newer** — the crate is on edition 2024. Install via [rustup](https://rustup.rs).
- **Docker (with Compose)** — used to run the S3-compatible object store locally.

#### 1. Start the object store

RAPID stores originals and derivatives in S3-compatible storage. `compose.yaml`
provides [RustFS](https://rustfs.com) for local development:

```sh
docker compose up -d
```

This exposes the S3 API on `localhost:9000` and the console on `localhost:9001`.
Credentials default to `rustfsadmin` / `rustfsadmin`; override them with
`RUSTFS_ROOT_USER` and `RUSTFS_ROOT_PASSWORD`. The `rapid-uploads` bucket does
not need to be created by hand — the service creates it on startup if it is
missing.

#### 2. Configure the environment

The service reads a `.env` file from the project root (via `dotenvy`). Only the
two S3 credentials are required; everything else has a working default:

```sh
# required
RAPID_S3_ACCESS_KEY=rustfsadmin
RAPID_S3_SECRET_KEY=rustfsadmin
```

| Variable | Default | Purpose |
| --- | --- | --- |
| `RAPID_S3_ACCESS_KEY` | *(required)* | S3 access key |
| `RAPID_S3_SECRET_KEY` | *(required)* | S3 secret key |
| `RAPID_S3_ENDPOINT` | `http://localhost:9000` | S3 endpoint URL (path-style addressing is forced) |
| `RAPID_S3_BUCKET` | `rapid-uploads` | Bucket for originals and derivatives |
| `RAPID_S3_REGION` | `us-east-1` | S3 region |
| `RAPID_DB_PATH` | `sqlite:data/rapid.db?mode=rwc` | SQLite metadata database URL |
| `RAPID_UPLOAD_DIR` | `uploads/` under the system temp dir | Staging directory for in-flight uploads (created on startup) |
| `RAPID_MAX_INFLIGHT_PARTS` | `64` | Global cap on concurrent S3 part uploads (see [Upload Concurrency](#upload-concurrency)) |
| `RAPID_LOG_LEVEL` | `info` | `tracing` env-filter directive, e.g. `rapid=debug,info` |

The SQLite file is created on demand but its parent directory is not, so create
it once:

```sh
mkdir -p data
```

Migrations in `migrations/` are applied automatically at startup.

#### 3. Run the service

```sh
cargo run
```

The binary is `rapid-svc` and it listens on `0.0.0.0:8080`. Open
<http://localhost:8080> for the upload UI, served from `assets/`. Logs are
emitted as JSON on stdout, and shutdown is graceful on Ctrl-C.

Useful endpoints:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/health`, `/ready` | Liveness and readiness probes |
| `POST` | `/upload` | Image upload — raw request body (10 GB limit, 30 minute timeout) |
| `GET` | `/ws/upload-progress` | WebSocket stream of upload and derivative progress |
| `GET` | `/files/{id}` | Download an original |
| `GET` | `/derivatives/{parent_id}/{size}` | Download a derivative; `{size}` is `table_thumb`, `thumbnail`, `small_preview`, `large_preview` or `high_res` |
| `GET` | `/files/{id}/metadata`, `/files/{id}/derivatives` | Metadata for one file |
| `GET` | `/api/files`, `/api/files/{id}/detail` | Metadata listing used by the UI |
| `DELETE` | `/api/files`, `/api/files/{id}` | Delete all files, or one file |

A quick smoke test once the service is up:

```sh
curl -i localhost:8080/health
curl -i --data-binary @"data/files/NGC 6888.jpg" \
  -H "x-file-name: NGC 6888.jpg" \
  localhost:8080/upload
```

The upload body is the raw file bytes — not a multipart form. Two headers shape
the request: `x-file-name` carries the original name, whose extension is used
for image format detection, and an optional `x-upload-id` ties the upload to a
WebSocket subscriber so progress events can be routed to it. The MIME type is
detected from the file contents, and anything that is not `image/*` or
`video/*` is rejected with a 4xx.

For a release build, use `cargo run --release` — derivative generation is
noticeably faster with optimisations on.

#### Running the tests

```sh
cargo test
```

The tests need the same native libraries as the build but no database or object
store. The MIME-detection tests do read image fixtures from `data/files`
(`.jpg`, `.indd`) and `data/files/raw` (`.raf`), which is a gitignored
directory — on a fresh clone those three tests fail until you drop sample files
in. Every other test is self-contained.


### Upload Concurrency

Multipart uploads to S3 are governed by two semaphores:

- **Global semaphore** (`RAPID_MAX_INFLIGHT_PARTS`, default 64): limits the total number of S3 part uploads in flight across all uploads. This protects local resources (memory, file descriptors, network connections) — not S3 itself, which scales horizontally.
- **Per-upload semaphore** (derived as `global / 4`, minimum 4): limits how many parts a single upload can have in flight at once. This enforces fairness — without it, one large file (e.g. 10GB = 1,250 parts) could monopolize all global permits and starve every other concurrent upload.

Both semaphores use a 10-minute acquisition timeout to guard against deadlocks without rejecting legitimate traffic under load.

### Upload Progress Tracking

Upload progress is tracked via a shared map (`upload_progress`) that maps client-provided upload IDs to event channels. WebSocket subscribers register their upload IDs in this map, and the upload handler sends progress events through it.

This map uses [DashMap](https://docs.rs/dashmap), a concurrent hash map with fine-grained per-shard locking, instead of a `RwLock<HashMap>`. Under concurrent uploads with active WebSocket subscribers, a single `RwLock` becomes a serialization bottleneck — every progress lookup or subscription insert contends on the same lock. DashMap shards the map internally so that operations on different keys rarely contend, giving near-linear scalability as concurrency increases.

### WebSocket Architecture

Each WebSocket connection uses two layers of channel splitting to allow concurrent reading and writing without shared mutable state:

- **WebSocket split** (`socket.split()`): separates the socket into independent write (`ws_tx`) and read (`ws_rx`) halves so they can be used in concurrent tasks — a send task that pushes events to the client and a receive task that reads subscribe commands.
- **mpsc channel** (`tokio::sync::mpsc`): acts as an indirection layer between upload workers and the WebSocket. When a client subscribes to an upload ID, a clone of the channel's sender is stored in the shared `upload_progress` map. Upload workers send progress events into this sender from any task, and the send task drains the receiver and serializes events as JSON onto the WebSocket. This decouples upload workers from the WebSocket lifetime — multiple concurrent uploads can emit events (multi-producer) while a single task writes them to the socket (single-consumer).

### Project structure


