// Live voice chat via Azure OpenAI's Realtime API (the same `scorp-tech-innovations`
// resource `ai.rs` already uses for commit messages, same az-CLI-token auth — no
// separate API key to manage). A phone-call model, like ChatGPT's voice mode: one
// click starts the call — the mic streams continuously and the server's own voice
// activity detection (`turn_detection: server_vad`) decides when you've stopped
// talking and replies on its own. A second click ends the call.
//
// Real barge-in (talk over it to interrupt, like ChatGPT) needs the mic to stay
// live WHILE the assistant is talking, which means its own voice coming back out
// the speaker must be removed from what the mic sends up — otherwise the server
// hears its own reply and answers itself. That's acoustic echo cancellation:
// `webrtc-audio-processing` wraps Google's AEC3 (the same canceller Chrome uses
// for `echoCancellation: true`, and what ChatGPT's browser/app voice mode relies
// on — not a bespoke trick). See `Aec` below.
//
// Mic capture, resampling, AEC, the WebSocket session, and speaker playback all
// run on one dedicated background thread; only transcript/text deltas cross to
// the UI thread (see `WorkerMsg::Voice*`).
//
// Protocol: wss://{resource}.cognitiveservices.azure.com/openai/realtime
//   ?api-version=2024-10-01-preview&deployment={deployment}
// Same event shape as OpenAI's own Realtime API (session.update, input_audio_buffer
// .append, server_vad-triggered speech_started/stopped + auto response.create,
// response.audio.delta, response.audio_transcript.delta,
// conversation.item.input_audio_transcription.completed, error, ...).
// Audio wire format is always PCM16 mono @ 24kHz, base64-encoded — capture/playback
// resample to/from whatever the local mic/speaker actually run at.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use webrtc_audio_processing::config::EchoCanceller;
use webrtc_audio_processing::{Config as AecConfig, Processor};
use winit::event_loop::EventLoopProxy;

use crate::marketplace::WorkerMsg;
use crate::mcp::McpRequest;

const API_VERSION: &str = "2024-10-01-preview";
const SAMPLE_RATE: u32 = 24_000; // required wire format for the Realtime API
// WebRTC's audio processing module only runs at one of a handful of fixed native
// rates (8/16/32/48kHz) in 10ms frames — everything on the mic/speaker side gets
// resampled to/from this rate specifically to feed it, separately from the
// wire's fixed 24kHz.
const AEC_RATE: u32 = 48_000;
const AEC_FRAME: usize = (AEC_RATE / 100) as usize; // 480 samples = 10ms @ 48kHz

fn deployment() -> String {
    std::env::var("AETHER_AZURE_VOICE_DEPLOYMENT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "gpt-realtime-2.1-mini".to_string())
}
fn resource_name() -> String {
    std::env::var("AETHER_AZURE_RESOURCE_NAME").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
        // Derived from the same endpoint `ai.rs` uses (https://{name}.cognitiveservices.azure.com).
        crate::ai::endpoint().trim_start_matches("https://").split('.').next().unwrap_or("").to_string()
    })
}
fn resource_group() -> String {
    std::env::var("AETHER_AZURE_RESOURCE_GROUP").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "openai".to_string())
}

// Cached resource API key: the Realtime API's preview WebSocket endpoint only
// accepts the resource key (`api-key` header) — the AAD bearer token `ai.rs` uses
// for chat completions gets a 401 PermissionDenied on this endpoint (verified
// live against the actual resource). `az` is slow to spawn, so cache like `ai.rs`
// caches its bearer token.
static KEY_CACHE: Mutex<Option<(String, Instant)>> = Mutex::new(None);
fn get_api_key() -> Result<String, String> {
    if let Ok(guard) = KEY_CACHE.lock() {
        if let Some((k, fetched)) = guard.as_ref() {
            if fetched.elapsed() < Duration::from_secs(45 * 60) {
                return Ok(k.clone());
            }
        }
    }
    let mut cmd = Command::new(if cfg!(windows) { "az.cmd" } else { "az" });
    cmd.args([
        "cognitiveservices",
        "account",
        "keys",
        "list",
        "--name",
        &resource_name(),
        "--resource-group",
        &resource_group(),
        "--query",
        "key1",
        "-o",
        "tsv",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("could not run `az` (is the Azure CLI installed and on PATH?): {e}"))?;
    if !out.status.success() {
        return Err(format!("az cognitiveservices account keys list failed (run `az login`): {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let key = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if key.is_empty() {
        return Err("az returned an empty API key".to_string());
    }
    if let Ok(mut guard) = KEY_CACHE.lock() {
        *guard = Some((key.clone(), Instant::now()));
    }
    Ok(key)
}

/// `mcp::tools::list()` produces MCP's `{name, description, inputSchema}` shape;
/// the Realtime API's function-calling wants a flat
/// `{type:"function", name, description, parameters}`. Same tools Claude Code
/// gets over the IDE-MCP channel — no separate/narrower set for voice.
fn tools_for_session() -> Value {
    let mcp_tools = crate::mcp::tools::list();
    let arr = mcp_tools.as_array().cloned().unwrap_or_default();
    Value::Array(
        arr.into_iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.get("name").cloned().unwrap_or(Value::Null),
                    "description": t.get("description").cloned().unwrap_or(Value::Null),
                    "parameters": t.get("inputSchema").cloned().unwrap_or(json!({"type":"object","properties":{}})),
                })
            })
            .collect(),
    )
}

/// Commands from the UI thread to the voice session.
pub enum VoiceCmd {
    Stop, // end the call
}

pub struct VoiceHandle {
    cmd_tx: Sender<VoiceCmd>,
    // Flipped true once the Realtime API session + mic/speaker are actually up
    // (see `WorkerMsg::VoiceConnected`) — lets the mic button show a distinct
    // "connecting" state instead of jumping straight to "live" on click, so a
    // fast VoiceError revert reads as "never connected" rather than "UI lag".
    connected: Arc<AtomicBool>,
}

impl VoiceHandle {
    pub fn stop(&self) {
        let _ = self.cmd_tx.send(VoiceCmd::Stop);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

fn wake(proxy: &Option<EventLoopProxy<()>>) {
    if let Some(p) = proxy {
        let _ = p.send_event(());
    }
}

/// Start a voice call in the background: connects to the Realtime API, opens the
/// mic + speaker (streaming continuously, echo-cancelled — server VAD decides
/// turns, and a real interruption gets through even while it's talking), and
/// runs until `Stop` (or a fatal error). Actual work happens on the spawned
/// thread; this returns immediately with a handle.
pub fn start(tx: Sender<WorkerMsg>, proxy: Option<EventLoopProxy<()>>, mcp_req_tx: Sender<McpRequest>) -> VoiceHandle {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<VoiceCmd>();
    let connected = Arc::new(AtomicBool::new(false));
    let connected_thread = connected.clone();
    std::thread::spawn(move || {
        if let Err(e) = run(&tx, &proxy, cmd_rx, mcp_req_tx, &connected_thread) {
            let _ = tx.send(WorkerMsg::VoiceError { message: e });
            wake(&proxy);
        }
    });
    VoiceHandle { cmd_tx, connected }
}

fn run(
    tx: &Sender<WorkerMsg>,
    proxy: &Option<EventLoopProxy<()>>,
    cmd_rx: Receiver<VoiceCmd>,
    mcp_req_tx: Sender<McpRequest>,
    connected: &Arc<AtomicBool>,
) -> Result<(), String> {
    let api_key = get_api_key()?;
    let host = crate::ai::endpoint();
    let host = host.trim_start_matches("https://").trim_end_matches('/');
    let url = format!("wss://{host}/openai/realtime?api-version={API_VERSION}&deployment={}", deployment());

    use tungstenite::client::IntoClientRequest;
    let mut req = url.into_client_request().map_err(|e| e.to_string())?;
    let key_value = api_key.parse().map_err(|_| "invalid api-key header value".to_string())?;
    req.headers_mut().insert("api-key", key_value);
    let (mut ws, _) = tungstenite::connect(req).map_err(|e| format!("Realtime API connect failed: {e}"))?;
    // Non-blocking-ish reads (a short timeout, not true nonblocking) so this one
    // thread can interleave inbound server events with outbound mic audio and UI
    // commands, instead of a blocking read starving everything else.
    set_read_timeout(&mut ws, Duration::from_millis(15));

    // Server-side voice-activity detection: the API itself watches the continuous
    // audio stream, detects when you start/stop talking (even mid-reply, since
    // the mic stays live — see the AEC setup below), auto-commits the buffer, and
    // creates/cancels responses accordingly. No manual turn boundaries needed.
    send(&mut ws, &json!({
        "type": "session.update",
        "session": {
            "modalities": ["audio", "text"],
            "voice": "alloy",
            "input_audio_format": "pcm16",
            "output_audio_format": "pcm16",
            "input_audio_transcription": { "model": "whisper-1" },
            "turn_detection": { "type": "server_vad", "interrupt_response": true },
            "tools": tools_for_session(),
            "tool_choice": "auto",
        }
    }))?;
    connected.store(true, Ordering::Relaxed);
    let _ = tx.send(WorkerMsg::VoiceConnected);
    wake(proxy);

    let host_cpal = cpal::default_host();
    let input_device = host_cpal.default_input_device().ok_or("no microphone found")?;
    let output_device = host_cpal.default_output_device().ok_or("no speaker/output device found")?;
    let in_cfg = input_device.default_input_config().map_err(|e| format!("mic config: {e}"))?;
    let out_cfg = output_device.default_output_config().map_err(|e| format!("speaker config: {e}"))?;
    let out_rate = out_cfg.sample_rate().0;

    let aec = Processor::new(AEC_RATE).map_err(|e| format!("couldn't start echo cancellation: {e:?}"))?;
    aec.set_config(AecConfig { echo_canceller: Some(EchoCanceller::default()), ..Default::default() });

    // Raw mic audio, downmixed to mono and resampled to the AEC's rate — NOT yet
    // echo-cancelled (that happens in fixed 10ms frames on the main loop below,
    // paired with the render reference, since AEC needs steady paced frames more
    // than it needs minimum latency).
    let capture_raw: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
    // Echo-cancelled mic audio waiting to be shipped to the Realtime API, at the
    // wire's 24kHz.
    let outgoing: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::new()));
    // Assistant audio waiting to be played — filled below, drained by the output
    // callback. Already resampled to the OUTPUT DEVICE's rate (independent of
    // the AEC's 48kHz).
    let playback_buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));

    let _input_stream = build_input_stream(&input_device, &in_cfg, capture_raw.clone())?;
    let _output_stream = build_output_stream(&output_device, &out_cfg, playback_buf.clone())?;

    // Paced at a fixed 10ms step (not "whenever data is available") — WebRTC's
    // AEC expects one process_capture_frame() per 10ms of real time, in lockstep
    // with the matching render reference, or its internal delay estimate drifts.
    let frame_period = Duration::from_millis(10);
    let mut next_frame_at = Instant::now();

    // Tool calls the model has asked for, dispatched to the UI thread over the
    // same `McpRequest` channel Claude Code's IDE-MCP connection uses (`App`
    // state can only be touched there). Each `reply` receiver is polled
    // non-blockingly below so a slow tool can't stall the audio pipeline.
    let mut pending_calls: Vec<(String, Receiver<Result<Value, String>>)> = Vec::new();
    // The API rejects a second `response.create` while one is still running
    // ("Conversation already has an active response in progress") — happens
    // whenever a turn makes multiple tool calls, since each one's result would
    // otherwise try to trigger its own continuation. Track the single active
    // response and defer to at most one queued `response.create`.
    let mut response_active = false;
    let mut response_create_pending = false;

    loop {
        match cmd_rx.try_recv() {
            Ok(VoiceCmd::Stop) => {
                let _ = tx.send(WorkerMsg::VoiceStopped);
                wake(proxy);
                return Ok(());
            }
            Err(TryRecvError::Disconnected) => {
                let _ = tx.send(WorkerMsg::VoiceStopped);
                wake(proxy);
                return Ok(());
            }
            Err(TryRecvError::Empty) => {}
        }

        // Run as many 10ms AEC frames as real time has actually advanced (catches
        // up after any stall instead of drifting further behind).
        while Instant::now() >= next_frame_at {
            next_frame_at += frame_period;
            // Peek (don't pop) what's actually queued to leave the speaker —
            // that IS the render reference, and staying in the same queue the
            // output callback drains keeps this locked to real playback timing
            // instead of drifting with decode-time network jitter.
            let mut render_frame = peek_for_aec(&playback_buf, out_rate);
            let _ = aec.analyze_render_frame([render_frame.as_mut_slice()]);
            let mut capture_frame = pull_or_pad(&capture_raw, AEC_FRAME);
            let _ = aec.process_capture_frame([capture_frame.as_mut_slice()]);
            let cleaned_i16: Vec<i16> =
                capture_frame.iter().map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).collect();
            let wire = resample_i16(&cleaned_i16, AEC_RATE, SAMPLE_RATE);
            outgoing.lock().unwrap().extend(wire);
        }

        flush_outgoing(&mut ws, &outgoing)?;

        // Poll outstanding tool calls; ship each finished result back to the
        // model the moment it's ready (not necessarily in request order).
        pending_calls.retain(|(call_id, rx)| {
            match rx.try_recv() {
                Ok(result) => {
                    let output = match result {
                        Ok(v) => v.to_string(),
                        Err(e) => json!({ "error": e }).to_string(),
                    };
                    let _ = send(&mut ws, &json!({
                        "type": "conversation.item.create",
                        "item": {
                            "type": "function_call_output",
                            "call_id": call_id,
                            "output": output,
                        }
                    }));
                    if response_active {
                        response_create_pending = true;
                    } else {
                        let _ = send(&mut ws, &json!({ "type": "response.create" }));
                        response_active = true;
                    }
                    false
                }
                Err(TryRecvError::Empty) => true,
                Err(TryRecvError::Disconnected) => false,
            }
        });

        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    match v.get("type").and_then(|k| k.as_str()).unwrap_or("") {
                        "response.created" => response_active = true,
                        "response.done" => {
                            response_active = false;
                            if response_create_pending {
                                response_create_pending = false;
                                let _ = send(&mut ws, &json!({ "type": "response.create" }));
                                response_active = true;
                            }
                        }
                        _ => {}
                    }
                    if let Some((call_id, name, args)) = handle_event(&v, tx, proxy, &playback_buf, out_rate) {
                        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
                        let _ = mcp_req_tx.send(McpRequest { tool: name, args, reply: reply_tx });
                        pending_calls.push((call_id, reply_rx));
                    }
                }
            }
            Ok(Message::Close(_)) => {
                let _ = tx.send(WorkerMsg::VoiceStopped);
                wake(proxy);
                return Ok(());
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(format!("Realtime API connection dropped: {e}")),
        }
    }
}

/// Pop up to `n` samples off the front of `buf`, padding the remainder with
/// silence — AEC frames must always be exactly `n` samples.
fn pull_or_pad(buf: &Arc<Mutex<VecDeque<f32>>>, n: usize) -> Vec<f32> {
    let mut b = buf.lock().unwrap();
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(b.pop_front().unwrap_or(0.0));
    }
    out
}

/// Non-destructively peeks the front of `playback_buf` (the exact queue the
/// output callback drains for real playback) and resamples it to the AEC's
/// rate for the render reference — NOT popped, so this doesn't compete with
/// actual playback for samples, and it can be called on a different cadence.
fn peek_for_aec(playback_buf: &Arc<Mutex<VecDeque<f32>>>, out_rate: u32) -> Vec<f32> {
    let needed_out = (AEC_FRAME as u64 * out_rate as u64 / AEC_RATE as u64) as usize + 2;
    let src: Vec<f32> = {
        let buf = playback_buf.lock().unwrap();
        buf.iter().take(needed_out).copied().collect()
    };
    let mut resampled = resample_f32(&src, out_rate, AEC_RATE);
    resampled.resize(AEC_FRAME, 0.0);
    resampled
}

/// Returns `Some((call_id, tool_name, args))` when the model just asked to run
/// a tool, so `run()`'s loop can dispatch it to the UI thread.
fn handle_event(
    v: &Value,
    tx: &Sender<WorkerMsg>,
    proxy: &Option<EventLoopProxy<()>>,
    playback_buf: &Arc<Mutex<VecDeque<f32>>>,
    out_rate: u32,
) -> Option<(String, String, Value)> {
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match kind {
        "response.function_call_arguments.done" => {
            let call_id = v.get("call_id").and_then(|c| c.as_str())?.to_string();
            let name = v.get("name").and_then(|n| n.as_str())?.to_string();
            let args_str = v.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
            let args: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
            return Some((call_id, name, args));
        }
        "response.audio.delta" => {
            if let Some(b64) = v.get("delta").and_then(|d| d.as_str()) {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                    let mono: Vec<i16> = bytes
                        .chunks_exact(2)
                        .map(|c| i16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    let for_playback = resample_i16(&mono, SAMPLE_RATE, out_rate);
                    playback_buf.lock().unwrap().extend(for_playback.into_iter().map(|s| s as f32 / i16::MAX as f32));
                }
            }
        }
        "response.audio_transcript.delta" | "response.text.delta" => {
            if let Some(d) = v.get("delta").and_then(|d| d.as_str()) {
                let _ = tx.send(WorkerMsg::VoiceAssistantTextDelta { text: d.to_string() });
                wake(proxy);
            }
        }
        "conversation.item.input_audio_transcription.delta" => {
            if let Some(d) = v.get("delta").and_then(|d| d.as_str()) {
                let _ = tx.send(WorkerMsg::VoiceUserTranscript { text: d.to_string(), done: false });
                wake(proxy);
            }
        }
        "conversation.item.input_audio_transcription.completed" => {
            let text = v.get("transcript").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let _ = tx.send(WorkerMsg::VoiceUserTranscript { text, done: true });
            wake(proxy);
        }
        // The server detected you talking over it: stop playing/queuing the old
        // reply immediately rather than letting it keep going in your ear while
        // the new turn is already being processed.
        "input_audio_buffer.speech_started" => {
            playback_buf.lock().unwrap().clear();
        }
        "response.done" => {
            let _ = tx.send(WorkerMsg::VoiceAssistantDone);
            wake(proxy);
        }
        "error" => {
            let msg = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown Realtime API error")
                .to_string();
            let _ = tx.send(WorkerMsg::VoiceError { message: msg });
            wake(proxy);
        }
        _ => {}
    }
    None
}

/// Drain whatever's been echo-cancelled since the last flush and append it to
/// the session as base64 PCM16. A no-op (not an error) when there's nothing new.
fn flush_outgoing(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, outgoing: &Arc<Mutex<VecDeque<i16>>>) -> Result<(), String> {
    let samples: Vec<i16> = {
        let mut buf = outgoing.lock().unwrap();
        buf.drain(..).collect()
    };
    if samples.is_empty() {
        return Ok(());
    }
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    send(ws, &json!({ "type": "input_audio_buffer.append", "audio": b64 }))
}

fn send(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, v: &Value) -> Result<(), String> {
    ws.send(Message::Text(v.to_string())).map_err(|e| format!("Realtime API send failed: {e}"))
}

fn set_read_timeout(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, d: Duration) {
    match ws.get_ref() {
        MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(Some(d));
        }
        MaybeTlsStream::Rustls(s) => {
            let _ = s.sock.set_read_timeout(Some(d));
        }
        _ => {}
    }
}

/// Downmix to mono + linearly resample between two sample rates. Good enough for
/// speech (no anti-aliasing filter) — a proper sinc resampler would be overkill
/// for a voice-call UI, and AEC3 itself is far more sensitive to timing than to
/// this stage's resampling artifacts.
fn resample_i16(input: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor().max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos.floor() as usize;
        let frac = src_pos - idx as f64;
        let a = input[idx.min(input.len() - 1)] as f64;
        let b = input[(idx + 1).min(input.len() - 1)] as f64;
        out.push((a + (b - a) * frac) as i16);
    }
    out
}

/// Same linear resampling as `resample_i16`, staying in the f32 domain (used
/// for the AEC render-reference peek, which is already normalized f32).
fn resample_f32(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor().max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos.floor() as usize;
        let frac = src_pos - idx as f64;
        let a = input[idx.min(input.len() - 1)] as f64;
        let b = input[(idx + 1).min(input.len() - 1)] as f64;
        out.push((a + (b - a) * frac) as f32);
    }
    out
}

fn build_input_stream(
    device: &cpal::Device,
    cfg: &cpal::SupportedStreamConfig,
    capture_raw: Arc<Mutex<VecDeque<f32>>>,
) -> Result<cpal::Stream, String> {
    let channels = cfg.channels();
    let device_rate = cfg.sample_rate().0;
    let stream_cfg: StreamConfig = cfg.clone().into();
    let err_fn = |e| eprintln!("[voice] mic stream error: {e}");
    // Resampling happens in the i16 domain (reusing `resample_i16`, already tuned
    // for speech) — the f32<->i16 round trip at this boundary costs negligible
    // precision for voice.
    let push = move |mono: Vec<i16>, buf: &Arc<Mutex<VecDeque<f32>>>| {
        let resampled = resample_i16(&mono, device_rate, AEC_RATE);
        buf.lock().unwrap().extend(resampled.into_iter().map(|s| s as f32 / i16::MAX as f32));
    };
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => {
            let buf = capture_raw.clone();
            device.build_input_stream(
                &stream_cfg,
                move |data: &[f32], _| {
                    let mono = downmix_f32_to_i16(data, channels);
                    push(mono, &buf);
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let buf = capture_raw.clone();
            device.build_input_stream(
                &stream_cfg,
                move |data: &[i16], _| {
                    let mono = downmix_i16_to_i16(data, channels);
                    push(mono, &buf);
                },
                err_fn,
                None,
            )
        }
        other => return Err(format!("unsupported mic sample format: {other:?}")),
    }
    .map_err(|e| format!("couldn't open microphone: {e}"))?;
    stream.play().map_err(|e| format!("couldn't start microphone: {e}"))?;
    Ok(stream)
}

fn build_output_stream(
    device: &cpal::Device,
    cfg: &cpal::SupportedStreamConfig,
    playback_buf: Arc<Mutex<VecDeque<f32>>>,
) -> Result<cpal::Stream, String> {
    let channels = cfg.channels() as usize;
    let stream_cfg: StreamConfig = cfg.clone().into();
    let err_fn = |e| eprintln!("[voice] speaker stream error: {e}");
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => {
            let buf = playback_buf.clone();
            device.build_output_stream(
                &stream_cfg,
                move |data: &mut [f32], _| fill_output(data, channels, &buf, |s| s),
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let buf = playback_buf.clone();
            device.build_output_stream(
                &stream_cfg,
                move |data: &mut [i16], _| fill_output(data, channels, &buf, |s: f32| (s * i16::MAX as f32) as i16),
                err_fn,
                None,
            )
        }
        other => return Err(format!("unsupported speaker sample format: {other:?}")),
    }
    .map_err(|e| format!("couldn't open speaker: {e}"))?;
    stream.play().map_err(|e| format!("couldn't start speaker: {e}"))?;
    Ok(stream)
}

/// Fill an output callback's buffer from the shared mono queue, duplicating each
/// mono sample across every channel (silence once the queue runs dry).
fn fill_output<T: Copy + Default>(
    data: &mut [T],
    channels: usize,
    playback_buf: &Arc<Mutex<VecDeque<f32>>>,
    conv: impl Fn(f32) -> T,
) {
    let mut buf = playback_buf.lock().unwrap();
    for frame in data.chunks_mut(channels.max(1)) {
        let s = buf.pop_front().unwrap_or(0.0);
        let v = conv(s);
        for out in frame {
            *out = v;
        }
    }
}

fn downmix_f32_to_i16(data: &[f32], channels: u16) -> Vec<i16> {
    let ch = channels.max(1) as usize;
    data.chunks(ch)
        .map(|frame| {
            let avg = frame.iter().sum::<f32>() / ch as f32;
            (avg.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
        })
        .collect()
}

fn downmix_i16_to_i16(data: &[i16], channels: u16) -> Vec<i16> {
    let ch = channels.max(1) as usize;
    data.chunks(ch)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|&s| s as i32).sum();
            (sum / ch as i32) as i16
        })
        .collect()
}
