//! LiveKit agent worker protocol, client side, in Rust.
//!
//! Checked against livekit-server v1.13.7 (the deployed version) and the
//! `livekit-protocol` 0.8.0 crate (protocol 1.50.4): `livekit_agent.proto` is
//! byte-identical to the protocol revision v1.13.7 pins (a4f4b5c0c23f).
//! Server behaviour this relies on (`pkg/service/agentservice.go`,
//! `pkg/agent/worker.go`, `pkg/service/wsprotocol.go` at v1.13.7):
//! - WebSocket at `<LIVEKIT_URL>/agent?protocol=1`, `Authorization: Bearer <jwt>`
//!   where the JWT carries the `video.agent` grant.
//! - Binary frames are protobuf `WorkerMessage` / `ServerMessage`.
//! - The first message must be `register` within 10 s; the server answers
//!   `register` with the worker id.
//! - Per job: `availability` request → the worker must answer within 10 s with
//!   `AvailabilityResponse` → `assignment` carrying the job token. The server
//!   leaves `JobAssignment.url` empty, so the worker joins `LIVEKIT_URL`.
//! - `ping` → `pong`; `update_job` reports JS_RUNNING / JS_SUCCESS / JS_FAILED.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures::future::BoxFuture;
use futures::{SinkExt, StreamExt};
use livekit_protocol as proto;
use prost::Message as _;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// `?protocol=` value (livekit-server `agent.CurrentProtocol`).
pub const WORKER_PROTOCOL: u32 = 1;
const PING_EVERY: Duration = Duration::from_secs(10);
const PONG_DEADLINE: Duration = Duration::from_secs(35);
const REGISTER_DEADLINE: Duration = Duration::from_secs(10);

/// An assigned job, everything the call needs to join the room.
#[derive(Debug, Clone)]
pub struct JobInfo {
    pub job_id: String,
    pub room_name: String,
    /// Room join token the server minted for this job.
    pub token: String,
    /// Usually empty on self-hosted LiveKit: join `LIVEKIT_URL`.
    pub url: Option<String>,
    /// Dispatch attributes (outbound dispatch carries `direction`, `consentRef`, …).
    pub attributes: HashMap<String, String>,
    pub metadata: String,
}

/// What the worker does with jobs. Implemented by `room.rs` (and by tests).
pub trait JobHandler: Send + Sync + 'static {
    /// Accept this offer? (capacity check; also refuse foreign room prefixes)
    fn available(&self, job: &proto::Job) -> bool;
    /// Run an accepted job to completion.
    fn run(self: Arc<Self>, job: JobInfo) -> BoxFuture<'static, Result<(), String>>;
    /// The server terminated the job (room deleted, worker drained).
    fn terminate(&self, job_id: &str);
    /// Current `(load 0..1, running jobs)` for `update_worker`.
    fn load(&self) -> (f32, u32);
}

fn worker_msg(m: proto::worker_message::Message) -> Message {
    Message::Binary(proto::WorkerMessage { message: Some(m) }.encode_to_vec())
}

pub fn register_message(agent_name: &str) -> proto::WorkerMessage {
    proto::WorkerMessage {
        message: Some(proto::worker_message::Message::Register(
            proto::RegisterWorkerRequest {
                r#type: proto::JobType::JtRoom as i32,
                agent_name: agent_name.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
        )),
    }
}

fn job_status(
    job_id: &str,
    status: proto::JobStatus,
    error: String,
) -> proto::worker_message::Message {
    proto::worker_message::Message::UpdateJob(proto::UpdateJobStatus {
        job_id: job_id.to_string(),
        status: status as i32,
        error,
    })
}

/// Keep a worker registered forever, reconnecting with backoff. Running calls
/// are independent room connections and survive a dispatch reconnect.
pub async fn run_dispatch(
    ws_url: String,
    agent_name: String,
    token: impl Fn() -> Result<String> + Send + Sync,
    handler: Arc<dyn JobHandler>,
) -> Result<()> {
    let mut delay = Duration::from_millis(500);
    loop {
        let started = Instant::now();
        let res = match token() {
            Ok(t) => run_session(&ws_url, &agent_name, &t, handler.clone()).await,
            Err(e) => Err(e),
        };
        if let Err(e) = res {
            tracing::warn!("agent dispatch connection ended: {e:#}");
        }
        if started.elapsed() > Duration::from_secs(60) {
            delay = Duration::from_millis(500);
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(15));
    }
}

/// One registered connection to `/agent`, until it drops.
pub async fn run_session(
    ws_url: &str,
    agent_name: &str,
    token: &str,
    handler: Arc<dyn JobHandler>,
) -> Result<()> {
    let mut req = ws_url.into_client_request().context("agent url")?;
    req.headers_mut().insert(
        "Authorization",
        format!("Bearer {token}").parse().context("token header")?,
    );
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .context("connect /agent")?;
    let (mut sink, mut stream) = ws.split();

    sink.send(Message::Binary(
        register_message(agent_name).encode_to_vec(),
    ))
    .await?;
    let worker_id = tokio::time::timeout(REGISTER_DEADLINE, async {
        while let Some(msg) = stream.next().await {
            if let Message::Binary(b) = msg? {
                if let Ok(proto::ServerMessage {
                    message: Some(proto::server_message::Message::Register(r)),
                }) = proto::ServerMessage::decode(b.as_slice())
                {
                    return Ok(r.worker_id);
                }
            }
        }
        bail!("server closed before register response")
    })
    .await
    .context("register timed out")??;
    tracing::info!(%worker_id, agent_name, "registered with LiveKit as agent worker");

    // Job tasks report status here; the loop owns the socket.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<proto::worker_message::Message>();
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let mut last_pong = Instant::now();

    loop {
        tokio::select! {
            msg = stream.next() => {
                let msg = match msg {
                    Some(m) => m?,
                    None => bail!("agent socket closed"),
                };
                let bytes = match msg {
                    Message::Binary(b) => b,
                    Message::Close(_) => bail!("agent socket closed by server"),
                    _ => continue,
                };
                let Ok(proto::ServerMessage { message: Some(m) }) = proto::ServerMessage::decode(bytes.as_slice()) else {
                    tracing::warn!("undecodable agent server message");
                    continue;
                };
                match m {
                    proto::server_message::Message::Availability(req) => {
                        let job = req.job.unwrap_or_default();
                        let available = handler.available(&job);
                        tracing::info!(job_id = %job.id, room = ?job.room.as_ref().map(|r| &r.name), available, "job offer");
                        sink.send(worker_msg(proto::worker_message::Message::Availability(proto::AvailabilityResponse {
                            job_id: job.id.clone(),
                            available,
                            participant_identity: format!("agent-{}", job.id),
                            participant_name: "Allternit voice".into(),
                            ..Default::default()
                        }))).await?;
                    }
                    proto::server_message::Message::Assignment(a) => {
                        let job = a.job.unwrap_or_default();
                        let info = JobInfo {
                            job_id: job.id.clone(),
                            room_name: job.room.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
                            token: a.token,
                            url: a.url.filter(|u| !u.is_empty()),
                            attributes: job.attributes.clone(),
                            metadata: job.metadata.clone(),
                        };
                        sink.send(worker_msg(job_status(&info.job_id, proto::JobStatus::JsRunning, String::new()))).await?;
                        let h = handler.clone();
                        let tx = out_tx.clone();
                        tokio::spawn(async move {
                            let id = info.job_id.clone();
                            let (status, err) = match h.clone().run(info).await {
                                Ok(()) => (proto::JobStatus::JsSuccess, String::new()),
                                Err(e) => (proto::JobStatus::JsFailed, e),
                            };
                            let _ = tx.send(job_status(&id, status, err));
                            let (load, job_count) = h.load();
                            let _ = tx.send(proto::worker_message::Message::UpdateWorker(proto::UpdateWorkerStatus {
                                status: None,
                                load,
                                job_count,
                            }));
                        });
                        let (load, job_count) = handler.load();
                        sink.send(worker_msg(proto::worker_message::Message::UpdateWorker(proto::UpdateWorkerStatus {
                            status: None,
                            load,
                            job_count,
                        }))).await?;
                    }
                    proto::server_message::Message::Termination(t) => handler.terminate(&t.job_id),
                    proto::server_message::Message::Pong(_) => last_pong = Instant::now(),
                    proto::server_message::Message::Register(_) => {}
                }
            }
            Some(m) = out_rx.recv() => sink.send(worker_msg(m)).await?,
            _ = ping.tick() => {
                if last_pong.elapsed() > PONG_DEADLINE {
                    bail!("no pong from LiveKit in {PONG_DEADLINE:?}");
                }
                sink.send(worker_msg(proto::worker_message::Message::Ping(proto::WorkerPing {
                    timestamp: chrono::Utc::now().timestamp_millis(),
                }))).await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    struct FakeHandler {
        capacity: u32,
        running: AtomicU32,
        ran: Mutex<Vec<JobInfo>>,
        terminated: Mutex<Vec<String>>,
    }

    impl JobHandler for FakeHandler {
        fn available(&self, job: &proto::Job) -> bool {
            job.room
                .as_ref()
                .is_some_and(|r| r.name.starts_with("call-"))
                && self.running.load(Ordering::SeqCst) < self.capacity
        }
        fn run(self: Arc<Self>, job: JobInfo) -> BoxFuture<'static, Result<(), String>> {
            self.running.fetch_add(1, Ordering::SeqCst);
            self.ran.lock().unwrap().push(job);
            Box::pin(async { Ok(()) })
        }
        fn terminate(&self, job_id: &str) {
            self.terminated.lock().unwrap().push(job_id.into());
        }
        fn load(&self) -> (f32, u32) {
            (0.0, self.running.load(Ordering::SeqCst))
        }
    }

    fn server_msg(m: proto::server_message::Message) -> Message {
        Message::Binary(proto::ServerMessage { message: Some(m) }.encode_to_vec())
    }

    fn job(id: &str, room: &str) -> proto::Job {
        proto::Job {
            id: id.into(),
            r#type: proto::JobType::JtRoom as i32,
            room: Some(proto::Room {
                name: room.into(),
                ..Default::default()
            }),
            agent_name: "allternit-voice".into(),
            ..Default::default()
        }
    }

    async fn next_worker_msg<S>(ws: &mut S) -> proto::worker_message::Message
    where
        S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        loop {
            let Message::Binary(b) = ws.next().await.unwrap().unwrap() else {
                continue;
            };
            return proto::WorkerMessage::decode(b.as_slice())
                .unwrap()
                .message
                .unwrap();
        }
    }

    /// A mock LiveKit `/agent` endpoint speaking the agent protos.
    #[tokio::test]
    #[allow(clippy::result_large_err)] // tungstenite's handshake callback signature
    async fn register_offer_assign_and_report() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = Arc::new(FakeHandler {
            capacity: 1,
            running: AtomicU32::new(0),
            ran: Mutex::default(),
            terminated: Mutex::default(),
        });

        let server = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut ws =
                tokio_tungstenite::accept_hdr_async(sock, |req: &Request, resp: Response| {
                    assert_eq!(req.uri().path(), "/agent");
                    assert_eq!(req.uri().query(), Some("protocol=1"));
                    assert_eq!(req.headers().get("authorization").unwrap(), "Bearer jwt-1");
                    Ok(resp)
                })
                .await
                .unwrap();

            let proto::worker_message::Message::Register(reg) = next_worker_msg(&mut ws).await
            else {
                panic!()
            };
            assert_eq!(reg.agent_name, "allternit-voice");
            assert_eq!(reg.r#type, proto::JobType::JtRoom as i32);
            ws.send(server_msg(proto::server_message::Message::Register(
                proto::RegisterWorkerResponse {
                    worker_id: "W_1".into(),
                    server_info: None,
                },
            )))
            .await
            .unwrap();

            // Offer a call room: accepted.
            ws.send(server_msg(proto::server_message::Message::Availability(
                proto::AvailabilityRequest {
                    job: Some(job("AJ_1", "call-abc")),
                    resuming: false,
                },
            )))
            .await
            .unwrap();
            let proto::worker_message::Message::Availability(a) = next_worker_msg(&mut ws).await
            else {
                panic!()
            };
            assert_eq!(a.job_id, "AJ_1");
            assert!(a.available);
            assert_eq!(a.participant_identity, "agent-AJ_1");

            let mut j = job("AJ_1", "call-abc");
            j.attributes.insert("direction".into(), "inbound".into());
            ws.send(server_msg(proto::server_message::Message::Assignment(
                proto::JobAssignment {
                    job: Some(j),
                    url: None,
                    token: "room-token".into(),
                },
            )))
            .await
            .unwrap();

            let mut statuses = vec![];
            while statuses.len() < 2 {
                if let proto::worker_message::Message::UpdateJob(u) = next_worker_msg(&mut ws).await
                {
                    statuses.push(u.status);
                }
            }
            assert_eq!(
                statuses,
                [
                    proto::JobStatus::JsRunning as i32,
                    proto::JobStatus::JsSuccess as i32
                ]
            );

            // At capacity now: a second offer is declined. A non-call room too.
            for (id, room) in [("AJ_2", "call-def"), ("AJ_3", "meeting-1")] {
                ws.send(server_msg(proto::server_message::Message::Availability(
                    proto::AvailabilityRequest {
                        job: Some(job(id, room)),
                        resuming: false,
                    },
                )))
                .await
                .unwrap();
                loop {
                    if let proto::worker_message::Message::Availability(a) =
                        next_worker_msg(&mut ws).await
                    {
                        assert_eq!(a.job_id, id);
                        assert!(!a.available);
                        break;
                    }
                }
            }

            ws.send(server_msg(proto::server_message::Message::Termination(
                proto::JobTermination {
                    job_id: "AJ_1".into(),
                },
            )))
            .await
            .unwrap();
            ws.send(server_msg(proto::server_message::Message::Pong(
                proto::WorkerPong::default(),
            )))
            .await
            .unwrap();
            ws.close(None).await.unwrap();
        });

        let err = run_session(
            &format!("ws://{addr}/agent?protocol=1"),
            "allternit-voice",
            "jwt-1",
            handler.clone(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("closed"), "{err:#}");
        server.await.unwrap();
        let ran = handler.ran.lock().unwrap();
        assert_eq!(ran.len(), 1);
        assert_eq!(ran[0].room_name, "call-abc");
        assert_eq!(ran[0].token, "room-token");
        assert_eq!(ran[0].url, None);
        assert_eq!(
            ran[0].attributes.get("direction").map(String::as_str),
            Some("inbound")
        );
        assert_eq!(*handler.terminated.lock().unwrap(), ["AJ_1"]);
    }

    #[test]
    fn register_encodes() {
        let bytes = register_message("allternit-voice").encode_to_vec();
        let back = proto::WorkerMessage::decode(bytes.as_slice()).unwrap();
        let Some(proto::worker_message::Message::Register(r)) = back.message else {
            panic!()
        };
        assert_eq!(r.agent_name, "allternit-voice");
    }
}
