//! Voice Session test client: streams a WAV file as mic audio in real time,
//! prints every event, and writes the speech it receives to a WAV file.
//!
//! ```text
//! cargo run -p voice-service --example voice_session_client -- \
//!     --wav question.wav --say "It is noon. Anything else?" --out reply.wav
//! ```
//!
//! Options:
//!   --url URL        default ws://127.0.0.1:8001/v1/voice/session
//!   --token T        sidecar token (default: env ALLTERNIT_VOICE_TOKEN)
//!   --wav FILE       PCM16 WAV (mono or stereo, any rate) to stream as mic audio
//!   --say TEXT       reply to speak: sent as speak.delta pieces + speak.done,
//!                    after turn.ended when --wav is given, else right away
//!   --out FILE       where to write received speech (default voice_session_out.wav)
//!   --turn MODE      smart | vad
//!   --voice ID       TTS voice
//!   --tail-ms N      silence streamed after the WAV (default 2000)
//!   --barge-wav FILE speak this WAV (same rate as --wav) over the reply, to test
//!                    barge-in; --barge-after-ms N after speak.started (default 800)
//!   --wait-ms N      max wait for the reply to finish (default 20000)

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug)]
struct Args {
    url: String,
    token: Option<String>,
    wav: Option<String>,
    say: Option<String>,
    out: String,
    turn: Option<String>,
    voice: Option<String>,
    tail_ms: u64,
    wait_ms: u64,
    barge_wav: Option<String>,
    barge_after_ms: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        url: "ws://127.0.0.1:8001/v1/voice/session".into(),
        token: std::env::var("ALLTERNIT_VOICE_TOKEN")
            .ok()
            .filter(|t| !t.is_empty()),
        wav: None,
        say: None,
        out: "voice_session_out.wav".into(),
        turn: None,
        voice: None,
        tail_ms: 2000,
        wait_ms: 20_000,
        barge_wav: None,
        barge_after_ms: 800,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--url" => a.url = val()?,
            "--token" => a.token = Some(val()?),
            "--wav" => a.wav = Some(val()?),
            "--say" => a.say = Some(val()?),
            "--out" => a.out = val()?,
            "--turn" => a.turn = Some(val()?),
            "--voice" => a.voice = Some(val()?),
            "--tail-ms" => a.tail_ms = val()?.parse().map_err(|e| format!("--tail-ms: {e}"))?,
            "--barge-wav" => a.barge_wav = Some(val()?),
            "--barge-after-ms" => {
                a.barge_after_ms = val()?
                    .parse()
                    .map_err(|e| format!("--barge-after-ms: {e}"))?
            }
            "--wait-ms" => a.wait_ms = val()?.parse().map_err(|e| format!("--wait-ms: {e}"))?,
            "-h" | "--help" => {
                println!("see the header of examples/voice_session_client.rs for usage");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.wav.is_none() && a.say.is_none() {
        return Err("give --wav and/or --say".into());
    }
    Ok(a)
}

/// Read a PCM16 WAV as mono samples + rate.
fn read_wav(path: &str) -> Result<(Vec<i16>, u32), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("{path}: not a RIFF/WAVE file"));
    }
    let (mut pos, mut fmt, mut data) = (12usize, None, None);
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..(pos + 8 + len).min(bytes.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                let format = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes([body[14], body[15]]);
                fmt = Some((format, channels, rate, bits));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    let (format, channels, rate, bits) = fmt.ok_or("no fmt chunk")?;
    if !(format == 1 || format == 0xFFFE) || bits != 16 || channels == 0 {
        return Err(format!(
            "{path}: need 16-bit PCM (got format {format}, {bits} bits)"
        ));
    }
    let data = data.ok_or("no data chunk")?;
    let samples: Vec<i16> = data
        .chunks_exact(2 * channels as usize)
        .map(|f| {
            let sum: i32 = f
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]) as i32)
                .sum();
            (sum / channels as i32) as i16
        })
        .collect();
    Ok((samples, rate))
}

fn write_wav(path: &str, pcm: &[u8], rate: u32) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    std::fs::write(path, out)
}

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

/// Send `text` as word-sized `speak.delta` pieces (LLM-like pacing), then `speak.done`.
async fn say(tx: &mut WsSink, text: String) -> Result<(), String> {
    for piece in text.split_inclusive(' ') {
        let m = json!({"type": "speak.delta", "id": "reply-1", "text": piece});
        tx.send(Message::Text(m.to_string()))
            .await
            .map_err(|e| e.to_string())?;
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let done = json!({"type": "speak.done", "id": "reply-1"});
    tx.send(Message::Text(done.to_string()))
        .await
        .map_err(|e| e.to_string())
}

/// Wait up to `ms` for an event whose type is in `want`. A fatal error or a
/// closed session is an `Err`; the timeout is the outer `Err`.
async fn wait(
    want: &[&str],
    ev_rx: &mut mpsc::UnboundedReceiver<Value>,
    ms: u64,
) -> Result<Result<Value, String>, tokio::time::error::Elapsed> {
    let fut = async {
        while let Some(ev) = ev_rx.recv().await {
            if ev["type"] == "error" && ev["fatal"] == true {
                return Err(format!("fatal error: {ev}"));
            }
            if want.contains(&ev["type"].as_str().unwrap_or("")) {
                return Ok(ev);
            }
        }
        Err("session closed".to_string())
    };
    tokio::time::timeout(Duration::from_millis(ms), fut).await
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    let wav = match &args.wav {
        Some(p) => Some(read_wav(p)?),
        None => None,
    };
    let mut url = args.url.clone();
    if let Some(t) = &args.token {
        url.push_str(if url.contains('?') {
            "&token="
        } else {
            "?token="
        });
        url.push_str(t);
    }
    let (ws, _) = tokio_tungstenite::connect_async(url.as_str())
        .await
        .map_err(|e| format!("connect {}: {e}", args.url))?;
    let (mut tx, mut rx) = ws.split();
    let t0 = Instant::now();

    let mut start = json!({"type": "session.start"});
    if let Some((_, rate)) = &wav {
        start["inputSampleRate"] = json!(rate);
    }
    if let Some(v) = &args.voice {
        start["voice"] = json!(v);
    }
    if let Some(m) = &args.turn {
        start["turn"] = json!({"mode": m});
    }
    tx.send(Message::Text(start.to_string()))
        .await
        .map_err(|e| e.to_string())?;

    // Reader: print events, collect speech audio, tell the main task what happened.
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<Value>();
    let reader = tokio::spawn(async move {
        let mut speech = Vec::<u8>::new();
        let mut out_rate = 24_000u32;
        while let Some(Ok(msg)) = rx.next().await {
            match msg {
                Message::Text(t) => {
                    let ev: Value = serde_json::from_str(&t).unwrap_or(json!({"raw": t}));
                    println!("+{:>6}ms {}", t0.elapsed().as_millis(), ev);
                    if ev["type"] == "session.ready" {
                        out_rate = ev["outputSampleRate"].as_u64().unwrap_or(24_000) as u32;
                    }
                    let _ = ev_tx.send(ev);
                }
                Message::Binary(b) => {
                    if speech.is_empty() {
                        println!("+{:>6}ms (first speech audio)", t0.elapsed().as_millis());
                    }
                    speech.extend_from_slice(&b)
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        (speech, out_rate)
    });

    wait(&["session.ready"], &mut ev_rx, 120_000)
        .await
        .map_err(|_| "no session.ready within 120 s".to_string())??;

    let mut started: Option<Instant> = None;
    if let Some((samples, rate)) = wav {
        // Stream the file then trailing silence, 20 ms per frame, in real time.
        let frame = (rate / 50) as usize;
        let tail = vec![0i16; (rate as u64 * args.tail_ms / 1000) as usize];
        let all: Vec<i16> = samples.into_iter().chain(tail).collect();
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        let mut replied = false;
        for chunk in all.chunks(frame) {
            tick.tick().await;
            let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
            tx.send(Message::Binary(bytes))
                .await
                .map_err(|e| e.to_string())?;
            while let Ok(ev) = ev_rx.try_recv() {
                if ev["type"] == "speak.started" {
                    started = Some(Instant::now());
                }
                if ev["type"] == "turn.ended" && !replied {
                    if let Some(text) = args.say.clone() {
                        replied = true;
                        say(&mut tx, text).await?;
                    }
                }
            }
        }
        if args.say.is_some() && !replied {
            match wait(&["turn.ended"], &mut ev_rx, args.wait_ms).await {
                Ok(Ok(_)) => say(&mut tx, args.say.clone().unwrap_or_default()).await?,
                _ => eprintln!("no turn.ended: not sending --say"),
            }
        }
    } else if let Some(text) = args.say.clone() {
        say(&mut tx, text).await?;
    }

    if let (Some(p), true) = (&args.barge_wav, args.say.is_some()) {
        let (samples, rate) = read_wav(p)?;
        let seen = match started {
            Some(at) => Some(at),
            None => match wait(&["speak.started"], &mut ev_rx, args.wait_ms).await {
                Ok(Ok(_)) => Some(Instant::now()),
                _ => None,
            },
        };
        match seen {
            Some(at) => {
                let due = Duration::from_millis(args.barge_after_ms);
                tokio::time::sleep(due.saturating_sub(at.elapsed())).await;
                println!(
                    "+{:>6}ms (barge-in: speaking over the reply)",
                    t0.elapsed().as_millis()
                );
                let mut tick = tokio::time::interval(Duration::from_millis(20));
                for chunk in samples.chunks((rate / 50) as usize) {
                    tick.tick().await;
                    let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
                    tx.send(Message::Binary(bytes))
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            None => eprintln!("no speak.started: barge-in not sent"),
        }
    }

    if args.say.is_some() {
        match wait(
            &["speak.ended", "speak.interrupted"],
            &mut ev_rx,
            args.wait_ms,
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => eprintln!("{e}"),
            Err(_) => eprintln!("reply did not finish within {} ms", args.wait_ms),
        }
        // Let paced audio that is still in flight arrive.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let _ = tx
        .send(Message::Text(json!({"type": "session.end"}).to_string()))
        .await;
    let (speech, rate) = reader.await.map_err(|e| e.to_string())?;
    write_wav(&args.out, &speech, rate).map_err(|e| format!("{}: {e}", args.out))?;
    println!(
        "wrote {} ({:.2} s of speech at {rate} Hz)",
        args.out,
        speech.len() as f64 / 2.0 / rate as f64
    );
    Ok(())
}
