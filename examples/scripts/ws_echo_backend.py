#!/usr/bin/env python3
"""Minimal WebSocket server without external deps.

Supports:
 - Text & binary echo (opcode 0x1, 0x2)
 - Ping -> Pong (0x9 -> 0xA)
 - Close handshake (echo close frame and terminate)
Limitations:
 - Only handles small frames (<=125 bytes, no fragmentation/extended lengths)
 - No compression, continuation frames, or masking of server frames (per spec not required)
"""
import socket
import os
import threading
import base64
import hashlib

HOST='127.0.0.1'
PORT=int(os.environ.get('PORT','9105'))

GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11'

def handle(conn):
    try:
        data = conn.recv(2048).decode('utf-8', errors='ignore')
        if not data or 'Upgrade: websocket' not in data:
            return
        key_line = next((line for line in data.split('\r\n') if line.lower().startswith('sec-websocket-key:')), None)
        if not key_line:
            return
        key = key_line.split(':', 1)[1].strip()
        # Naive subprotocol select (echo first one)
        proto_line = next((line for line in data.split('\r\n') if line.lower().startswith('sec-websocket-protocol:')), None)
        chosen_proto = None
        if proto_line:
            parts = proto_line.split(':',1)[1].split(',')
            if parts:
                chosen_proto = parts[0].strip()
        accept = base64.b64encode(hashlib.sha1((key+GUID).encode()).digest()).decode()
        resp = (
            'HTTP/1.1 101 Switching Protocols\r\n'
            'Upgrade: websocket\r\n'
            'Connection: Upgrade\r\n'
            f'Sec-WebSocket-Accept: {accept}\r\n'
            f'{(f"Sec-WebSocket-Protocol: {chosen_proto}\r\n" if chosen_proto else "")}'
            '\r\n'
        )
        conn.sendall(resp.encode())
        # Frame loop
        while True:
            hdr = conn.recv(2)
            if len(hdr) < 2:
                break
            fin_opcode = hdr[0]
            opcode = fin_opcode & 0x0F
            masked_len = hdr[1]
            masked = (masked_len & 0x80) != 0
            payload_len = masked_len & 0x7F
            if payload_len > 125:  # keep tiny
                break
            mask = b''
            if masked:
                mask = conn.recv(4)
            payload = bytearray(conn.recv(payload_len)) if payload_len else bytearray()
            if masked and mask:
                for i in range(payload_len):
                    payload[i] ^= mask[i % 4]
            payload_bytes = bytes(payload)
            # Handle opcodes
            if opcode == 0x8:  # close
                # echo close
                out = bytearray([0x88])
                out.append(len(payload_bytes))
                out.extend(payload_bytes)
                conn.sendall(out)
                break
            elif opcode == 0x9:  # ping -> pong
                out = bytearray([0x8A])
                out.append(len(payload_bytes))
                out.extend(payload_bytes)
                conn.sendall(out)
                continue
            elif opcode in (0x1, 0x2):  # text/binary echo
                out = bytearray([0x80 | opcode])
                out.append(len(payload_bytes))
                out.extend(payload_bytes)
                conn.sendall(out)
            else:
                # ignore other opcodes
                continue
    finally:
        conn.close()

def main():
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((HOST, PORT))
    s.listen(5)
    try:
        while True:
            c, _ = s.accept()
            threading.Thread(target=handle, args=(c,), daemon=True).start()
    except KeyboardInterrupt:
        pass
    finally:
        s.close()

if __name__ == '__main__':
    main()
