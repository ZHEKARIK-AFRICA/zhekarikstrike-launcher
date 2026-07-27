use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{stream::FuturesUnordered, StreamExt};
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::{Client, StatusCode};
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::models::{ProgressEmitter, ProgressPayload, ProgressStage};
use crate::utils::hash_utils::sha256_file;
use crate::utils::time_utils::seconds_remaining;

const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct DownloadFileTask {
    pub url: String,
    pub relative_path: String,
    pub expected_size: Option<u64>,
    pub expected_sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedContentRange {
    start: u64,
    end: u64,
    total: u64,
}

pub async fn download_file(
    client: &Client,
    url: &str,
    target_path: &Path,
    progress: Option<ProgressEmitter>,
    cancel: CancellationToken,
    expected_size: Option<u64>,
    expected_sha256: Option<&str>,
) -> Result<DownloadResult, AppError> {
    let part_path = target_path.with_extension(
        target_path
            .extension()
            .map(|ext| format!("{}.part", ext.to_string_lossy()))
            .unwrap_or_else(|| "part".to_string()),
    );

    if let Some(parent) = target_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut last_error = None;
    for attempt in 0..=3 {
        if cancel.is_cancelled() {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(AppError::Canceled);
        }

        match try_download_file(
            client,
            url,
            &part_path,
            progress.clone(),
            cancel.clone(),
            expected_size,
        )
        .await
        {
            Ok(bytes) => {
                let verification = async {
                    validate_download_size(bytes, expected_size)?;
                    verify_download_hash(&part_path, expected_sha256).await
                }
                .await;
                if let Err(error) = verification {
                    last_error = Some(error);
                    let _ = tokio::fs::remove_file(&part_path).await;
                    if attempt < 3 {
                        let delay_ms = 500_u64.saturating_mul(2_u64.saturating_pow(attempt));
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                    continue;
                }

                if let Ok(metadata) = tokio::fs::symlink_metadata(target_path).await {
                    if metadata.is_dir() {
                        return Err(AppError::InvalidData(format!(
                            "download target is a directory: {}",
                            target_path.display()
                        )));
                    }
                    tokio::fs::remove_file(target_path).await?;
                }
                tokio::fs::rename(&part_path, target_path).await?;
                return Ok(DownloadResult {
                    path: target_path.to_path_buf(),
                    bytes,
                });
            }
            Err(AppError::Canceled) => {
                let _ = tokio::fs::remove_file(&part_path).await;
                return Err(AppError::Canceled);
            }
            Err(error) => {
                let preserve_partial = matches!(error, AppError::Network(_));
                last_error = Some(error);
                if !preserve_partial {
                    let _ = tokio::fs::remove_file(&part_path).await;
                }
                if attempt < 3 {
                    let delay_ms = 500_u64.saturating_mul(2_u64.saturating_pow(attempt));
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }
    }

    if !matches!(&last_error, Some(AppError::Network(_))) {
        let _ = tokio::fs::remove_file(&part_path).await;
    }
    Err(last_error.unwrap_or_else(|| AppError::Network("download failed".to_string())))
}

async fn try_download_file(
    client: &Client,
    url: &str,
    part_path: &Path,
    progress: Option<ProgressEmitter>,
    cancel: CancellationToken,
    expected_size: Option<u64>,
) -> Result<u64, AppError> {
    let mut offset = resumable_offset(part_path, expected_size).await?;
    if offset > 0 && Some(offset) == expected_size {
        return Ok(offset);
    }

    let mut request = client.get(url);
    if offset > 0 {
        request = request.header(RANGE, format!("bytes={offset}-"));
    }
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(AppError::Canceled),
        result = tokio::time::timeout(DOWNLOAD_IDLE_TIMEOUT, request.send()) => {
            match result {
                Ok(response) => response?,
                Err(_) => return Err(AppError::Network(
                    "download timed out waiting for response headers".to_string()
                )),
            }
        }
    };
    if offset > 0 && response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        return Err(AppError::InvalidData(format!(
            "server rejected resume offset {offset}"
        )));
    }
    response.error_for_status_ref()?;

    if offset > 0 {
        if response.status() == StatusCode::PARTIAL_CONTENT {
            validate_partial_response(&response, offset, expected_size)?;
        } else {
            offset = 0;
            validate_full_response(&response, expected_size)?;
        }
    } else {
        if response.status() == StatusCode::PARTIAL_CONTENT {
            return Err(AppError::InvalidData(
                "unexpected partial response for a full download".to_string(),
            ));
        }
        validate_full_response(&response, expected_size)?;
    }

    let total = expected_size.or_else(|| {
        response
            .content_length()
            .and_then(|length| length.checked_add(offset))
    });
    let mut stream = response.bytes_stream();
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).write(true);
    if offset > 0 {
        options.append(true);
    } else {
        options.truncate(true);
    }
    let mut file = options.open(part_path).await?;
    let mut downloaded = offset;
    let mut received = 0_u64;
    let start = Instant::now();

    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Canceled),
            result = tokio::time::timeout(DOWNLOAD_IDLE_TIMEOUT, stream.next()) => {
                match result {
                    Ok(chunk) => chunk,
                    Err(_) => return Err(AppError::Network(
                        "download stalled while waiting for data".to_string()
                    )),
                }
            }
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk?;
        let next_size = downloaded
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| AppError::InvalidData("download size overflow".to_string()))?;
        if let Some(expected) = expected_size {
            if next_size > expected {
                return Err(AppError::InvalidData(format!(
                    "download exceeded expected size {expected}"
                )));
            }
        }
        file.write_all(&chunk).await?;
        downloaded = next_size;
        received += chunk.len() as u64;

        if let Some(progress) = progress.as_ref() {
            let mut payload =
                ProgressPayload::new(progress.operation_id().to_string(), ProgressStage::Download);
            payload.downloaded_bytes = Some(downloaded);
            payload.total_bytes = total;
            payload.progress = total.map(|total| (downloaded as f64 / total as f64) * 100.0);
            payload.speed_bytes_per_sec =
                Some(received as f64 / start.elapsed().as_secs_f64().max(0.001));
            payload.time_remaining_sec = seconds_remaining(
                start,
                received,
                total.map(|total| total.saturating_sub(offset)),
            );
            progress.emit(payload)?;
        }
    }

    file.flush().await?;
    Ok(downloaded)
}

async fn resumable_offset(part_path: &Path, expected_size: Option<u64>) -> Result<u64, AppError> {
    let metadata = match tokio::fs::symlink_metadata(part_path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };

    if !metadata.file_type().is_file() {
        return Err(AppError::InvalidData(format!(
            "download partial is not a regular file: {}",
            part_path.display()
        )));
    }

    let Some(expected) = expected_size else {
        tokio::fs::remove_file(part_path).await?;
        return Ok(0);
    };
    if metadata.len() > expected {
        tokio::fs::remove_file(part_path).await?;
        return Ok(0);
    }

    Ok(metadata.len())
}

fn validate_full_response(
    response: &reqwest::Response,
    expected_size: Option<u64>,
) -> Result<(), AppError> {
    if let (Some(actual), Some(expected)) = (response.content_length(), expected_size) {
        validate_download_size(actual, Some(expected))?;
    }
    Ok(())
}

fn validate_partial_response(
    response: &reqwest::Response,
    offset: u64,
    expected_size: Option<u64>,
) -> Result<(), AppError> {
    let expected = expected_size.ok_or_else(|| {
        AppError::InvalidData("cannot validate a resumed download without its size".to_string())
    })?;
    let header = response
        .headers()
        .get(CONTENT_RANGE)
        .ok_or_else(|| {
            AppError::InvalidData("partial response is missing Content-Range".to_string())
        })?
        .to_str()
        .map_err(|_| AppError::InvalidData("Content-Range is not valid ASCII".to_string()))?;
    let parsed = parse_content_range(header)?;
    if parsed.start != offset
        || parsed.total != expected
        || parsed.end.checked_add(1) != Some(expected)
    {
        return Err(AppError::InvalidData(format!(
            "unexpected Content-Range: {header}"
        )));
    }

    let remaining = expected
        .checked_sub(offset)
        .ok_or_else(|| AppError::InvalidData("resume offset exceeds expected size".to_string()))?;
    let content_length = response.content_length().ok_or_else(|| {
        AppError::InvalidData("partial response is missing Content-Length".to_string())
    })?;
    validate_download_size(content_length, Some(remaining))
}

fn parse_content_range(value: &str) -> Result<ParsedContentRange, AppError> {
    let value = value
        .strip_prefix("bytes ")
        .ok_or_else(|| AppError::InvalidData(format!("invalid Content-Range unit: {value}")))?;
    let (range, total) = value
        .split_once('/')
        .ok_or_else(|| AppError::InvalidData(format!("invalid Content-Range: {value}")))?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| AppError::InvalidData(format!("invalid Content-Range: {value}")))?;
    let start = start
        .parse::<u64>()
        .map_err(|_| AppError::InvalidData(format!("invalid Content-Range start: {value}")))?;
    let end = end
        .parse::<u64>()
        .map_err(|_| AppError::InvalidData(format!("invalid Content-Range end: {value}")))?;
    let total = total
        .parse::<u64>()
        .map_err(|_| AppError::InvalidData(format!("invalid Content-Range total: {value}")))?;
    if start > end || end >= total {
        return Err(AppError::InvalidData(format!(
            "invalid Content-Range bounds: {value}"
        )));
    }

    Ok(ParsedContentRange { start, end, total })
}

fn validate_download_size(actual: u64, expected: Option<u64>) -> Result<(), AppError> {
    if let Some(expected) = expected {
        if actual != expected {
            return Err(AppError::InvalidData(format!(
                "download size mismatch: expected {expected}, received {actual}"
            )));
        }
    }
    Ok(())
}

async fn verify_download_hash(
    part_path: &Path,
    expected_sha256: Option<&str>,
) -> Result<(), AppError> {
    if let Some(expected) = expected_sha256 {
        let actual = sha256_file(part_path).await?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(AppError::InvalidData(format!(
                "sha256 mismatch for {}",
                part_path.display()
            )));
        }
    }

    Ok(())
}

pub async fn download_files_parallel(
    client: Client,
    files: Vec<DownloadFileTask>,
    target_root: PathBuf,
    concurrency: usize,
    progress: ProgressEmitter,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    let concurrency = concurrency.clamp(1, crate::constants::MAX_DOWNLOAD_CONCURRENCY);
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let total = files.len().max(1);
    let mut completed = 0_usize;
    let mut futures = FuturesUnordered::new();

    for task in files {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let client = client.clone();
        let target = target_root.join(&task.relative_path);
        let progress = progress.clone();
        let task_cancel = cancel.clone();

        futures.push(tokio::spawn(async move {
            let _permit = permit;
            download_file(
                &client,
                &task.url,
                &target,
                None,
                task_cancel,
                task.expected_size,
                task.expected_sha256.as_deref(),
            )
            .await
        }));

        if cancel.is_cancelled() {
            return Err(AppError::Canceled);
        }

        while futures.len() >= concurrency {
            if let Some(result) = futures.next().await {
                result.map_err(|error| AppError::Unknown(error.to_string()))??;
                completed += 1;
                progress.emit_stage(
                    ProgressStage::Download,
                    Some((completed as f64 / total as f64) * 100.0),
                    None,
                )?;
            }
        }
    }

    while let Some(result) = futures.next().await {
        result.map_err(|error| AppError::Unknown(error.to_string()))??;
        completed += 1;
        progress.emit_stage(
            ProgressStage::Download,
            Some((completed as f64 / total as f64) * 100.0),
            None,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    use super::{download_file, parse_content_range, validate_download_size, ParsedContentRange};
    use crate::error::AppError;

    async fn read_request(socket: &mut TcpStream) -> String {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = socket.read(&mut buffer).await.expect("request should read");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&request).to_string()
    }

    #[test]
    fn download_size_must_match_the_signed_manifest_value() {
        validate_download_size(9_153_970_381, Some(9_153_970_381))
            .expect("exact archive size should pass");
        assert!(validate_download_size(9_153_970_380, Some(9_153_970_381)).is_err());
        assert!(validate_download_size(9_153_970_382, Some(9_153_970_381)).is_err());
        validate_download_size(123, None).expect("legacy unbounded callers should still work");
    }

    #[test]
    fn content_range_must_be_complete_and_well_formed() {
        assert_eq!(
            parse_content_range("bytes 5-11/12").expect("valid range should parse"),
            ParsedContentRange {
                start: 5,
                end: 11,
                total: 12,
            }
        );
        for invalid in [
            "items 5-11/12",
            "bytes */12",
            "bytes 5-4/12",
            "bytes 5-12/12",
            "bytes five-11/12",
        ] {
            assert!(parse_content_range(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[tokio::test]
    async fn interrupted_download_resumes_from_the_existing_part_file() {
        let payload = b"hello world!";
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener should expose its address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = requests.clone();

        let server = tokio::spawn(async move {
            for _ in 0..4 {
                let (mut socket, _) = listener.accept().await.expect("request should connect");
                let request = read_request(&mut socket).await;
                let resumed = request
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case("range: bytes=5-"));
                server_requests.lock().await.push(request);

                if resumed {
                    socket
                        .write_all(
                            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 7\r\nContent-Range: bytes 5-11/12\r\nConnection: close\r\n\r\n world!",
                        )
                        .await
                        .expect("resume response should write");
                    break;
                }

                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nhello",
                    )
                    .await
                    .expect("interrupted response should write");
            }
        });

        let directory = tempdir().expect("temporary directory should exist");
        let target = directory.path().join("archive.zip");
        let expected_hash = hex::encode(Sha256::digest(payload));
        let result = download_file(
            &reqwest::Client::new(),
            &format!("http://{address}/archive"),
            &target,
            None,
            CancellationToken::new(),
            Some(payload.len() as u64),
            Some(&expected_hash),
        )
        .await;

        server.await.expect("test server should finish");
        result.expect("interrupted download should resume");
        assert_eq!(
            tokio::fs::read(&target)
                .await
                .expect("downloaded file should exist"),
            payload
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(requests[1]
            .lines()
            .any(|line| line.eq_ignore_ascii_case("range: bytes=5-")));
    }

    #[tokio::test]
    async fn rejected_resume_discards_the_part_and_restarts_from_zero() {
        let payload = b"hello world!";
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener should expose its address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = requests.clone();

        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (mut socket, _) = listener.accept().await.expect("request should connect");
                let request = read_request(&mut socket).await;
                server_requests.lock().await.push(request);
                if attempt == 0 {
                    socket
                        .write_all(
                            b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nContent-Range: bytes */12\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .expect("range rejection should write");
                } else {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nhello world!",
                        )
                        .await
                        .expect("full retry should write");
                }
            }
        });

        let directory = tempdir().expect("temporary directory should exist");
        let target = directory.path().join("archive.zip");
        let part = directory.path().join("archive.zip.part");
        tokio::fs::write(&part, b"hello")
            .await
            .expect("partial download should exist");
        let expected_hash = hex::encode(Sha256::digest(payload));
        download_file(
            &reqwest::Client::new(),
            &format!("http://{address}/archive"),
            &target,
            None,
            CancellationToken::new(),
            Some(payload.len() as u64),
            Some(&expected_hash),
        )
        .await
        .expect("rejected range should restart safely");

        server.await.expect("test server should finish");
        assert_eq!(tokio::fs::read(&target).await.unwrap(), payload);
        let requests = requests.lock().await;
        assert!(requests[0]
            .lines()
            .any(|line| line.eq_ignore_ascii_case("range: bytes=5-")));
        assert!(!requests[1]
            .lines()
            .any(|line| line.to_ascii_lowercase().starts_with("range:")));
    }

    #[tokio::test]
    async fn server_ignoring_range_restarts_without_appending_to_the_part() {
        let payload = b"hello world!";
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("request should connect");
            let request = read_request(&mut socket).await;
            assert!(request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("range: bytes=5-")));
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nhello world!",
                )
                .await
                .expect("full response should write");
        });

        let directory = tempdir().expect("temporary directory should exist");
        let target = directory.path().join("archive.zip");
        let part = directory.path().join("archive.zip.part");
        tokio::fs::write(&part, b"hello")
            .await
            .expect("partial download should exist");
        let expected_hash = hex::encode(Sha256::digest(payload));
        download_file(
            &reqwest::Client::new(),
            &format!("http://{address}/archive"),
            &target,
            None,
            CancellationToken::new(),
            Some(payload.len() as u64),
            Some(&expected_hash),
        )
        .await
        .expect("a full response should safely replace the partial download");

        server.await.expect("test server should finish");
        assert_eq!(tokio::fs::read(&target).await.unwrap(), payload);
    }

    #[tokio::test]
    async fn completed_part_with_a_bad_hash_is_redownloaded() {
        let payload = b"hello world!";
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("request should connect");
            let request = read_request(&mut socket).await;
            assert!(!request
                .lines()
                .any(|line| line.to_ascii_lowercase().starts_with("range:")));
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nhello world!",
                )
                .await
                .expect("full response should write");
        });

        let directory = tempdir().expect("temporary directory should exist");
        let target = directory.path().join("archive.zip");
        let part = directory.path().join("archive.zip.part");
        tokio::fs::write(&part, b"HELLO WORLD!")
            .await
            .expect("corrupt completed part should exist");
        let expected_hash = hex::encode(Sha256::digest(payload));
        download_file(
            &reqwest::Client::new(),
            &format!("http://{address}/archive"),
            &target,
            None,
            CancellationToken::new(),
            Some(payload.len() as u64),
            Some(&expected_hash),
        )
        .await
        .expect("bad completed part should be redownloaded");

        server.await.expect("test server should finish");
        assert_eq!(tokio::fs::read(&target).await.unwrap(), payload);
    }

    #[tokio::test]
    async fn cancellation_removes_the_partial_download() {
        let directory = tempdir().expect("temporary directory should exist");
        let target = directory.path().join("archive.zip");
        let part = directory.path().join("archive.zip.part");
        tokio::fs::write(&part, b"hello")
            .await
            .expect("partial download should exist");
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = download_file(
            &reqwest::Client::new(),
            "http://127.0.0.1:1/archive",
            &target,
            None,
            cancel,
            Some(12),
            None,
        )
        .await;

        assert!(matches!(result, Err(AppError::Canceled)));
        assert!(!part.exists());
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_stalled_response_body() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("request should connect");
            let _ = read_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\n")
                .await
                .expect("response headers should write");
            tokio::time::sleep(Duration::from_secs(10)).await;
        });

        let directory = tempdir().expect("temporary directory should exist");
        let target = directory.path().join("archive.zip");
        let part = directory.path().join("archive.zip.part");
        let cancel = CancellationToken::new();
        let cancel_later = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_later.cancel();
        });

        let result = download_file(
            &reqwest::Client::new(),
            &format!("http://{address}/archive"),
            &target,
            None,
            cancel,
            Some(12),
            None,
        )
        .await;

        server.abort();
        assert!(matches!(result, Err(AppError::Canceled)));
        assert!(!part.exists());
    }
}
