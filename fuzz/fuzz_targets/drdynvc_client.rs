#![no_main]

use ironrdp_core::decode;
use ironrdp_dvc::pdu::DrdynvcClientPdu;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = decode::<DrdynvcClientPdu>(data);
});
