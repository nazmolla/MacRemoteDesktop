//! Garbage-input tests for the decoders that read bytes a client (or anything
//! on the network) controls.
//!
//! Each decoder is fed every truncation and many random mutations of a valid
//! seed, plus purely random buffers, from a fixed-seed generator so a failure
//! reproduces exactly. The only requirement is that decoding returns (Ok or an
//! error) instead of panicking. This is a cheap, always-on complement to the
//! coverage-guided targets in `fuzz/`, which run the same decoders for as long
//! as you let them (see fuzz/README.md).

use std::panic::{catch_unwind, AssertUnwindSafe};

use ironrdp_core::{decode, ReadCursor};

/// xorshift64*: small, deterministic, good enough to spread bytes around.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Every truncation of each seed, 300 random mutations of each, and 2000
/// random buffers of up to 1500 bytes.
fn corpus(seeds: &[&[u8]]) -> Vec<Vec<u8>> {
    let mut rng = Rng(0x6d61_6372_6470_0001);
    let mut out = Vec::new();
    for seed in seeds {
        for n in 0..=seed.len() {
            out.push(seed[..n].to_vec());
        }
        for _ in 0..300 {
            let mut m = seed.to_vec();
            for _ in 0..=rng.below(4) {
                if m.is_empty() {
                    break;
                }
                let i = rng.below(m.len());
                m[i] = match rng.below(4) {
                    0 => 0x00,
                    1 => 0xff,
                    2 => m[i] ^ (1 << rng.below(8)),
                    _ => rng.next() as u8,
                };
            }
            out.push(m);
        }
    }
    for _ in 0..2000 {
        let len = rng.below(1500);
        out.push((0..len).map(|_| rng.next() as u8).collect());
    }
    out
}

/// Run `f` on every input, failing with the offending bytes on a panic.
fn never_panics(name: &str, seeds: &[&[u8]], f: impl Fn(&[u8])) {
    // Keep the expected panics' default output out of the test log.
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut failure = None;
    for input in corpus(seeds) {
        if catch_unwind(AssertUnwindSafe(|| f(&input))).is_err() {
            failure = Some(input);
            break;
        }
    }
    std::panic::set_hook(quiet);
    if let Some(input) = failure {
        let hex: String = input.iter().map(|b| format!("{b:02x}")).collect();
        panic!("{name} panicked on {} bytes: {hex}", input.len());
    }
}

/// A real client SYN prefix (see src/multitransport.rs), padded as on the wire.
fn client_syn() -> Vec<u8> {
    #[rustfmt::skip]
    let prefix: [u8; 84] = [
        0xff, 0xff, 0xff, 0xff, 0x00, 0x40, 0x18, 0x01, 0x64, 0x7a, 0x02, 0xbc, 0x04, 0xd0,
        0x04, 0xd0, 0x43, 0x33, 0x3c, 0x63, 0xee, 0x77, 0x40, 0x6e, 0x97, 0xdf, 0x80, 0x0c,
        0xa1, 0xfd, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0xcb, 0x86, 0x4c, 0x5b,
        0x54, 0x3a, 0xdc, 0x7a, 0x7a, 0x36, 0x7b, 0xb8, 0x11, 0x20, 0x71, 0x7c, 0x28, 0x6d,
        0x09, 0x3d, 0x3f, 0x3a, 0xd8, 0x80, 0x2c, 0x59, 0x4f, 0x4f, 0x21, 0x99, 0x86, 0x94,
    ];
    let mut syn = prefix.to_vec();
    syn.resize(1232, 0);
    syn
}

#[test]
fn rdpeudp_datagrams() {
    use ironrdp_rdpeudp::datagram::Datagram;
    let syn = client_syn();
    never_panics("Datagram::decode", &[&syn], |b| {
        let _ = Datagram::peek_fec_flags(b);
        let _ = Datagram::decode(b);
    });
}

#[test]
fn rdpeudp2_packets() {
    use ironrdp_rdpeudp::eudp2::{unwrap_packet, Eudp2Header};
    let syn = client_syn();
    never_panics("eudp2", &[&syn], |b| {
        let _ = Eudp2Header::decode(b);
        if let Some(inner) = unwrap_packet(b) {
            let _ = Eudp2Header::decode(&inner);
        }
    });
}

#[test]
fn rdpemt_tunnel_pdus() {
    use ironrdp_rdpeudp::emt;
    // CREATEREQUEST: action 0x1, flags, payload length 24, header length 4.
    let mut create = vec![0x01, 0x00, 0x18, 0x00];
    create.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0]);
    create.extend_from_slice(&[0xab; 16]);
    // DATA: action 0x2 with a 6-byte payload.
    let data = [0x02, 0x00, 0x06, 0x00, 1, 2, 3, 4, 5, 6];
    never_panics("MS-RDPEMT", &[&create, &data], |b| {
        let _ = emt::peek_pdu_len(b);
        let _ = emt::peek_action(b);
        let _ = emt::TunnelCreateRequest::decode(b);
        let _ = emt::tunnel_data_payload(b);
    });
}

#[test]
fn drdynvc_client_pdus() {
    use ironrdp_dvc::pdu::DrdynvcClientPdu;
    // Soft-Sync response with one tunnel (see src/multitransport.rs tests),
    // a capabilities response and a create response.
    let soft_sync = [0x90, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00];
    let caps = [0x50, 0x00, 0x03, 0x00];
    let create = [0x10, 0x03, 0x00, 0x00, 0x00, 0x00];
    never_panics("DrdynvcClientPdu", &[&soft_sync, &caps, &create], |b| {
        let _ = decode::<DrdynvcClientPdu>(b);
    });
}

#[test]
fn rdpdr_pdus() {
    use ironrdp_rdpdr::pdu::efs::{
        DeviceCreateResponse, DeviceIoResponse, DeviceReadResponse, DeviceWriteResponse,
        FileDirectoryInformation,
    };
    use ironrdp_rdpdr::pdu::RdpdrPdu;
    // Client announce reply ("rDCC", version 1.12, client id 7) and a device
    // I/O completion header ("rDCI") with a small body.
    let announce = [
        0x72, 0x44, 0x43, 0x43, 0x01, 0x00, 0x0c, 0x00, 0x07, 0x00, 0x00, 0x00,
    ];
    let mut completion = vec![0x72, 0x44, 0x43, 0x49, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0];
    completion.extend_from_slice(&[4, 0, 0, 0, 0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 0]);
    never_panics("RDPDR", &[&announce, &completion], |b| {
        let _ = decode::<RdpdrPdu>(b);
        let _ = DeviceIoResponse::decode(&mut ReadCursor::new(b));
        let _ = DeviceCreateResponse::decode(&mut ReadCursor::new(b));
        let _ = DeviceReadResponse::decode(&mut ReadCursor::new(b));
        let _ = DeviceWriteResponse::decode(&mut ReadCursor::new(b));
        let _ = FileDirectoryInformation::decode(&mut ReadCursor::new(b));
    });
}

#[test]
fn smart_card_returns() {
    use ironrdp_pdu::utils::CharacterSet;
    use ironrdp_rdpdr::pdu::esc::{
        ConnectReturn, EstablishContextReturn, GetStatusChangeReturn, ListReadersReturn,
        LongReturn, StatusReturn, TransmitReturn,
    };
    // An NDR common + private type header followed by a small body.
    let mut ndr = vec![0x01, 0x10, 0x08, 0x00, 0xcc, 0xcc, 0xcc, 0xcc];
    ndr.extend_from_slice(&[0x20, 0, 0, 0, 0, 0, 0, 0]);
    ndr.extend_from_slice(&[0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 2, 0, 4, 0, 0, 0, 1, 2, 3, 4]);
    never_panics("MS-RDPESC returns", &[&ndr], |b| {
        let _ = EstablishContextReturn::decode(&mut ReadCursor::new(b));
        let _ = LongReturn::decode(&mut ReadCursor::new(b));
        let _ = ListReadersReturn::decode(&mut ReadCursor::new(b));
        let _ = GetStatusChangeReturn::decode(&mut ReadCursor::new(b));
        let _ = ConnectReturn::decode(&mut ReadCursor::new(b));
        let _ = StatusReturn::decode(&mut ReadCursor::new(b), CharacterSet::Unicode);
        let _ = TransmitReturn::decode(&mut ReadCursor::new(b));
    });
}

#[test]
fn urbdrc_client_pdus() {
    use ironrdp_rdpeusb::pdu::header::SharedMsgHeader;
    use ironrdp_rdpeusb::pdu::{UrbdrcClientControlPdu, UrbdrcClientDevicePdu};
    // Header: interface id + mask, message id, function id; then a result.
    let caps = [
        0x00, 0x00, 0x00, 0x00, 1, 0, 0, 0, 0x00, 0x01, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0,
    ];
    never_panics("URBDRC", &[&caps], |b| {
        let _ = decode::<SharedMsgHeader>(b);
        let _ = decode::<UrbdrcClientControlPdu>(b);
        let _ = decode::<UrbdrcClientDevicePdu>(b);
    });
}

#[test]
fn pre_auth_x224_and_mcs() {
    use ironrdp_pdu::gcc::ConferenceCreateRequest;
    use ironrdp_pdu::mcs::ConnectInitial;
    use ironrdp_pdu::nego::ConnectionRequest;
    use ironrdp_pdu::x224::X224;
    // TPKT + X.224 connection request with an RDP negotiation request
    // (TLS | CredSSP), as mstsc sends first.
    let conn_req = [
        0x03, 0x00, 0x00, 0x13, 0x0e, 0xe0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00,
        0x03, 0x00, 0x00, 0x00,
    ];
    never_panics("X.224 / MCS", &[&conn_req], |b| {
        let _ = decode::<X224<ConnectionRequest>>(b);
        let _ = decode::<ConnectInitial>(b);
        let _ = decode::<ConferenceCreateRequest>(b);
    });
}

#[test]
fn clipboard_rich_text() {
    let html = crate::clipboard_rich::encode_cf_html("<b>hi</b> caf\u{e9}");
    let rtf = crate::clipboard_rich::encode_rtf(b"{\\rtf1 hi}");
    never_panics("CF_HTML / RTF", &[&html, &rtf], |b| {
        let _ = crate::clipboard_rich::decode_cf_html(b);
        let _ = crate::clipboard_rich::decode_rtf(b);
    });
}

/// The harness itself: a decoder that panics on some input must be reported.
#[test]
#[should_panic(expected = "panicked on")]
fn the_harness_reports_a_panicking_decoder() {
    never_panics("self-test", &[&[1, 2, 3]], |b| assert!(b.len() != 2));
}

/// Seeds that decode cleanly let the mutations reach the deeper parsing
/// paths, not just the first length check.
#[test]
fn seeds_are_valid_where_it_matters() {
    use ironrdp_pdu::nego::ConnectionRequest;
    use ironrdp_pdu::x224::X224;
    let syn = client_syn();
    assert!(ironrdp_rdpeudp::datagram::Datagram::decode(&syn).is_ok());
    let conn_req = [
        0x03, 0x00, 0x00, 0x13, 0x0e, 0xe0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00,
        0x03, 0x00, 0x00, 0x00,
    ];
    assert!(decode::<X224<ConnectionRequest>>(&conn_req).is_ok());
    let soft_sync = [0x90, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00];
    let _ = decode::<ironrdp_dvc::pdu::DrdynvcClientPdu>(&soft_sync);
    let html = crate::clipboard_rich::encode_cf_html("<b>hi</b>");
    assert!(crate::clipboard_rich::decode_cf_html(&html).is_some());
}
