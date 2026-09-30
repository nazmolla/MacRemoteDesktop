#![no_main]

use ironrdp_core::decode;
use ironrdp_rdpeusb::pdu::header::SharedMsgHeader;
use ironrdp_rdpeusb::pdu::{UrbdrcClientControlPdu, UrbdrcClientDevicePdu};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = decode::<SharedMsgHeader>(data);
    let _ = decode::<UrbdrcClientControlPdu>(data);
    let _ = decode::<UrbdrcClientDevicePdu>(data);
});
