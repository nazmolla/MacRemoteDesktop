#![no_main]

use ironrdp_core::{decode, ReadCursor};
use ironrdp_rdpdr::pdu::efs::{
    DeviceCreateResponse, DeviceIoResponse, DeviceReadResponse, DeviceWriteResponse,
    FileDirectoryInformation,
};
use ironrdp_rdpdr::pdu::RdpdrPdu;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = decode::<RdpdrPdu>(data);
    let _ = DeviceIoResponse::decode(&mut ReadCursor::new(data));
    let _ = DeviceCreateResponse::decode(&mut ReadCursor::new(data));
    let _ = DeviceReadResponse::decode(&mut ReadCursor::new(data));
    let _ = DeviceWriteResponse::decode(&mut ReadCursor::new(data));
    let _ = FileDirectoryInformation::decode(&mut ReadCursor::new(data));
});
