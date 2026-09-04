//! Bounded HTTP readers. The parent keeps exactly one sequential file writer.
use super::*;
use std::sync::atomic::AtomicBool;
use tokio::sync::OwnedSemaphorePermit;
use tokio::task::JoinHandle;

const BUFFER_BLOCK: usize = 64 * 1024;
const BUFFER_SLOTS: usize = 32; // <=2 MiB per reader, including its reserved read slot.
pub(super) const PREFETCH_TRIGGER: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct Measurement {
    pub bytes: u64,
    pub header: f64,
    pub body: f64,
}
pub(super) struct RangePipe {
    pub range: ByteRange,
    pub url: Url,
    rx: mpsc::Receiver<Vec<u8>>,
    task: Option<JoinHandle<Result<Measurement, AttemptError>>>,
    stop: CancellationToken,
    handed_off: Arc<AtomicBool>,
    buffered: Arc<AtomicU64>,
    threshold: Option<tokio::sync::oneshot::Receiver<()>>,
}
impl Drop for RangePipe {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl RangePipe {
    pub fn lookahead(
        &mut self,
        context: RequestContext,
        next: ByteRange,
    ) -> JoinHandle<Option<RangePipe>> {
        let threshold = self.threshold.take().expect("one lookahead per range");
        tokio::spawn(async move {
            tokio::select! {
                _ = context.cancel.cancelled() => None,
                _ = context.preempt.cancelled() => None,
                signal = threshold => if signal.is_ok() { context.prefetch(next) } else {None},
            }
        })
    }
    pub async fn next(&mut self) -> Option<Vec<u8>> {
        self.handed_off.store(true, Ordering::Relaxed);
        let bytes = self.rx.recv().await?;
        self.buffered
            .fetch_sub(bytes.len() as u64, Ordering::Relaxed);
        Some(bytes)
    }
    pub async fn finish(&mut self, cancel: bool) -> Result<Measurement, AttemptError> {
        if cancel {
            self.stop.cancel();
            self.rx.close();
        }
        // Receiver must not hold a task waiting for buffer space during cleanup.
        while let Ok(bytes) = self.rx.try_recv() {
            self.buffered
                .fetch_sub(bytes.len() as u64, Ordering::Relaxed);
        }
        match self.task.take().expect("range joined once").await {
            Ok(result) => result,
            Err(e) => Err(fatal(AppError::Unknown(format!(
                "pack HTTP reader failed: {e}"
            )))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn drive_pack_http2_reader_and_demand_prefetch_limits() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!(
            "http://{}/download",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let stop = CancellationToken::new();
        let server_stop = stop.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut connection = h2::server::handshake(socket).await.unwrap();
            loop {
                tokio::select! {
                    _ = server_stop.cancelled()=>break,
                    request=connection.accept()=> {
                        let Some(Ok((request,mut sender)))=request else {break};
                        assert_eq!(request.headers()["range"],"bytes=0-3");
                        let response=http::Response::builder().status(206)
                            .header("content-length","4").header("content-range","bytes 0-3/4").body(()).unwrap();
                        sender.send_response(response,false).unwrap().send_data(bytes::Bytes::from_static(b"test"),true).unwrap();
                    }
                }
            }
        });
        let options = PackRunOptions::new("h2");
        let context = RequestContext {
            client: Client::builder()
                .http2_adaptive_window(true)
                .http2_prior_knowledge()
                .build()
                .unwrap(),
            url,
            pack_sha: "a".repeat(64),
            pack_size: 4,
            cancel: CancellationToken::new(),
            preempt: CancellationToken::new(),
            options: options.clone(),
        };
        let range = ByteRange {
            start: 0,
            end_inclusive: 3,
        };
        options.metrics.materializer(1, 1, 0., 256 * 1024 * 1024);
        assert!(context.prefetch(range).is_none());
        options.metrics.materializer(1, 1, 0., 4096 * 1024 * 1024);
        let all = options
            .http_slots
            .clone()
            .acquire_many_owned(6)
            .await
            .unwrap();
        assert!(context.prefetch(range).is_none());
        let demand = options.http_slots.clone().acquire_owned();
        tokio::pin!(demand);
        assert!(futures_util::poll!(&mut demand).is_pending());
        drop(all);
        let wanted = demand.await.unwrap();
        assert_eq!(options.http_slots.available_permits(), 5);
        drop(wanted);
        let mut pipe = context.demand(range).await.unwrap();
        let mut body = Vec::new();
        while let Some(bytes) = pipe.next().await {
            body.extend(bytes);
        }
        pipe.finish(false).await.unwrap();
        assert_eq!(body, b"test");
        assert!(options
            .metrics
            .snapshot()
            .http_protocols
            .contains_key("HTTP/2.0"));
        assert_eq!(options.metrics.snapshot().active_requests, 0);
        assert_eq!(options.http_slots.available_permits(), 6);
        stop.cancel();
        server.await.unwrap();
    }
}

#[derive(Clone)]
pub(super) struct RequestContext {
    pub client: Client,
    pub url: Url,
    pub pack_sha: String,
    pub pack_size: u64,
    pub cancel: CancellationToken,
    pub preempt: CancellationToken,
    pub options: PackRunOptions,
}
impl RequestContext {
    pub async fn demand(&self, range: ByteRange) -> Result<RangePipe, AttemptError> {
        let permit = tokio::select! {
            _ = self.cancel.cancelled() => return Err(cancelled()),
            _ = self.preempt.cancelled() => return Err(preempted()),
            p = self.options.http_slots.clone().acquire_owned() => p.map_err(|_|fatal(AppError::Canceled))?,
        };
        Ok(self.spawn(range, permit, false))
    }
    pub fn prefetch(&self, range: ByteRange) -> Option<RangePipe> {
        if self.cancel.is_cancelled()
            || self.preempt.is_cancelled()
            || self.options.metrics.snapshot().available_memory < 512 * 1024 * 1024
        {
            return None;
        }
        // Tokio's FIFO semaphore reserves capacity for queued demand waiters;
        // speculation never waits in that queue or steals a demanded slot.
        let permit = self.options.http_slots.clone().try_acquire_owned().ok()?;
        Some(self.spawn(range, permit, true))
    }
    fn spawn(&self, range: ByteRange, permit: OwnedSemaphorePermit, prefetch: bool) -> RangePipe {
        let (tx, rx) = mpsc::channel(BUFFER_SLOTS);
        let (threshold_tx, threshold) = tokio::sync::oneshot::channel();
        let stop = self.cancel.child_token();
        let handed_off = Arc::new(AtomicBool::new(!prefetch));
        let buffered = Arc::new(AtomicU64::new(0));
        let context = self.clone();
        let local_stop = stop.clone();
        let ready = handed_off.clone();
        let outstanding = buffered.clone();
        let task = tokio::spawn(async move {
            let _slot = permit;
            let expected = range.len().map_err(fatal)?;
            let mut guard = context.options.request(expected).map_err(fatal)?;
            context.options.metrics.request_started(prefetch);
            let started = Instant::now();
            let request = context
                .client
                .get(context.url.clone())
                .header(header::ACCEPT_ENCODING, "identity")
                .header(
                    header::RANGE,
                    format!("bytes={}-{}", range.start, range.end_inclusive),
                );
            let response = send_attempt(request, &local_stop, &context.preempt).await?;
            let header_time = started.elapsed().as_secs_f64();
            context.options.metrics.response_headers(
                &format!("{:?}", response.version()),
                header_time,
                prefetch,
            );
            validate_common_response(&response, &context.url, &context.pack_sha)?;
            validate_range_headers(&response, range, context.pack_size)?;
            let stream = response
                .bytes_stream()
                .map(|r| r.map_err(std::io::Error::other));
            let mut input = tokio_util::io::StreamReader::new(stream);
            let mut received = 0_u64;
            let mut body = 0.;
            let mut buffer_wait = 0.;
            let mut threshold_tx = Some(threshold_tx);
            let result: Result<(),AttemptError> = async {
                loop {
                    let wait=Instant::now();
                    let capacity=tokio::select! {
                        _ = local_stop.cancelled() => return Err(cancelled_with_bytes(received)),
                        _ = context.preempt.cancelled() => return Err(preempted_with_bytes(received)),
                        p = tx.reserve() => p.map_err(|_|cancelled_with_bytes(received))?,
                    };
                    buffer_wait += wait.elapsed().as_secs_f64();
                    let mut bytes=vec![0;BUFFER_BLOCK];
                    let reading=Instant::now();
                    let n=tokio::select! {
                        _ = local_stop.cancelled() => return Err(cancelled_with_bytes(received)),
                        _ = context.preempt.cancelled() => return Err(preempted_with_bytes(received)),
                        r = tokio::time::timeout(IDLE_TIMEOUT,input.read(&mut bytes)) => match r {
                            Ok(Ok(n)) => n,
                            Ok(Err(e)) => return Err(retryable(AppError::Network(e.to_string()),None,true,false,received)),
                            Err(_) => return Err(retryable(AppError::Network("Google Drive pack download stalled".into()),None,true,false,received)),
                        },
                    };
                    body += reading.elapsed().as_secs_f64();
                    if n==0 {break;}
                    guard.received(n as u64).map_err(fatal)?;
                    let unique=context.options.traffic.observe(&context.pack_sha,range.start+received,range.start+received+n as u64);
                    context.options.metrics.unique(unique);
                    received+=n as u64;
                    if received>expected {return Err(integrity("pack response exceeded range",received));}
                    if expected-received <= PREFETCH_TRIGGER {
                        if let Some(signal) = threshold_tx.take() { let _ = signal.send(()); }
                    }
                    bytes.truncate(n);
                    let queued=outstanding.fetch_add(n as u64,Ordering::Relaxed)+n as u64;
                    if !ready.load(Ordering::Relaxed) {context.options.metrics.prefetched(n as u64,queued);}
                    capacity.send(bytes);
                }
                if received!=expected {return Err(retryable(AppError::Network("pack response ended before EOF".into()),None,true,false,received));}
                Ok(())
            }.await;
            guard.response_finished();
            context
                .options
                .metrics
                .transport_time(body, buffer_wait, 0.);
            result?;
            Ok(Measurement {
                bytes: received,
                header: header_time,
                body,
            })
        });
        RangePipe {
            range,
            url: self.url.clone(),
            rx,
            task: Some(task),
            stop,
            handed_off,
            buffered,
            threshold: Some(threshold),
        }
    }
}
