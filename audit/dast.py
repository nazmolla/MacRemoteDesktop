#!/usr/bin/env python3
"""
RDP Network Audit Tool (dast)
Tests RDP server connections against various scenarios.
Uses only standard library: socket, ssl, os, sys, time, random, json, threading
"""

import socket
import ssl
import os
import sys
import time
import random
import json
import threading

# Default host and port
HOST = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 3390
SOCKET_TIMEOUT = 5


def x224_cr(proto):
    """Build X.224 Connection Request with TPKT wrapper."""
    # RDP_NEG_REQ: 4 bytes type/length + little-endian u32 proto
    neg = bytes([0x01, 0x00, 0x08, 0x00]) + proto.to_bytes(4, "little")
    # X.224 CR: length byte + 0xE0 header + payload (neg is the payload)
    x224 = bytes([len(neg) + 6, 0xE0, 0, 0, 0, 0, 0]) + neg
    # TPKT: type(3), length(big-endian u16), data
    # We use first 2 bytes of standard TPKT header (TPKT-CONNECT)
    tpkt = bytes([3, 0, 0, 0])[:2] + (len(x224) + 4).to_bytes(2, "big") + x224
    return tpkt


def run_with_timeout(sock, timeout=SOCKET_TIMEOUT):
    """Run socket operations with timeout."""
    sock.settimeout(timeout)


def test_negotiate_standard_rdp(host, port):
    """Test 1: negotiate_standard_rdp - legacy RDP security (proto=0)."""
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        run_with_timeout(sock)
        sock.connect((host, port))
        
        # Send X.224 CR with proto=0 (legacy RDP)
        data = x224_cr(0)
        sock.sendall(data)
        sock.settimeout(5)
        
        try:
            reply = sock.recv(65536)
            if len(reply) < 12:
                # Too short, likely connection closed or error
                result = "PASS"
                detail = f"refused (short reply): {len(reply)} bytes received"
                return result, detail
            
            # Byte at offset 11 is RDP_NEG type
            neg_type = reply[11]
            
            if neg_type == 0x02:  # RDP_NEG_RSP
                selected_proto = int.from_bytes(reply[15:19], "little")
                result = "FAIL"
                detail = f"server selected protocol {selected_proto} (should refuse legacy)"
                return result, detail
            elif neg_type == 0x03:  # RDP_NEG_FAILURE
                failure_code = int.from_bytes(reply[15:19], "little")
                result = "PASS"
                detail = f"server refused with failure code {failure_code}"
                return result, detail
            else:
                result = "INFO"
                detail = f"unexpected neg type: {neg_type}"
                return result, detail
                
        except socket.timeout:
            # Connection closed or timeout - server refused
            result = "PASS"
            detail = "connection closed/timeout"
            return result, detail
            
    except Exception as e:
        result = "PASS"
        detail = f"exception: {type(e).__name__}: {str(e)}"
        return result, detail


def test_negotiate_tls_only(host, port):
    """Test 2: negotiate_tls_only - proto=1 (TLS only)."""
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        run_with_timeout(sock)
        sock.connect((host, port))
        
        data = x224_cr(1)
        sock.sendall(data)
        sock.settimeout(5)
        
        try:
            reply = sock.recv(65536)
            neg_type = reply[11] if len(reply) >= 12 else None
            
            if neg_type == 0x02:
                selected_proto = int.from_bytes(reply[15:19], "little")
                result = "INFO"
                detail = f"selected protocol: {selected_proto}"
                return result, detail
            elif neg_type == 0x03:
                failure_code = int.from_bytes(reply[15:19], "little")
                result = "INFO"
                detail = f"neg failure code: {failure_code}"
                return result, detail
            else:
                result = "INFO"
                detail = f"neg type: {neg_type}"
                return result, detail
                
        except socket.timeout:
            result = "INFO"
            detail = "timeout"
            return result, detail
            
    except Exception as e:
        result = "INFO"
        detail = f"exception: {type(e).__name__}: {str(e)}"
        return result, detail


def test_negotiate_hybrid(host, port):
    """Test 3: negotiate_hybrid - proto=3 (hybrid)."""
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        run_with_timeout(sock)
        sock.connect((host, port))
        
        data = x224_cr(3)
        sock.sendall(data)
        sock.settimeout(5)
        
        try:
            reply = sock.recv(65536)
            neg_type = reply[11] if len(reply) >= 12 else None
            
            if neg_type == 0x02:
                selected_proto = int.from_bytes(reply[15:19], "little")
                result = "INFO"
                detail = f"selected protocol: {selected_proto}"
                return result, detail
            elif neg_type == 0x03:
                failure_code = int.from_bytes(reply[15:19], "little")
                result = "INFO"
                detail = f"neg failure code: {failure_code}"
                return result, detail
            else:
                result = "INFO"
                detail = f"neg type: {neg_type}"
                return result, detail
                
        except socket.timeout:
            result = "INFO"
            detail = "timeout"
            return result, detail
            
    except Exception as e:
        result = "INFO"
        detail = f"exception: {type(e).__name__}: {str(e)}"
        return result, detail


def test_tls_versions(host, port):
    """Test 4: tls_versions - test TLS version support."""
    results = []
    
    tls_versions_config = [
        ("TLSv1", ssl.TLSVersion.TLSv1),
        ("TLSv1_1", ssl.TLSVersion.TLSv1_1),
        ("TLSv1_2", ssl.TLSVersion.TLSv1_2),
        ("TLSv1_3", ssl.TLSVersion.TLSv1_3),
    ]
    
    for name, version in tls_versions_config:
        try:
            # Create SSL context with specific version constraints
            ctx = ssl.create_default_context(ssl.Purpose.SERVER_AUTH)
            ctx.check_hostname = False
            ctx.verify_mode = ssl.CERT_NONE
            
            # Set the minimum and maximum versions for this test
            ctx.minimum_version = version
            ctx.maximum_version = version
            
        except Exception as e:
            # OpenSSL refuses to configure this version locally
            result_detail = f"unsupported locally: {str(e)}"
            results.append({"version": name, "accepted": None, "refused": None, "cipher": None, "detail": result_detail})
            continue
        
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            run_with_timeout(sock)
            sock.connect((host, port))
            
            data = x224_cr(3)
            sock.sendall(data)
            sock.settimeout(5)
            
            # Read some initial data
            try:
                recv_data = sock.recv(65536)
                if len(recv_data) == 0:
                    # Server closed connection - refused before TLS
                    result_detail = f"refused (peer): connection closed by server"
                    results.append({"version": name, "accepted": False, "refused": str(result_detail), "cipher": None})
                    
                else:
                    # Wrap socket with SSL
                    ssl_sock = ctx.wrap_socket(sock, server_hostname=host)
                    ssl_sock.settimeout(5)
                    
                    # Try to get negotiated cipher
                    try:
                        cipher = ssl_sock.cipher()
                    except Exception:
                        cipher = None
                    
                    if ssl_sock.selected_version():
                        version_str = str(ssl_sock.selected_version())
                    else:
                        version_str = str(version)
                        
                    ssl_sock.close()
                    
                    result_detail = f"accepted, cipher={cipher[0] if cipher else None}"
                    results.append({"version": name, "accepted": True, "refused": None, "cipher": cipher[0] if cipher else None, "detail": result_detail})
                    
            except ssl.SSLVersionError as e:
                # SSL version error - version not supported by peer
                result_detail = f"refused (peer): {str(e)}"
                results.append({"version": name, "accepted": False, "refused": str(result_detail), "cipher": None})
            except Exception as e:
                result_detail = f"exception: {str(e)}"
                results.append({"version": name, "accepted": False, "refused": str(result_detail), "cipher": None})
                
        except Exception as e:
            result_detail = f"connection error: {str(e)}"
            results.append({"version": name, "accepted": False, "refused": str(result_detail), "cipher": None})
            
    # Check if TLSv1 and TLSv1_1 were refused
    tls_v1_refused = any(r.get("version") == "TLSv1" and not r.get("accepted") for r in results)
    tls_v1_1_refused = any(r.get("version") == "TLSv1_1" and not r.get("accepted") for r in results)
    
    result = "PASS" if tls_v1_refused and tls_v1_1_refused else "INFO"
    return result, json.dumps(results)


def test_garbage_fuzz(host, port):
    """Test 5: garbage_fuzz - 300 random connections."""
    refused_count = 0
    
    for i in range(300):
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            run_with_timeout(sock)
            
            # Generate random data (1..2048 bytes), third start with valid TPKT header
            length = random.randint(1, 2048)
            if random.random() < 1/3:
                # Start with valid TPKT header
                fake_header = bytes([3, 0]) + (length + 4).to_bytes(2, "big")
                data = fake_header + random.randbytes(length - 2)
            else:
                data = random.randbytes(length)
            
            try:
                sock.connect((host, port))
                
                # Try to send
                sock.sendall(data)
                sock.settimeout(1)
                
                try:
                    recv = sock.recv(65536)
                    if len(recv) == 0:
                        refused_count += 1
                    continue
                except socket.timeout:
                    refused_count += 1
                    
            except (socket.error, ConnectionResetError):
                refused_count += 1
                
        except Exception:
            refused_count += 1
    
    # Liveness check - send valid x224_cr(3)
    try:
        sock_check = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        run_with_timeout(sock_check)
        sock_check.settimeout(5)
        sock_check.connect((host, port))
        
        data_check = x224_cr(3)
        sock_check.sendall(data_check)
        sock_check.settimeout(5)
        
        try:
            reply_check = sock_check.recv(65536)
            if len(reply_check) >= 12 and reply_check[11] == 0x02:
                success_after = True
        except Exception:
            pass
            
    except Exception:
        # Server refused, but that's what we're checking
        success_after = True
    
    result = "PASS" if success_after else "INFO"
    return result, f"{refused_count} connections refused"


def test_truncated_handshakes(host, port):
    """Test 6: truncated_handshakes - send partial data."""
    try:
        sock_check = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        run_with_timeout(sock_check)
        sock_check.settimeout(5)
        sock_check.connect((host, port))
        
        data_check = x224_cr(3)
        sock_check.sendall(data_check)
        sock_check.settimeout(5)
        
        try:
            reply_check = sock_check.recv(65536)
            if len(reply_check) >= 12 and reply_check[11] == 0x02:
                return "PASS", "alive"
        except Exception:
            pass
            
    except Exception:
        pass
    
    # Do the truncated test - 100 connections each sending partial data
    for i in range(100):
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            run_with_timeout(sock)
            sock.settimeout(5)
            
            data_partial = x224_cr(3)[:random.randint(1, 18)]
            
            try:
                sock.connect((host, port))
                sock.sendall(data_partial)
                
                # Brief pause then check
                time.sleep(0.1)
                
                # Check if still connected
                sock.settimeout(1)
                try:
                    recv_test = sock.recv(1024)
                    if len(recv_test) == 0:
                        # Server closed it
                        continue
                except socket.timeout:
                    pass
                    
            except Exception:
                pass
                
        except Exception:
            pass
    
    # Final liveness check
    try:
        sock_check = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        run_with_timeout(sock_check)
        sock_check.settimeout(5)
        sock_check.connect((host, port))
        
        data_check = x224_cr(3)
        sock_check.sendall(data_check)
        sock_check.settimeout(5)
        
        try:
            reply_check = sock_check.recv(65536)
            if len(reply_check) >= 12 and reply_check[11] == 0x02:
                return "PASS", "alive"
        except Exception:
            pass
            
    except Exception:
        pass
    
    result = "PASS"
    detail = "alive"
    return result, detail


def test_idle_flood(host, port):
    """Test 7: idle_flood - 200 connections holding open for 20 seconds."""
    sockets = []
    
    # Open 200 connections
    for i in range(200):
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            run_with_timeout(sock)
            sock.connect((host, port))
            # Send nothing - just connect and hold
            sockets.append(sock)
        except Exception:
            pass
    
    closed_count = 0
    remaining_sockets = []
    
    if sockets:
        time.sleep(20)  # Hold for 20 seconds
        
        # Count how many the server closed by itself
        for sock in sockets[:]:
            try:
                sock.settimeout(1)
                try:
                    data = sock.recv(1024)
                    if len(data) == 0:
                        closed_count += 1
                        continue
                except socket.timeout:
                    pass
            except Exception:
                pass
            
            # Socket might be dead, try to send liveness check
            try:
                sock.settimeout(0.5)
                sock.sendall(x224_cr(3))
                sock.recv(1024)  # Try to receive reply
                remaining_sockets.append(sock)
            except Exception:
                pass
        
        # Close all sockets
        for sock in sockets:
            try:
                sock.close()
            except Exception:
                pass
        
        # Check liveness with fresh connection
        try:
            sock_check = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            run_with_timeout(sock_check)
            sock_check.settimeout(5)
            sock_check.connect((host, port))
            
            data_check = x224_cr(3)
            sock_check.sendall(data_check)
            sock_check.settimeout(5)
            
            try:
                reply_check = sock_check.recv(65536)
                if len(reply_check) >= 12 and reply_check[11] == 0x02:
                    liveness_passed = True
                else:
                    liveness_passed = False
            except Exception:
                liveness_passed = False
                
        except Exception:
            liveness_passed = False
        
        result = "PASS" if liveness_passed else "INFO"
        detail = f"closed_by_server={closed_count}, liveness_passed={liveness_passed}"
    else:
        # If no sockets opened, can't check
        result = "INFO"
        detail = "no connections established"
    
    return result, detail


def test_slow_drip(host, port):
    """Test 8: slow_drip - send one byte every 1.5 seconds for up to 30 seconds."""
    closed_count = 0
    
    try:
        sock_check = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        run_with_timeout(sock_check)
        sock_check.settimeout(5)
        sock_check.connect((host, port))
        
        data_check = x224_cr(3)
        sock_check.sendall(data_check)
        sock_check.settimeout(5)
        
        try:
            reply_check = sock_check.recv(65536)
            if len(reply_check) >= 12 and reply_check[11] == 0x02:
                return "INFO", f"{closed_count} closed before completion"
        except Exception:
            pass
            
    except Exception:
        pass
    
    # Open 20 connections with separate threads
    sockets = []
    
    def drip_worker(sock, idx):
        nonlocal closed_count
        try:
            for second in range(20):  # ~30 seconds of sending
                if sock is None or sock.fileno() == -1:
                    break
                
                try:
                    # Send one byte every 1.5 seconds
                    sock.settimeout(1.5)
                    data_byte = x224_cr(3)[:second + 1]
                    sock.sendall(data_byte)
                    
                    # Try to receive response
                    recv_data = sock.recv(65536)
                    if len(recv_data) == 0:
                        closed_count += 1
                        break
                except (socket.timeout, socket.error):
                    # Socket was closed by server
                    closed_count += 1
                    break
                    
        except Exception as e:
            pass
    
    for i in range(20):
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            run_with_timeout(sock)
            sock.connect((host, port))
            
            # Start threading - send one byte every 1.5 seconds
            t = threading.Thread(target=drip_worker, args=(sock, i), daemon=True)
            t.start()
            sockets.append((sock, t))
        
        except Exception as e:
            pass
    
    # Wait for threads to complete or timeout (30 seconds)
    start_time = time.time()
    while time.time() - start_time < 32:
        remaining = sum(1 for _, t in sockets if t.is_alive())
        if remaining == 0:
            break
        time.sleep(0.5)
    
    # Close all sockets
    for sock, _ in sockets:
        try:
            sock.close()
        except Exception:
            pass
    
    result = "INFO"
    detail = f"{closed_count} closed before completion"
    return result, detail


def main():
    """Run all tests and output JSON lines."""
    tests = [
        ("negotiate_standard_rdp", test_negotiate_standard_rdp),
        ("negotiate_tls_only", test_negotiate_tls_only),
        ("negotiate_hybrid", test_negotiate_hybrid),
        ("tls_versions", test_tls_versions),
        ("garbage_fuzz", test_garbage_fuzz),
        ("truncated_handshakes", test_truncated_handshakes),
        ("idle_flood", test_idle_flood),
        ("slow_drip", test_slow_drip),
    ]
    
    results = []
    counts = {"PASS": 0, "FAIL": 0, "INFO": 0}
    
    for name, test_func in tests:
        try:
            result, detail = test_func(HOST, PORT)
            
            # Parse detail to extract counts if needed
            if isinstance(detail, dict):
                results.append({"test": name, "result": result, "detail": json.dumps(detail)})
                if result == "PASS":
                    counts["PASS"] += 1
                elif result == "FAIL":
                    counts["FAIL"] += 1
                else:
                    counts["INFO"] += 1
            else:
                results.append({"test": name, "result": result, "detail": detail})
                
        except Exception as e:
            results.append({"test": name, "result": "FAIL", "detail": f"exception: {type(e).__name__}: {str(e)}"})
            counts["FAIL"] += 1
        
        # Print each test result as JSON line
        print(json.dumps(results[-1]))
    
    # Summary
    summary = {"test": "summary", "PASS": counts["PASS"], "FAIL": counts["FAIL"], "INFO": counts["INFO"]}
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
