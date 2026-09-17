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


