# Graph Report - .  (2026-05-10)

## Corpus Check
- 15 files · ~74,877 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 98 nodes · 117 edges · 17 communities detected
- Extraction: 82% EXTRACTED · 18% INFERRED · 0% AMBIGUOUS · INFERRED: 21 edges (avg confidence: 0.81)
- Token cost: 0 input · 0 output

## Community Hubs (Navigation)
- [[_COMMUNITY_Core API & Error Handling|Core API & Error Handling]]
- [[_COMMUNITY_MIME Type Detection Tests|MIME Type Detection Tests]]
- [[_COMMUNITY_Download & Range Parsing|Download & Range Parsing]]
- [[_COMMUNITY_Application Server Setup|Application Server Setup]]
- [[_COMMUNITY_Build System & Platform Config|Build System & Platform Config]]
- [[_COMMUNITY_Magic Cookie Pipeline|Magic Cookie Pipeline]]
- [[_COMMUNITY_Upload & Multipart Transfer|Upload & Multipart Transfer]]
- [[_COMMUNITY_Error Type Definitions|Error Type Definitions]]
- [[_COMMUNITY_WebSocket Progress Events|WebSocket Progress Events]]
- [[_COMMUNITY_Test Utilities|Test Utilities]]
- [[_COMMUNITY_Project Documentation|Project Documentation]]
- [[_COMMUNITY_Error Response|Error Response]]
- [[_COMMUNITY_Progress Notifier|Progress Notifier]]
- [[_COMMUNITY_Upload Response|Upload Response]]
- [[_COMMUNITY_WebSocket Commands|WebSocket Commands]]
- [[_COMMUNITY_Build Entry Point|Build Entry Point]]
- [[_COMMUNITY_Production Constants|Production Constants]]

## God Nodes (most connected - your core abstractions)
1. `upload_file` - 6 edges
2. `main()` - 5 edges
3. `assert_all_files_with_extension_have_mime_type()` - 5 edges
4. `upload_file()` - 5 edges
5. `multipart_upload()` - 5 edges
6. `Application` - 5 edges
7. `compile_merged_magic_file()` - 4 edges
8. `with_custom_cookie()` - 4 edges
9. `mime_type_magic_blocking()` - 4 edges
10. `mime_type_magic()` - 4 edges

## Surprising Connections (you probably didn't know these)
- `NGC 6888 Crescent Nebula` --conceptually_related_to--> `RAPID - Robust Asynchronous Processing for Image Data`  [INFERRED]
  data/files/NGC 6888.jpg → README.md
- `Upload UI (index.html)` --references--> `upload_file`  [INFERRED]
  assets/index.html → src/upload.rs
- `Upload UI (index.html)` --references--> `download_file`  [INFERRED]
  assets/index.html → src/download.rs
- `Upload UI (index.html)` --references--> `ws_upload_progress`  [INFERRED]
  assets/index.html → src/ws.rs
- `compile_merged_magic_file` --conceptually_related_to--> `custom_magic_databases`  [INFERRED]
  build.rs → src/magic.rs

## Hyperedges (group relationships)
- **File Upload Pipeline** — index_html_upload_ui, upload_upload_file, magic_mime_type_magic, upload_multipart_upload, ws_uploadevent [INFERRED 0.90]
- **S3 Storage Operations** — upload_upload_file, upload_multipart_upload, download_download_file, lib_appstate [INFERRED 0.85]
- **Custom Magic Database Pipeline** — build_compile_merged_magic_file, magic_custom_magic_databases, magic_with_custom_cookie, magic_mime_type_magic_blocking [INFERRED 0.85]

## Communities

### Community 0 - "Core API & Error Handling"
Cohesion: 0.15
Nodes (16): download_file, parse_range, DownloadError, MimeTypeError, UploadError, Upload UI (index.html), Application, AppState (+8 more)

### Community 1 - "MIME Type Detection Tests"
Cohesion: 0.25
Nodes (12): custom_magic_databases(), detect_by_path(), load_cookie(), mime_type_fujifilm_x_raw_files(), mime_type_indd_files(), mime_type_jpg_files(), mime_type_magic(), mime_type_magic_blocking() (+4 more)

### Community 2 - "Download & Range Parsing"
Cohesion: 0.22
Nodes (2): download_file(), parse_range()

### Community 3 - "Application Server Setup"
Cohesion: 0.2
Nodes (4): Application, AppState, ErrorResponse, main()

### Community 4 - "Build System & Platform Config"
Cohesion: 0.44
Nodes (7): clean_directory(), compile_merged_magic_file(), configure_macos_libmagic(), configure_windows_libmagic(), main(), resolve_file_command(), ProgressNotifier<'a>

### Community 5 - "Magic Cookie Pipeline"
Cohesion: 0.29
Nodes (8): compile_merged_magic_file, resolve_file_command, custom_magic_databases, load_cookie, mime_type_magic, mime_type_magic_blocking, with_custom_cookie, with_default_cookie

### Community 6 - "Upload & Multipart Transfer"
Cohesion: 0.43
Nodes (5): calculate_chunk_size(), multipart_upload(), ProgressNotifier, upload_file(), UploadResponse

### Community 7 - "Error Type Definitions"
Cohesion: 0.4
Nodes (3): DownloadError, MimeTypeError, UploadError

### Community 8 - "WebSocket Progress Events"
Cohesion: 0.5
Nodes (4): handle_socket(), UploadEvent, ws_upload_progress(), WsCommand

### Community 9 - "Test Utilities"
Cohesion: 1.0
Nodes (2): assert_all_files_with_extension_have_mime_type, list_files_with_extension

### Community 10 - "Project Documentation"
Cohesion: 1.0
Nodes (2): NGC 6888 Crescent Nebula, RAPID - Robust Asynchronous Processing for Image Data

### Community 14 - "Error Response"
Cohesion: 1.0
Nodes (1): ErrorResponse

### Community 15 - "Progress Notifier"
Cohesion: 1.0
Nodes (1): ProgressNotifier

### Community 16 - "Upload Response"
Cohesion: 1.0
Nodes (1): UploadResponse

### Community 17 - "WebSocket Commands"
Cohesion: 1.0
Nodes (1): WsCommand

### Community 18 - "Build Entry Point"
Cohesion: 1.0
Nodes (1): build.rs main

### Community 19 - "Production Constants"
Cohesion: 1.0
Nodes (1): Production Constants

## Knowledge Gaps
- **25 isolated node(s):** `MimeTypeError`, `ErrorResponse`, `AppState`, `WsCommand`, `UploadEvent` (+20 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **Thin community `Download & Range Parsing`** (10 nodes): `download_file()`, `end_clamps_to_last_byte()`, `full_range()`, `inverted_range_is_none()`, `missing_prefix_is_none()`, `open_ended_range()`, `parse_range()`, `download.rs`, `start_beyond_size_is_none()`, `suffix_range()`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Test Utilities`** (2 nodes): `assert_all_files_with_extension_have_mime_type`, `list_files_with_extension`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Project Documentation`** (2 nodes): `NGC 6888 Crescent Nebula`, `RAPID - Robust Asynchronous Processing for Image Data`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Error Response`** (1 nodes): `ErrorResponse`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Progress Notifier`** (1 nodes): `ProgressNotifier`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Upload Response`** (1 nodes): `UploadResponse`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `WebSocket Commands`** (1 nodes): `WsCommand`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Build Entry Point`** (1 nodes): `build.rs main`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Production Constants`** (1 nodes): `Production Constants`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `download_file()` connect `Download & Range Parsing` to `Build System & Platform Config`?**
  _High betweenness centrality (0.087) - this node is a cross-community bridge._
- **Why does `detect_by_path()` connect `MIME Type Detection Tests` to `Build System & Platform Config`?**
  _High betweenness centrality (0.058) - this node is a cross-community bridge._
- **Are the 3 inferred relationships involving `assert_all_files_with_extension_have_mime_type()` (e.g. with `mime_type_jpg_files()` and `mime_type_fujifilm_x_raw_files()`) actually correct?**
  _`assert_all_files_with_extension_have_mime_type()` has 3 INFERRED edges - model-reasoned connections that need verification._
- **What connects `MimeTypeError`, `ErrorResponse`, `AppState` to the rest of the system?**
  _25 weakly-connected nodes found - possible documentation gaps or missing edges._