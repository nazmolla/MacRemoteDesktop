#![no_main]

use ironrdp_rdpeudp::datagram::Datagram;
use ironrdp_rdpeudp::eudp2::{unwrap_packet, Eudp2Header};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = Datagram::peek_fec_flags(data);
    let _ = Datagram::decode(data);
    let _ = Eudp2Header::decode(data);
    if let Some(inner) = unwrap_packet(data) {
        let _ = Eudp2Header::decode(&inner);
    }
});
