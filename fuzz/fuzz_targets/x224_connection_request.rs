#![no_main]

use ironrdp_core::decode;
use ironrdp_pdu::gcc::ConferenceCreateRequest;
use ironrdp_pdu::mcs::ConnectInitial;
use ironrdp_pdu::nego::ConnectionRequest;
use ironrdp_pdu::x224::X224;
use libfuzzer_sys::fuzz_target;

// The first bytes any peer sends, before TLS or authentication.
fuzz_target!(|data: &[u8]| {
    let _ = decode::<X224<ConnectionRequest>>(data);
    let _ = decode::<ConnectInitial>(data);
    let _ = decode::<ConferenceCreateRequest>(data);
});
