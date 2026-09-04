# Локальное ускорение Google Drive packs

Основание: утверждённый пользователем план от 2026-09-04. Baseline диагностики: ea2e714. Ветка: codex/drive-pack-speed-local.

## Ограничения

Только локальная реализация и сравнение. Не публиковать, не менять версии, backend, manifests, Drive или конфигурацию/установленную игру. Drive-only, packs <=64 MiB, Range 16 MiB. Все compressed/raw/file/pack SHA и durable journal сохраняются. Субагенты исключительно gpt-5.6-luna, low. Тесты одним блоком после реализации; никакого cargo clean/full gate.

## Task 1: точные окна и очередь

Уникальные диапазоны отдельно от всего трафика и проверенных байтов. База >=10 секунд и 64 MiB. Проба после реального дополнительного job и 2 секунд разгона; такие же окна. >=5% принять, <-10% откатить, иначе ещё одно окно и принять только >=5%. EWMA не участвует в сравнении. Недостаток заданий не является отрицательным результатом. Сохранить pressure/cooldown, integrity error запрещает подтверждение.

Очередь: raw work с оставшимися потребителями; производительность без сетевого голодания, цель 8 секунд, 64..512 MiB, <=RAM/4. RAM<512 MiB запрещает рост. Нет измерений: старые 256 MiB compressed. Верхний порог блокирует рост, превышение 10 секунд уменьшает target на 1 (min1); возврат роста ниже половины. Текущие загрузки не прерывать.

## Task 2: ранняя материализация и буфер

Чанк публикуется после compressed SHA и flush, до конца пака. Защищённый дескриптор + абсолютный диапазон; append-only поколения, старые освобождаются после readers. Resume повторно проверяет полный префикс чанков. Не удерживать дескрипторы в итогах. Буфер 1 MiB на сетевую задачу, хэши по входящим байтам, flush перед chunk/Range/end, прежний sync_all на Range. Bounded blocking read/hash/decode. Commit может переносить файлы, но inventory/state/success ждут downloader barrier без staging reservation. Ошибки/отмена дренируются и откатываются.

## Task 3: сравнение и проверки

cfg(test) baseline (старые четыре механизма) и optimized (новые) в общем конвейере. Диагностика каждые 10 секунд и сразу при смене target; unique/received/verified, queue seconds/capacity, first materialization и waits.

После всех изменений: fmt --check и cargo test --release drive_pack_ --lib. Высокорисковые сценарии: реальные jobs, окно и дубликаты, early chunk через Range, corruption/generation/resume, barrier/state, cancel/rollback/staging, buffering и dynamic queue.

Один ABBA: baseline/optimized/optimized/baseline. Зафиксированный предыдущий manifest и files: 359 files, 8 packs, 475532317 bytes/run; cache пустой, seed одинаковый. Общий бюджет 2 GiB including retries; stop on error/budget. Ускорение только если обе пары быстрее и среднее >=10%. После всех jobs убрать только помеченные work dirs, отчёты оставить. Проверить SHA config/state до и после.

## Прогресс

- [x] Baseline-коммит и отдельная ветка.
- [x] Точные окна/очередь.
- [x] Ранняя материализация, buffer, barrier.
- [x] Функциональный пакет (ошибки исправлены, повторены только затронутые scopes).
- [x] ABBA и итоговый отчёт: среднее сокращение времени 13,31%, обе пары быстрее. Отчёт: `docs/superpowers/reports/2026-09-04-drive-pack-speed-local.md`. Тестовые данные убраны, protected config/state неизменны, публикации нет.
