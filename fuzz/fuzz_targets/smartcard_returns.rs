#![no_main]

use ironrdp_core::ReadCursor;
use ironrdp_pdu::utils::CharacterSet;
use ironrdp_rdpdr::pdu::esc::{
    ConnectReturn, EstablishContextReturn, GetStatusChangeReturn, ListReadersReturn, LongReturn,
    StatusReturn, TransmitReturn,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = EstablishContextReturn::decode(&mut ReadCursor::new(data));
    let _ = LongReturn::decode(&mut ReadCursor::new(data));
    let _ = ListReadersReturn::decode(&mut ReadCursor::new(data));
    let _ = GetStatusChangeReturn::decode(&mut ReadCursor::new(data));
    let _ = ConnectReturn::decode(&mut ReadCursor::new(data));
    let _ = StatusReturn::decode(&mut ReadCursor::new(data), CharacterSet::Unicode);
    let _ = TransmitReturn::decode(&mut ReadCursor::new(data));
});
