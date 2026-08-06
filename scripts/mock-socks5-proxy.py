#!/usr/bin/env python3
"""极简 SOCKS5 代理（验证 obscura --proxy 生效用）——支持 CONNECT（IPv4/域名），
转发字节流并打印连接日志到 stdout（flush）。默认监听 127.0.0.1:10800。"""
import argparse
import socket
import struct
import threading
import sys

def handle(conn, log):
    try:
        # greeting: VER=5, NMETHODS, METHODS
        data = conn.recv(2)
        if len(data) < 2 or data[0] != 0x05:
            return
        n = data[1]
        conn.recv(n)
        conn.sendall(b"\x05\x00")  # no-auth
        # request: VER=5, CMD, RSV, ATYP, ...
        head = conn.recv(4)
        if len(head) < 4 or head[0] != 0x05 or head[1] != 0x01:
            return
        atyp = head[3]
        if atyp == 0x01:  # IPv4
            addr = socket.inet_ntoa(conn.recv(4))
        elif atyp == 0x03:  # domain
            ln = conn.recv(1)[0]
            addr = conn.recv(ln).decode("latin1")
        elif atyp == 0x04:  # IPv6
            addr = socket.inet_ntop(socket.AF_INET6, conn.recv(16))
        else:
            return
        port = struct.unpack(">H", conn.recv(2))[0]
        log("CONNECT %s:%d" % (addr, port))
        target = socket.create_connection((addr, port), timeout=10)
        conn.sendall(b"\x05\x00\x00\x01\x00\x00\x00\x00\x00\x00")
        log("TUNNEL %s:%d -> %s:%d (established)" % (conn.getpeername()[0], conn.getpeername()[1], addr, port))
        def pipe(src, dst):
            try:
                while True:
                    b = src.recv(65536)
                    if not b:
                        break
                    dst.sendall(b)
            except OSError:
                pass
            finally:
                try:
                    dst.shutdown(socket.SHUT_WR)
                except OSError:
                    pass
        t1 = threading.Thread(target=pipe, args=(conn, target), daemon=True)
        t2 = threading.Thread(target=pipe, args=(target, conn), daemon=True)
        t1.start(); t2.start()
        t1.join(); t2.join()
    except Exception as e:
        log("ERR %s" % e)
    finally:
        try:
            conn.close()
        except OSError:
            pass

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=10800)
    ap.add_argument("--host", default="127.0.0.1")
    args = ap.parse_args()
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((args.host, args.port))
    srv.listen(16)
    def log(msg):
        print(msg, flush=True)
    print("SOCKS5 proxy listening on %s:%d" % (args.host, args.port), flush=True)
    while True:
        conn, _ = srv.accept()
        threading.Thread(target=handle, args=(conn, log), daemon=True).start()

if __name__ == "__main__":
    main()
