# Fuzz targets

Coverage-guided fuzzing for the decoders that read bytes a client, or anyone on
the network, controls. The crate is separate from the macrdp build and has its
own workspace and lock file. Its `[patch]` sections mirror the root
`Cargo.toml`, so the targets run the same vendored and pinned code macrdp ships.

| Target | What it feeds | Reachable |
|---|---|---|
| `x224_connection_request` | X.224 connection request, MCS Connect Initial, GCC conference create | Before TLS and authentication |
| `rdpeudp_datagram` | RDPEUDP datagrams and RDPEUDP2 packets | Before authentication, from an IP that holds a multitransport offer |
| `rdpemt_tunnel` | MS-RDPEMT tunnel PDUs (after the UDP TLS/DTLS handshake) | Authenticated session's UDP flow |
| `drdynvc_client` | DRDYNVC client PDUs, including Soft-Sync | Authenticated session |
| `rdpdr` | RDPDR PDUs and the drive I/O responses the server decodes | Authenticated session |
| `smartcard_returns` | MS-RDPESC return structures (NDR) | Authenticated session |
| `urbdrc_client` | MS-RDPEUSB client PDUs | Authenticated session |

## Running

Needs a nightly toolchain and cargo-fuzz:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
cd fuzz
cargo +nightly fuzz run x224_connection_request -- -max_total_time=600
```

List the targets with `cargo +nightly fuzz list`. Crashes are written to
`fuzz/artifacts/<target>/`; reproduce one with
`cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<file>`.

The same decoders also get a fixed, fast garbage-input pass on every
`cargo test` (`src/garbage_input_test.rs`), so a regression that makes one of
them panic on malformed input fails the normal test run.
