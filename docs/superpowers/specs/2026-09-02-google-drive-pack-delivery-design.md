# Google Drive Pack Delivery and Streaming Commit Design

## Goal

Reduce a fresh ZHEKARIK STRIKE installation from roughly one hour to a practical duration while keeping Google Drive as the only primary object store. Launcher `1.6.16` introduces a new pack-based delivery protocol for the already-published game content `1.0.3.6` and removes v2 loose chunks from the runtime fallback chain.

The design also removes the separate full reread of staged game files during commit and cleans temporary content after a successful installation.

## Measured Baseline and Root Cause

The real Windows E2E installation of content `1.0.3.6-r1` completed successfully with these timings:

- download and concurrent materialization: about 35 minutes 51 seconds;
- final transactional commit: about 24 minutes 34 seconds;
- total: about one hour.

The v2 manifest contains 5,688 files, 7,804 chunk references, and 7,191 unique compressed objects. The publisher resets its fixed 8 MiB chunk window at every file boundary. Consequently, every non-empty file produces at least one object and every file has an arbitrarily small terminal object.

The resulting distribution is hostile to Google Drive public downloads:

- median compressed object: 22,301 bytes;
- 4,023 objects are at most 64 KiB but carry only 26.75 MB;
- 5,102 objects are at most 1 MiB but carry only 334.47 MB;
- only 2,110 objects are full raw 8 MiB chunks, but they carry 91.52% of all compressed bytes.

Exact-host probes observed roughly 1.1-1.4 seconds to the first byte of each Drive object. The existing launcher uses HTTP/1.1 because its native TLS build does not enable ALPN. Increasing the per-file chunk limit cannot solve the problem: 96.4% of game files contain no more than one chunk, so a layout that never crosses file boundaries has a floor of roughly one request per file.

The final commit is independently slow. Materialization already verifies the compressed chunk hash, raw chunk hash, complete file hash, size, and fsyncs the staged file. `commit_staged_files` then hashes every staged file again, serially rereading about 19.6 GB before renaming it. The successful install also leaves all 7,191 compressed chunks, about 8.16 GiB, in the game directory.

## Compatibility and Rollout Decision

V3 is a new self-contained public contract. Existing v1 and v2 payloads are not extended because launcher `1.6.13` strictly validates those schemas.

Runtime fallback is deliberately:

```text
v3 Google Drive packs -> v1 ZIP
```

There is no v2 runtime fallback in launcher `1.6.16`.

At rollout:

- v3 is prepared and verified while dormant;
- v1 remains active and immutable for the entire rollout;
- the v3 pointer is temporarily activated only for the controlled E2E, cleared immediately on failure, and left active for publication only after that E2E succeeds;
- the v2 content and mirror pointers are deactivated;
- old launchers observe v2 `404` and use the existing v1 ZIP;
- launcher `1.6.16` observes v3 and uses Drive packs;
- existing loose v2 Drive objects and manifests remain disabled for one successful release cycle only to make cleanup reversible, then are deleted explicitly; they are not an approved rollback transport for this release.

A missing v3 pointer returns `404` and permits the v1 ZIP path. A malformed, incomplete, or inconsistent active v3 returns `503`; launcher `1.6.16` fails visibly and does not hide a publication error behind a multi-gigabyte ZIP fallback. A failed pack download preserves resume data and returns an ordinary installation error rather than silently changing protocols.

## V3 Storage Layout

The publisher stores only manifests and publication state locally after upload:

```text
/srv/zhekarik-game/content-v3/
  current.json
  manifests/<manifest_sha256>.json
  staging/
  disabled/
/srv/zhekarik-game/publications/1.0.3.6/v3-state.json
```

Drive stores immutable pack replicas:

```text
Zhekarik Strike Launcher Releases/
  content-v3/
    <content_sha256>/
      packs/
        <pack_sha256>/
          replica-1.pack
          replica-2.pack
          replica-3.pack
```

Each replica has identical bytes and a different Drive file ID. The launcher downloads only one replica. Three replicas spread per-file public-download pressure and allow immediate failover without changing content identity. They use about 26.3 GB for the current 8.76 GB compressed release; the old disabled loose mirror temporarily adds about 8.76 GB during the cleanup-safety window but is never selected by launcher `1.6.16`.

For the current manifest, the deterministic layout produces exactly 136 packs at the 64 MiB limit (average 61.41 MiB). The previously considered 128 MiB limit would produce 67 packs. The selected 64 MiB profile deliberately trades 69 additional Drive requests for faster worker rebalancing and at most half as much retransmission after an unusable partial.

The NY publisher creates and uploads one pack at a time, verifies it, then removes the local pack. Peak additional server disk use is bounded to one pack plus small metadata rather than another complete content copy.

## Deterministic Pack Construction

The publisher traverses the existing validated content manifest in file order and each file's chunk order. It appends each unique compressed zstd frame on first use. Duplicate raw chunks refer to the already-recorded span and are not stored twice.

Pack profile `drive-pack-v1` has these fixed properties:

- target and maximum pack size: 67,108,864 bytes;
- ordering: manifest file order, chunk order, first occurrence only;
- payload: byte-for-byte concatenated existing independent zstd frames;
- no recompression and no change to raw or compressed chunk hashes;
- a pack closes before adding a chunk that would exceed the maximum;
- every compressed chunk appears in exactly one pack span;
- spans are ordered, non-overlapping, in bounds, and exactly cover every pack;
- pack identity is lowercase SHA-256 of the complete pack bytes.

The existing game `content_sha256` remains the content identity. It is recomputed without reading or requesting v2: publisher, backend, and launcher project the v3 data into the legacy transport-neutral schema-2 content document by removing pack-only fields from chunks, then hash its existing canonical bytes (`json.dumps(..., ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n"`). The result must equal the declared `content_sha256`. This preserves the already published game identity while keeping v2 out of runtime and allowing old v2 manifests to be deleted.

The v3 manifest has its own `manifest_sha256` and pack profile, so transport changes cannot impersonate a different game release. `manifest_sha256` is calculated using RFC 8785 JSON Canonicalization Scheme after removing the top-level `manifest_sha256` member. Publisher, backend, and launcher independently recompute it and reject a mismatch. Shared Python/Rust fixtures include reordered objects and Unicode strings so serializer behavior cannot create different identities.

For every pack, `prepare`:

1. validates each source compressed object against the active immutable content manifest;
2. writes the pack while calculating SHA-256 and exact span offsets;
3. fsyncs the completed temporary pack;
4. uploads three immutable Drive replicas through the server rclone configuration;
5. grants anonymous read access to each created replica and reads that permission back without exposing OAuth credentials;
6. validates every replica's Drive ID and size, and compares its complete remote MD5 with the MD5 calculated during the single local write; if complete trustworthy remote checksum metadata is unavailable, that replica is instead fully streamed anonymously and checked by SHA-256;
7. performs an anonymous exact-host `Range: bytes=0-0` probe for every replica and checks `206` plus the full size in `Content-Range`;
8. streams every replica of every pack anonymously and verifies the complete pack SHA-256, proving that each public ID serves the expected bytes before activation;
9. deletes only the verified local temporary pack.

Preparation is idempotent. A conflicting existing replica, duplicate Drive path, wrong ID, wrong size, bad range response, or bad streamed hash fails without changing a pointer. Activation is a separate atomic operation.

## Public V3 Contract

The backend adds:

```http
GET /launcher/game/v3/manifest
```

The active response is structurally equivalent to:

```json
{
  "schema_version": 3,
  "manifest_sha256": "64 lowercase hex",
  "content_sha256": "64 lowercase hex",
  "release_id": "1.0.3.6-r1",
  "game_version": "1.0.3.6",
  "generated_at": "RFC3339",
  "source_archive_sha256": "64 lowercase hex",
  "download_size": 8757634312,
  "unpacked_size": 19607923525,
  "chunking": {
    "profile": "fixed-v1",
    "chunk_size": 8388608
  },
  "compression": {
    "profile": "zstd-v1",
    "level": 6,
    "frame_checksum": true
  },
  "pack_profile": {
    "name": "drive-pack-v1",
    "max_pack_size": 67108864,
    "replica_count": 3
  },
  "packs": {
    "<pack_sha256>": {
      "size": 123456789,
      "replica_file_ids": ["drive-id-1", "drive-id-2", "drive-id-3"]
    }
  },
  "chunks": {
    "<raw_sha256>": {
      "uncompressed_size": 8388608,
      "compressed_size": 123456,
      "compressed_sha256": "64 lowercase hex",
      "pack_sha256": "64 lowercase hex",
      "offset": 0
    }
  },
  "files": []
}
```

File records preserve the existing path, full size, SHA-256, excluded, temporary, additional-check, and raw chunk sequence semantics.

V3 is self-contained on the wire, but installed-state consumers must not be coupled to its transport. After a successful v3 commit, the launcher stores a transport-neutral local inventory schema 1 under `.zhekarik/content/inventories/<content_sha256>.json`. It contains the validated release identity, source archive identity, chunking/compression profile, chunks, and files needed by adaptive verification, prerequisite analysis, repair, and recovery, but no delivery URL or Drive file ID. Existing valid v2 state for the same `content_sha256` is migrated by projecting its already persisted schema-2 manifest into this inventory without redownloading the game; the original manifest file is left untouched for downgrade compatibility. Prerequisite and integrity code reads the inventory first and performs this local migration when needed rather than assuming a public transport schema. This compatibility record does not authorize or initiate v2 network requests.

The backend and launcher both validate the complete manifest before accepting it:

- safe unique case-insensitive file paths;
- exact file/chunk closure and accumulated raw sizes;
- exact legacy content projection and `content_sha256`, without consulting a v2 pointer, endpoint, or retained v2 manifest;
- exact pack/chunk closure using checked arithmetic: for each pack, sort chunk spans by offset, require the first offset to be `0`, every next offset to equal the previous end, `end = offset + compressed_size`, and the final end to equal the pack size;
- `0 < pack.size <= pack_profile.max_pack_size`, every declared chunk and pack is referenced, and no undeclared span is tolerated;
- `download_size == sum(pack.size) == sum(unique chunk.compressed_size)`;
- three distinct valid Drive IDs per pack;
- pack span bounds, ordering, non-overlap, and complete coverage;
- expected content and canonical v1 game version agreement;
- exact `manifest_sha256` after RFC 8785 canonicalization with that field omitted.

The backend never calls Google during a user request. `current.json` points to a previously verified immutable manifest. Because `/launcher/game/v3/manifest` reflects a mutable pointer, every `200`, `404`, and error response uses `Cache-Control: no-cache` and a current ETag; activation and rollback must be observable after revalidation. Only immutable Drive pack objects may use long-lived immutable caching.

The launcher constructs only:

```text
https://drive.usercontent.google.com/download?id=<file_id>&export=download&confirm=t
```

Redirects and any other host, scheme, path, or query shape are rejected.

## Launcher Pack Planner

Discovery always requests v3 directly. It never probes v2. Only an HTTP `404` response from the v3 endpoint selects the existing v1 ZIP flow; a network error, timeout, `503`, invalid body, or active-pack failure remains a visible error.

After validating v3, the launcher determines required files using the existing state and adaptive integrity verifier. It then derives the unique missing raw chunks and groups their compressed spans by pack.

For each pack:

- a fresh installation or a plan whose unique required compressed spans contain at least 25% of the pack bytes downloads the complete pack;
- otherwise needed spans are sorted and adjacent ranges are coalesced when the gap is at most 64 KiB and the resulting range is at most 16 MiB;
- a complete pack is stored as `<pack_sha256>.pack.part` and atomically promoted after size and SHA-256 validation;
- complete partial ranges are stored in content-addressed range cache entries and each included compressed chunk is independently SHA-256 verified;
- an interrupted complete-pack `.part` may be resumed from any replica only after its length is checked and its existing prefix is fed into the continuing SHA-256 calculation; final pack SHA-256 still authenticates the combined bytes;
- already verified pack/range data is reused across cancel and restart.

Every Drive request sends `Accept-Encoding: identity`, forbids redirect, and requires the final URL to remain the exact constructed URL. Byte ranges are inclusive. A requested `bytes=start-end` must return `206`, exact `Content-Range: bytes start-end/total`, exact `Content-Length`, no multipart body, no content encoding, and EOF after exactly the declared length. A `200` response is accepted only for a fresh full-pack request and is never appended to a partial. A local offset equal to expected pack size triggers verify/promote without a request; an offset greater than expected size discards the partial.

Incomplete partial-range requests are discarded because they have no independently authenticated prefix. A transport-interrupted full-pack partial may survive cancel/restart, but if final pack SHA, size, range structure, or included compressed-chunk verification fails, the affected `.part` or range entry is deleted before another replica is tried. When all replicas fail, only previously completed and cryptographically verified cache entries are retained.

Required zero-byte files do not appear in a chunk-derived download plan, so the planner emits an explicit empty `VerifiedArtifact`. It is journaled, committed, and checked against the SHA-256 of the empty byte string like any other managed file.

Whole-pack verification checks pack size and SHA-256 before exposing it to materializers. Materialization additionally verifies every compressed chunk hash, bounded zstd output, raw chunk hash, final file size, and final file hash. A valid pack therefore never weakens the existing content integrity checks.

Replica selection is stable but distributed: a hash of operation ID and pack SHA chooses the first replica. On `403`, `404`, malformed range, incorrect size, or failed hash, that replica is disabled for the operation and the next replica is tried. Retryable `408`, `429`, `5xx`, header timeout, or body timeout gets bounded backoff before replica rotation. All three failed replicas end the operation and preserve only verified cache plus cleanly interrupted full-pack partials.

## HTTP and Adaptive Download Controller

The launcher enables native TLS ALPN and verifies/logs the negotiated HTTP version, allowing Google Drive to use HTTP/2. Pack downloads begin at two concurrent transfers and have a maximum of six. Large packs make byte throughput meaningful again; no controller decision is based only on the count of tiny completed objects.

The scheduler owns every pack attempt and receives live progress events containing source, pack, replica, current offset, useful bytes, header latency, and last-progress time. A real periodic timer runs independently of task completion.

Controller constants and behavior:

- tick every two seconds and measure unique verified/useful byte growth, excluding failed, gap, and duplicated retry bytes;
- use an EWMA with `alpha = 0.30` and keep a baseline at every accepted concurrency, including the maximum;
- collect at least three ticks and 64 MiB of unique useful bytes before judging a level or trial;
- after establishing a baseline, start a one-worker-higher trial only when pending work exists, no pressure occurred, and the verified-but-unconsumed backlog is below 256 MiB;
- keep the higher target only if its completed sample improves EWMA throughput by at least 5%; otherwise restore the previous target and enter a 20-second cooldown;
- halve target concurrency immediately on HTTP `429`, or after three timeout/`5xx` pressure events in a rolling 30-second window;
- lowering the target stops admission immediately and can rotate a clearly stalled outlier to another replica;
- user cancellation remains distinct from adaptive preemption and never causes fallback or a false error;
- per-attempt child cancellation tokens never cancel the whole installation;
- no automatic duplicate hedging is performed in `drive-pack-v1`.

Each replica receives at most two retryable attempts. Backoff is one then two seconds; a valid `Retry-After` is honored but capped at 30 seconds. Response headers have a 20-second timeout. A body with no new byte for 30 seconds is stalled; it may be adaptively rotated after 20 seconds only when another active transfer is advancing at least 512 KiB/s. Permanent response, Range, size, or hash errors rotate immediately. These bounds are controller decisions, not whole-operation cancellation.

The controller logs decisions locally without external telemetry. Progress shown to the user remains monotonic and is derived from unique downloaded compressed bytes plus committed raw bytes.

## Streaming Transactional Commit

The publisher/transport improvement alone does not remove the observed 24-minute commit tail. Launcher `1.6.16` therefore combines materialization and commit without weakening crash rollback.

Before the pipeline starts, Rust:

1. validates all target paths and current managed-file identities;
2. writes a complete write-ahead journal containing every replace/remove action, expected original identity, expected target identity, transaction ID, content SHA, and release ID;
3. fsyncs the journal;
4. switches it once to a streaming-commit phase before any game path is modified.

Materializer workers write a transaction-owned temporary file, validate compressed/raw/file hashes in one pass, flush and fsync it, then send a `VerifiedArtifact` to one commit coordinator. The coordinator revalidates the complete identity of any original managed target immediately before moving it. If that identity differs from the journal snapshot, it durably amends and fsyncs the journal with the newly captured original identity before filesystem mutation, then moves the original to backup and atomically renames the just-verified artifact into place. It never rereads the already verified new artifact to calculate the same SHA-256 again.

Only one coordinator mutates final game paths, preserving deterministic filesystem behavior. Downloads, decompression, hashing, original-target capture, and commit overlap through a bounded channel. Materializers reserve the final file size before starting and cannot exceed a staging-byte budget of `max(largest required file, 1 GiB)`; the reservation uses checked arithmetic and is released after commit or cleanup.

On any error or cancellation:

- admission stops;
- all workers are canceled and drained;
- the journal restores every original backup and removes only exact managed targets introduced by the transaction;
- verified pack and `.part` data remains available for resume;
- unknown user files are never removed.

After every artifact is committed, obsolete managed files are handled through the same journal, `state.json` is atomically written last with the same transaction ID/content SHA/release ID binding, the configured game version is updated, and transaction backup/staging is cleaned. Crash recovery compares that binding: a matching durable completion state means the installation succeeded and recovery only finishes forward cleanup; a missing or nonmatching state means reverse-order rollback. If any target, backup, journal, or state identity is ambiguous, recovery preserves all evidence and fails closed instead of deleting or overwriting either copy.

The design accepts the unavoidable local TOCTOU boundary between a verified staging handle being closed and its immediate guarded rename. Rehashing all data does not protect against a user who controls the running launcher or machine, and the game-content threat model does not claim that property.

## Cache Cleanup and Disk Accounting

Verified pack and range cache is content-addressed by content SHA, pack SHA, and exact range, independent of a transaction ID. Complete pack entries are promoted only after full pack SHA verification. Complete range entries are promoted only after all represented compressed chunks verify. In-progress writers and exclusive claim locks are transaction-owned. On rollback, verified entries survive; incomplete full-pack partials survive only clean cancellation or transport interruption, while incomplete ranges and any structurally or hash-invalid partials are discarded. A later operation takes a fresh exclusive claim, rechecks expected length, hashes an existing pack prefix, and completes normal final verification before promotion.

After `state.json` is durable, cache cleanup must not extend the visible installation tail:

- atomically rename the completed cache directory into `.zhekarik/content/cleanup/<uuid>`;
- report installation success after the rename;
- delete the cleanup directory in the background;
- retry leftover cleanup directories on next launcher startup.

V2 loose chunk cache left by older launcher versions is migrated through the same cleanup mechanism after a valid v3 state is committed.

Streaming commit changes disk preflight from full staging duplication to:

```text
missing pack/range bytes
+ backups for managed files being replaced or removed
+ bounded in-flight staging
+ 2 GiB safety reserve
```

All additions use checked arithmetic. Failure to reserve space occurs before journal activation.

## Status and Failure Semantics

Verification and installation remain separate user-visible `0-100%` stages.

The installation stage reports:

- unique network bytes and total planned network bytes;
- combined download/verified-commit progress;
- network throughput;
- effective installation throughput;
- ETA based on the slower remaining side of the overlapped pipeline.

The UI never displays internal filenames. A terminal error or cancellation clears ETA/progress animation and leaves a clear final status. Resume is explicitly shown as restoration of a previous interrupted installation.

V3 integrity failures, damaged active pointers, unsafe paths, impossible ranges, or hash mismatches fail closed. Network and replica failures preserve valid resume state. V1 ZIP is selected only when the v3 endpoint returns `404`, not when an active v3 release is malformed or partially unavailable.

## Repositories and Deployment

Changes are developed from fresh remote targets without force-push:

- launcher: `tauri-rework` -> `codex/drive-pack-v3-1.6.16`;
- publisher (`zs-updater`): `master` -> `codex/drive-pack-v3-1.0.3.6`;
- backend: `dev` -> `codex/drive-pack-v3-api`.

The NY server receives source only through GitHub at exact commit SHAs. Existing rclone credentials remain server-only and are never printed, committed, or added to launcher/GitHub artifacts.

The failed launcher `v1.6.15` GitHub release remains a private draft and its immutable tag is not reused. The corrected public launcher version is `1.6.16`. Before publication, the invalid `LAUNCHER_RELEASE_API_TOKEN` GitHub secret must be replaced with a token accepted by the deployed backend admin endpoint.

## Test Strategy

Testing is deliberately sparse and stage-batched. No tests run after an individual file, function, small commit, or isolated refactor. New automated tests are added only for failures that could corrupt installed data, activate an invalid publication, break resume, or silently select the wrong transport. Closely related cases use table-driven fixtures inside one cohesive test instead of one test function per field or status.

Do not add tests for trivial constructors/getters, serde fields individually, progress wording, logs, constants already exercised by a boundary test, or thin delegating functions. The expected new-test budget is approximately two publisher tests, two backend endpoint tests, and five focused Rust test functions; exceeding it requires identifying a distinct high-risk behavior not covered by an existing table.

Publisher tests cover:

- deterministic first-use pack construction;
- exact pack/chunk coverage and deduplication;
- no recompression and preserved chunk hashes;
- three distinct replicas;
- remote metadata, anonymous range, and streamed SHA verification;
- interrupted upload resume, idempotence, and atomic activation;
- one-pack disk bound and cleanup of only the inactive attempt.

Backend tests cover:

- v3 `200`, dormant `404`, and corrupt/incomplete active `503`;
- canonical manifest hash and exact v1/content/pack closure;
- invalid paths, ranges, Drive IDs, duplicate spans, and replica IDs;
- pointer responses use revalidation caching and activation/rollback is immediately observable;
- unchanged v1/v2 response schemas;
- deactivated v2 pointers while v1 remains available.

Focused Rust tests cover:

- strict v3 manifest validation and exact Drive URL construction;
- full-pack and coalesced-range planning boundaries;
- exact HTTP range validation, fresh/resumed streaming hashes, cross-replica resume, and deletion of corrupt partials;
- replica rotation and all-replicas-failed behavior;
- independent controller ticks, live useful-byte measurement, trial acceptance, cooldown, and in-flight downshift;
- user cancellation versus adaptive preemption;
- materialization from pack spans, explicit empty files, and corruption at pack/compressed/raw/file layers;
- transport-neutral inventory creation and migration from existing v2 local state without a v2 request;
- streaming commit success, error rollback, state-durable forward cleanup, incomplete-state crash rollback, identity ambiguity, and preservation of unknown files;
- post-state cache rename and startup cleanup retry;
- direct v3-404-to-v1 behavior and proof that v2 is never requested as fallback.

Tests run at only these implementation checkpoints:

1. After publisher and backend code are both complete, run their affected Python suites once as one checkpoint.
2. After the entire launcher transport, cache, materialization, commit, recovery, and inventory batch is complete, run formatting plus one focused Rust scope once. Do not run separate full `cargo check`, `cargo test`, or `clippy` here when that focused scope compiled the changed crate.
3. After all repositories and batches are integrated, run the existing complete local release gate exactly once. Do not duplicate its full frontend or Rust commands before or after it.
4. Run one real cold-cache Windows E2E after the local gate.
5. The tagged GitHub Actions publication performs the second and final complete release gate.

When a checkpoint fails, fix the whole related group and rerun only the failed scope. A complete checkpoint is repeated only if subsequent code changed something that it directly covers.

The one real Windows E2E must measure phase timings and demonstrate:

- fresh `1.0.3.6` installation from Drive packs;
- cancel/resume across different Drive replicas;
- no long small-object phase;
- download, materialization, and commit overlap;
- no separate full reread before commit;
- manual full verification and corruption repair using ranges;
- crash rollback and restart recovery;
- prerequisite manager, UAC game launch, overlays, and game-close cleanup;
- v3 `404` selects ZIP;
- deactivated v2 is not requested;
- successful install leaves no active 8.16 GiB chunk cache.

The performance baseline is the recorded cold-cache `3,625` seconds on the same Windows PC, target disk, and connection. The single performance run starts immediately before the frontend invokes installation and ends only after durable `state.json` plus the resolved success response; background deletion after the atomic cache-to-cleanup rename is excluded. Before it starts, only the disposable E2E game/state/cache directories are removed and the launcher process is restarted. Publication requires at most `1,812.5` seconds, no interval longer than 120 seconds between the final received network byte and durable state/success, and no regression in cancel/resume or integrity guarantees. If the result is within 10% of the time limit or is invalidated by a demonstrated environmental failure, do not publish; repeat it only after a concrete implementation adjustment or a confirmed environment correction, not merely to obtain a better sample.

## Rollout and Rollback

1. Deploy the backend with v3 dormant; endpoint returns `404`.
2. Build packs sequentially from the verified local v2 CAS and upload all three Drive replicas.
3. Run full publisher verification and reconstruct/hash every file from pack spans without writing another full reconstructed tree.
4. Build and locally gate launcher `1.6.16`.
5. Temporarily activate v3 for a controlled E2E; no published launcher calls it yet, v1 remains active, and failure immediately clears the v3 pointer. V2 may remain temporarily available only to already-released old launchers and is never consulted by `1.6.16`.
6. Complete the real Windows performance, repair, launch, and recovery E2E.
7. Publish signed launcher `1.6.16` after correcting the backend publication token.
8. Deactivate v2 content and mirror pointers; verify old launchers use ZIP and `1.6.16` uses v3.
9. Prune local NY v2 blobs only after Drive pack repair succeeds with v2 disabled.
10. Keep dormant loose Drive objects for one successful release cycle, then delete them explicitly.

V1 remains active and immutable throughout preparation, E2E, publication, and rollback. Manual rollback atomically clears only the v3 pointer, after which v3 returns literal `404` and the launcher selects the already-active v1 ZIP. V2 is not re-enabled and has no role in launcher `1.6.16` discovery, download, repair, or recovery.

## Non-Goals

- No non-Google object store or CDN.
- No FastCDC or delta patch algorithm.
- No signing of game manifests beyond the existing HTTPS and hash trust model.
- No automatic watchdog that mutates production pointers.
- No automatic Windows restart.
- No launcher source-integrity or anti-cheat system.
- No reuse of tag `v1.6.15`.
