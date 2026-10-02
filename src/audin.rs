//! The client's microphone (MS-RDPEAI "AUDIO_INPUT" channel). Negotiates 16-bit
//! PCM, opens the stream, and hands each audio packet to a [`MicSink`].

use ironrdp_dvc::{DvcMessage, DvcProcessor};
use ironrdp_pdu::PduResult;
use ironrdp_server::{DvcFactory, RawDvcMessage};

use crate::audin_pdu::{decode_client, encode_server, pcm, AudioFormat, ClientPdu, ServerPdu};

/// Receives the client's microphone audio as interleaved 16-bit PCM.
pub trait MicSink: Send + Sync {
    fn start(&self, format: &AudioFormat);
    fn samples(&self, pcm: &[u8]);
}

/// Logs the stream so the channel can be verified without an audio device.
#[derive(Default)]
pub struct LogSink {
    bytes: std::sync::atomic::AtomicU64,
}

impl MicSink for LogSink {
    fn start(&self, format: &AudioFormat) {
        tracing::info!(rate = format.samples_per_sec, channels = format.channels, "microphone stream opened");
    }
    fn samples(&self, pcm: &[u8]) {
        let total = self
            .bytes
            .fetch_add(pcm.len() as u64, std::sync::atomic::Ordering::Relaxed)
            + pcm.len() as u64;
        if total / 1_000_000 != (total - pcm.len() as u64) / 1_000_000 {
            tracing::info!(total_bytes = total, "microphone audio received");
        }
    }
}

pub struct AudinFactory {
    sink: std::sync::Arc<dyn MicSink>,
}

impl AudinFactory {
    pub fn new(sink: std::sync::Arc<dyn MicSink>) -> Self {
        Self { sink }
    }
}

impl DvcFactory for AudinFactory {
    fn build(&self) -> Box<dyn DvcProcessor> {
        Box::new(Audin { sink: self.sink.clone(), offered: server_formats(), chosen: None })
    }
}

/// What we can take, in preference order.
fn server_formats() -> Vec<AudioFormat> {
    vec![pcm(44100, 2), pcm(48000, 2), pcm(44100, 1), pcm(16000, 1)]
}

struct Audin {
    sink: std::sync::Arc<dyn MicSink>,
    offered: Vec<AudioFormat>,
    chosen: Option<AudioFormat>,
}

fn msg(pdu: &ServerPdu) -> DvcMessage {
    RawDvcMessage::new(encode_server(pdu))
}

impl ironrdp_core::AsAny for Audin {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

impl DvcProcessor for Audin {
    fn channel_name(&self) -> &str {
        "AUDIO_INPUT"
    }

    fn start(&mut self, _channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        Ok(vec![msg(&ServerPdu::Version(1))])
    }

    fn process(&mut self, _channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        let pdu = match decode_client(payload) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "microphone: bad message, ignored");
                return Ok(Vec::new());
            }
        };
        match pdu {
            ClientPdu::Version(v) => {
                tracing::info!(version = v, "microphone channel opened by the client");
                Ok(vec![msg(&ServerPdu::Formats(self.offered.clone()))])
            }
            ClientPdu::Formats(theirs) => {
                // The client answers with the subset of our list it supports; the
                // Open's initial format indexes that list.
                let Some((index, format)) = theirs
                    .iter()
                    .enumerate()
                    .find(|(_, f)| f.tag == 1 && f.bits_per_sample == 16)
                else {
                    tracing::info!("microphone: the client offers no 16-bit PCM format");
                    return Ok(Vec::new());
                };
                // About 20 ms per packet.
                let frames = format.samples_per_sec / 50;
                self.chosen = Some(format.clone());
                Ok(vec![msg(&ServerPdu::Open {
                    frames_per_packet: frames,
                    initial_format: index as u32,
                    format: format.clone(),
                })])
            }
            ClientPdu::OpenReply(hresult) => {
                match (&self.chosen, hresult) {
                    (Some(f), 0) => self.sink.start(f),
                    (_, h) => tracing::warn!(hresult = format!("{h:#x}"), "microphone: the client could not open it"),
                }
                Ok(Vec::new())
            }
            ClientPdu::FormatChange(i) => {
                tracing::info!(format = i, "microphone format change");
                Ok(Vec::new())
            }
            ClientPdu::DataIncoming => Ok(Vec::new()),
            ClientPdu::Data(bytes) => {
                self.sink.samples(&bytes);
                Ok(Vec::new())
            }
        }
    }
}
