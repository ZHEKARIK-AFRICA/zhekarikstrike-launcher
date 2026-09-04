# Следующий этап ускорения Google Drive — локальная реализация

Утверждённый источник требований: план пользователя «Следующий этап ускорения Google Drive: четыре улучшения» от 2026-09-04. Базовый production-код: `80abb47`. Ветка: `codex/drive-pack-speed-next-local`.

## Global Constraints

- Только локальные изменения и измерения; без push, production, backend, Drive writes, нового EXE для пользователя или смены версии.
- Google Drive only; packs <=64 MiB; запросы <=16 MiB; download jobs 2..6.
- Не изменять установленную игру и config.json. Помеченные уникальные тестовые каталоги только на D:; удалять их после остановки всех задач, отчёты сохранять.
- Сохранить compressed/raw/file/whole-pack hashes, resume, immutable generations, streaming commit/journal и final download barrier.
- Реальный бюджет двух серий вместе: 4 GiB принятых pack bytes, включая retries/prefetch; резервировать размер каждого запроса. На ошибке/недостоверном отчёте/исчерпании бюджета остановить серию без дополнительных попыток эксперимента.
- Субагенты только gpt-5.6-luna / low; иначе основная модель. Тесты одним функциональным блоком после реализации; при ошибках только затронутый scope. Никаких full Rust/frontend/release/clean.

## Task 1: Freeze baseline and isolated single-run harness

До production edits расширить только cfg(test) harness: один запуск принимает полный frozen manifest, ordered files, budget, seed, report path и сценарий install/repair. Baseline всегда использует прежний Optimized (не Baseline enum). Repair создаёт повреждённые заменители и sentinel внутри нового owned workspace, реально проверяет выбранные файлы и выполняет transactional replacement; финальные SHA и sentinel проверяются. Оба executable используют одинаковый harness. Собрать один baseline --release --no-run, сохранить executable, source/build/tool metadata и SHA после asInvoker patch.

## Task 2: Non-Tauri content-pack-core

Workspace crate src-tauri/crates/content-pack-core, opt-level=3/codegen-units=16, без LTO; root/Win32 settings сохранить. Перенести concrete streaming SHA, bounded zstd/raw verification, unique intervals, checked range arithmetic, cost planner и pure download/queue controllers. Никаких Tauri/AppError/URL/logging/config/async/fs descriptor dependencies. Launcher adapters map typed core errors; hot loops выполняются внутри core без generic callbacks.

## Task 3: HTTP/2, bounded prefetch and measured planning

Reused pack client: http2_adaptive_window(true), обычный ALPN HTTP/1.1 fallback, exact host/TLS/no redirects unchanged. Record negotiated protocol and header/body/local-writer wait separately.

Prefetch next range at <=2 MiB remaining; one current + one prefetch per pack, global <=6 HTTP requests, demand priority, no new prefetch below512 MiB free memory. Bounded application prefetch <=2 MiB/pack (<=12 MiB overall). One sequential writer/hash; buffer flush before compressed chunk publication; retain sync_all at Range boundaries. Count bytes immediately at receive; unsaved prefetch not resumable progress. Cancel/error/replica rotation drain both requests; final barrier includes prefetch.

Cost = bytes/rate + requests*header latency; full also uses16 MiB requests. DP choose complete required compressed chunk spans, join gaps only if beneficial, partial ranges <=16 MiB. Sparse only if >=5% predicted gain. Defaults8 MiB/s +250ms; calibrated after4 successes and64 MiB, medians last8; exclude buffer wait/failed attempts. Replan pending packs only. Atomically freeze started plans bound manifest/pack hashes, validate range bounds/closure on resume; preserve compatible legacy cache boundaries. Reuse valid local bytes first; existing progress forecast updated.

## Task 4: Functional block and two real ABBA series

Commands: cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check; cargo test --manifest-path src-tauri/Cargo.toml --workspace --release drive_pack_ --lib. Cover actual prefetch overlap and limits, HTTP1/2/security, range-boundary chunks, resume/replicas/hash/cancel/drain, barrier rollback, core equivalence, sparse/dense/frozen/legacy plans.

Save separate candidate executable. Orchestrate saved binaries via scripts/test-adaptive-packs.ps1 without rebuilding. Frozen manifest ac4ab8a152e3e0371c7a61f77f4e531d5ed163f7283ea5672c1fe47f182cc488, content01a13dfb3448ce6c55ec2051d70ad61775cbe1c2fa322330542d3b879d9675db, seed461ea0b9-971f-4b65-bdea-0a7495a0b813.

Install ABBA: existing359 whole files/8packs, baseline475532317 bytes/run. Repair ABBA: csgo/maps/{cs_italy,cs_rush,de_aztec,de_cbble}.bsp, baseline264752165 bytes/run, required compressed84331816. Identical order/seed and empty per-run caches; full manifest never truncated. Single sequential shared4GiB ledger across executables, stop on missing report/error. Primary pipeline-to-commit time; bytes/requests/header/write waits/concurrency/prefetch/first materialization/memory secondary. Evaluate install/repair separately: both paired comparisons faster AND mean time reduction>=10%; otherwise report unconfirmed.

## Task 5: Review, report, cleanup

Preserve test coverage. Scoped independent review plus root concurrency audit. Report exact commits, binary hashes/build settings/tools, raw measurements and decisions. Verify protected config/state SHA+mtime before/after. Remove only owned test data; keep reports/build evidence. Save changes locally, never publish.
