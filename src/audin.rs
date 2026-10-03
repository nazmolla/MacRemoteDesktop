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

/// Writes the microphone into the ring file the VigaMic CoreAudio driver reads
/// (`mic-driver/VigaMic.c`): a header, then interleaved int16 stereo at 44.1 kHz.
pub struct RingSink {
    map: std::sync::Mutex<Option<RingMap>>,
    format: std::sync::Mutex<Option<AudioFormat>>,
}

struct RingMap {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is only touched under the sink's mutex.
unsafe impl Send for RingMap {}

const RING_PATH: &str = "/Users/Shared/Viga/mic.ring";
const RING_MAGIC: u32 = 0x5647_414D;
const RING_RATE: u32 = 44_100;
const RING_FRAMES: u32 = RING_RATE * 2;
const HEADER: usize = 24;

impl RingSink {
    pub fn new() -> Self {
        Self { map: std::sync::Mutex::new(None), format: std::sync::Mutex::new(None) }
    }

    fn open(&self) -> std::io::Result<RingMap> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let dir = std::path::Path::new(RING_PATH).parent().expect("ring path has a parent");
        std::fs::create_dir_all(dir)?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
        let len = HEADER + RING_FRAMES as usize * 4;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o644)
            .open(RING_PATH)?;
        file.set_len(len as u64)?;
        use std::os::fd::AsRawFd;
        // SAFETY: maps `len` bytes of a file just sized to `len`; checked for MAP_FAILED.
        let ptr = unsafe {
            libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, file.as_raw_fd(), 0)
        };
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        let ptr = ptr.cast::<u8>();
        // SAFETY: the header fits in the mapping; written before any reader can see the magic.
        unsafe {
            for (i, v) in [0u32, 2, RING_FRAMES].iter().enumerate() {
                ptr.add(4 + i * 4).cast::<u32>().write_unaligned(if i == 0 { RING_RATE } else { *v });
            }
            ptr.add(16).cast::<u64>().write_unaligned(0);
            ptr.cast::<u32>().write_unaligned(RING_MAGIC);
        }
        Ok(RingMap { ptr, len })
    }
}

impl Default for RingSink {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RingSink {
    fn drop(&mut self) {
        if let Some(m) = self.map.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            // SAFETY: unmaps the mapping created in `open`.
            unsafe { libc::munmap(m.ptr.cast(), m.len) };
        }
    }
}

impl MicSink for RingSink {
    fn start(&self, format: &AudioFormat) {
        use crate::sync_ext::LockExt;
        tracing::info!(rate = format.samples_per_sec, channels = format.channels, "microphone stream opened (virtual microphone)");
        *self.format.lock_or_recover() = Some(format.clone());
        let mut map = self.map.lock_or_recover();
        if map.is_none() {
            match self.open() {
                Ok(m) => *map = Some(m),
                Err(e) => tracing::warn!(error = %e, "microphone: cannot open the virtual microphone ring"),
            }
        }
    }

    fn samples(&self, pcm: &[u8]) {
        use crate::sync_ext::LockExt;
        let Some(fmt) = self.format.lock_or_recover().clone() else { return };
        let map = self.map.lock_or_recover();
        let Some(m) = map.as_ref() else { return };
        let ch = usize::from(fmt.channels.max(1));
        let input: Vec<i16> = pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let in_frames = input.len() / ch;
        // Resample by nearest frame to 44.1 kHz stereo.
        let out_frames = (in_frames as u64 * u64::from(RING_RATE) / u64::from(fmt.samples_per_sec.max(1))) as usize;
        // SAFETY: indices stay within the RING_FRAMES-frame data area; the write
        // position is published after the frames (release ordering).
        unsafe {
            let pos_ptr = m.ptr.add(16).cast::<std::sync::atomic::AtomicU64>();
            let mut pos = (*pos_ptr).load(std::sync::atomic::Ordering::Relaxed);
            let data = m.ptr.add(HEADER).cast::<i16>();
            for i in 0..out_frames {
                let src = (i * in_frames / out_frames.max(1)).min(in_frames.saturating_sub(1)) * ch;
                let (l, r) = (input[src], input[src + ch - 1]);
                let slot = (pos % u64::from(RING_FRAMES)) as usize * 2;
                data.add(slot).write(l);
                data.add(slot + 1).write(r);
                pos += 1;
            }
            (*pos_ptr).store(pos, std::sync::atomic::Ordering::Release);
        }
    }
}
