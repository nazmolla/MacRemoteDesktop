#![no_main]

use ironrdp_rdpeudp::emt;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = emt::peek_pdu_len(data);
    let _ = emt::peek_action(data);
    let _ = emt::TunnelCreateRequest::decode(data);
    let _ = emt::tunnel_data_payload(data);
});
