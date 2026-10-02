import socket, ssl, sys
H, P = "127.0.0.1", 3390
def cr(proto):
    neg = bytes([1, 0, 8, 0]) + proto.to_bytes(4, "little")
    x = bytes([len(neg) + 6, 0xE0, 0, 0, 0, 0, 0]) + neg
    return bytes([3, 0]) + (len(x) + 4).to_bytes(2, "big") + x
def neg(proto):
    s = socket.create_connection((H, P), 5); s.sendall(cr(proto)); r = s.recv(64)
    if len(r) < 19: return s, f"closed ({len(r)} bytes)"
    kind = {2: "selected", 3: "failure"}.get(r[11], hex(r[11]))
    return s, f"{kind} {int.from_bytes(r[15:19], 'little')}"
for p, name in [(0, "standard RDP"), (1, "TLS only"), (2, "NLA (hybrid)"), (3, "TLS|NLA")]:
    s, d = neg(p); s.close(); print(f"negotiate {name:12}: {d}")
for v in ["TLSv1", "TLSv1_1", "TLSv1_2", "TLSv1_3"]:
    s, d = neg(3)
    c = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT); c.check_hostname = False; c.verify_mode = ssl.CERT_NONE
    try:
        c.minimum_version = c.maximum_version = getattr(ssl.TLSVersion, v)
        t = c.wrap_socket(s); print(f"{v:8}: ACCEPTED {t.version()} {t.cipher()[0]}")
        cert = t.getpeercert(binary_form=True); t.close()
    except Exception as e:
        print(f"{v:8}: refused ({type(e).__name__}: {str(e)[:60]})")
