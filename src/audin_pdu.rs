//! Audio Input channel messages (MS-RDPEAI 2.2): the client's microphone,
//! redirected to the server. Pure encode/decode; little-endian throughout.



#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFormat {
    pub tag: u16,
    pub channels: u16,
    pub samples_per_sec: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub extra: Vec<u8>,
}

/// 16-bit PCM at `samples_per_sec` with `channels` channels.
pub fn pcm(samples_per_sec: u32, channels: u16) -> AudioFormat {
    let block_align = channels * 2;
    AudioFormat {
        tag: 1,
        channels,
        samples_per_sec,
        avg_bytes_per_sec: samples_per_sec * u32::from(block_align),
        block_align,
        bits_per_sample: 16,
        extra: Vec::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerPdu {
    Version(u32),
    Formats(Vec<AudioFormat>),
    Open { frames_per_packet: u32, initial_format: u32, format: AudioFormat },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientPdu {
    Version(u32),
    Formats(Vec<AudioFormat>),
    OpenReply(u32),
    DataIncoming,
    Data(Vec<u8>),
    FormatChange(u32),
}

const VERSION: u8 = 0x01;
const FORMATS: u8 = 0x02;
const OPEN: u8 = 0x03;
const OPEN_REPLY: u8 = 0x04;
const DATA_INCOMING: u8 = 0x05;
const DATA: u8 = 0x06;
const FORMAT_CHANGE: u8 = 0x07;

fn put_format(out: &mut Vec<u8>, f: &AudioFormat) {
    out.extend_from_slice(&f.tag.to_le_bytes());
    out.extend_from_slice(&f.channels.to_le_bytes());
    out.extend_from_slice(&f.samples_per_sec.to_le_bytes());
    out.extend_from_slice(&f.avg_bytes_per_sec.to_le_bytes());
    out.extend_from_slice(&f.block_align.to_le_bytes());
    out.extend_from_slice(&f.bits_per_sample.to_le_bytes());
    out.extend_from_slice(&(f.extra.len() as u16).to_le_bytes());
    out.extend_from_slice(&f.extra);
}

fn put_formats(id: u8, formats: &[AudioFormat]) -> Vec<u8> {
    let mut body = Vec::new();
    for f in formats {
        put_format(&mut body, f);
    }
    let mut out = vec![id];
    out.extend_from_slice(&(formats.len() as u32).to_le_bytes());
    out.extend_from_slice(&((9 + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.b.len());
        let end = end.ok_or_else(|| format!("message truncated at byte {}", self.pos))?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2 bytes")))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    fn format(&mut self) -> Result<AudioFormat, String> {
        let tag = self.u16()?;
        let channels = self.u16()?;
        let samples_per_sec = self.u32()?;
        let avg_bytes_per_sec = self.u32()?;
        let block_align = self.u16()?;
        let bits_per_sample = self.u16()?;
        let cb = usize::from(self.u16()?);
        Ok(AudioFormat {
            tag,
            channels,
            samples_per_sec,
            avg_bytes_per_sec,
            block_align,
            bits_per_sample,
            extra: self.take(cb)?.to_vec(),
        })
    }
    fn formats(&mut self) -> Result<Vec<AudioFormat>, String> {
        let n = self.u32()?;
        let _size = self.u32()?;
        // Each format is at least 18 bytes, which bounds a hostile count.
        if n as usize > self.b.len() / 18 {
            return Err(format!("format count {n} exceeds the message"));
        }
        (0..n).map(|_| self.format()).collect()
    }
}

pub fn encode_server(pdu: &ServerPdu) -> Vec<u8> {
    match pdu {
        ServerPdu::Version(v) => [&[VERSION][..], &v.to_le_bytes()].concat(),
        ServerPdu::Formats(f) => put_formats(FORMATS, f),
        ServerPdu::Open { frames_per_packet, initial_format, format } => {
            let mut out = vec![OPEN];
            out.extend_from_slice(&frames_per_packet.to_le_bytes());
            out.extend_from_slice(&initial_format.to_le_bytes());
            put_format(&mut out, format);
            out
        }
    }
}

#[cfg(test)]
pub fn decode_server(bytes: &[u8]) -> Result<ServerPdu, String> {
    let mut r = Reader { b: bytes, pos: 0 };
    match r.take(1)?[0] {
        VERSION => Ok(ServerPdu::Version(r.u32()?)),
        FORMATS => Ok(ServerPdu::Formats(r.formats()?)),
        OPEN => Ok(ServerPdu::Open {
            frames_per_packet: r.u32()?,
            initial_format: r.u32()?,
            format: r.format()?,
        }),
        id => Err(format!("unknown server message {id:#04x}")),
    }
}

#[cfg(test)]
pub fn encode_client(pdu: &ClientPdu) -> Vec<u8> {
    match pdu {
        ClientPdu::Version(v) => [&[VERSION][..], &v.to_le_bytes()].concat(),
        ClientPdu::Formats(f) => put_formats(FORMATS, f),
        ClientPdu::OpenReply(h) => [&[OPEN_REPLY][..], &h.to_le_bytes()].concat(),
        ClientPdu::DataIncoming => vec![DATA_INCOMING],
        ClientPdu::Data(d) => [&[DATA][..], d].concat(),
        ClientPdu::FormatChange(f) => [&[FORMAT_CHANGE][..], &f.to_le_bytes()].concat(),
    }
}

pub fn decode_client(bytes: &[u8]) -> Result<ClientPdu, String> {
    let mut r = Reader { b: bytes, pos: 0 };
    match r.take(1)?[0] {
        VERSION => Ok(ClientPdu::Version(r.u32()?)),
        FORMATS => Ok(ClientPdu::Formats(r.formats()?)),
        OPEN_REPLY => Ok(ClientPdu::OpenReply(r.u32()?)),
        DATA_INCOMING => Ok(ClientPdu::DataIncoming),
        DATA => Ok(ClientPdu::Data(bytes[1..].to_vec())),
        FORMAT_CHANGE => Ok(ClientPdu::FormatChange(r.u32()?)),
        id => Err(format!("unknown client message {id:#04x}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_messages_round_trip() {
        for pdu in [
            ServerPdu::Version(2),
            ServerPdu::Formats(vec![pcm(44100, 2), pcm(16000, 1)]),
            ServerPdu::Open { frames_per_packet: 1024, initial_format: 0, format: pcm(44100, 2) },
        ] {
            assert_eq!(decode_server(&encode_server(&pdu)).unwrap(), pdu);
        }
    }

    #[test]
    fn client_messages_round_trip() {
        let mut odd = pcm(8000, 1);
        odd.extra = vec![1, 2, 3];
        for pdu in [
            ClientPdu::Version(1),
            ClientPdu::Formats(vec![odd, pcm(44100, 2)]),
            ClientPdu::OpenReply(0),
            ClientPdu::DataIncoming,
            ClientPdu::Data(vec![9, 8, 7, 6]),
            ClientPdu::FormatChange(1),
        ] {
            assert_eq!(decode_client(&encode_client(&pdu)).unwrap(), pdu);
        }
    }

    #[test]
    fn formats_size_field_is_the_message_length() {
        let bytes = encode_server(&ServerPdu::Formats(vec![pcm(44100, 2)]));
        assert_eq!(u32::from_le_bytes(bytes[5..9].try_into().unwrap()) as usize, bytes.len());
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(decode_client(&[]).is_err());
        assert!(decode_client(&[0x09]).is_err());
        assert!(decode_client(&[FORMATS, 0xff, 0xff, 0xff, 0x7f, 0, 0, 0, 0]).is_err());
        assert!(decode_client(&[VERSION, 1]).is_err());
    }
}
