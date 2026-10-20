#!/usr/bin/env bash
# PC Link's TLS (the system's OpenSSL) against a stand-in server here: a
# trusted certificate (SSL_CERT_FILE) connects; an untrusted one, or one for
# another name, is refused. Uses examples/connect.rs, built as the release is,
# since the release program connects only to lazyoverlay.com.
#
#   cargo build --release --example connect
#   scripts/tls-check.sh target/release/examples/connect

set -Eeuo pipefail
bin=$(realpath "$1")
dir=$(mktemp -d)
trap 'kill "$server" 2>/dev/null || true; rm -rf "$dir"' EXIT
cd "$dir"
openssl req -x509 -newkey rsa:2048 -nodes -keyout key.pem -out cert.pem -days 1 -subj /CN=localhost \
  -addext subjectAltName=DNS:localhost 2>/dev/null
python3 - <<'PY' &
import base64, hashlib, socket, ssl, threading
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain('cert.pem', 'key.pem')
s = socket.create_server(('127.0.0.1', 18443))
def serve(c):
    try:
        c = ctx.wrap_socket(c, server_side=True)
        req = b''
        while b'\r\n\r\n' not in req:
            req += c.recv(4096)
        k = [l.split(b':', 1)[1].strip() for l in req.split(b'\r\n') if l.lower().startswith(b'sec-websocket-key')][0]
        acc = base64.b64encode(hashlib.sha1(k + b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
        c.sendall(b'HTTP/1.1 101 OK\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ' + acc + b'\r\n\r\n')
        hello = b'{"t":"hello","login":"tls-check"}'
        c.sendall(bytes([0x81, len(hello)]) + hello)
        c.recv(4096)
        c.sendall(bytes([0x88, 2]) + (4401).to_bytes(2, 'big'))
        c.recv(100)
    except Exception:
        pass
while True:
    c, _ = s.accept()
    threading.Thread(target=serve, args=(c,), daemon=True).start()
PY
server=$!
sleep 1

# run HOST [SSL_CERT_FILE]: PC Link's first lines, for a few seconds.
run() {
  printf '[link]\nkey = abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG\n' >pclink.ini
  SSL_CERT_FILE=${2:-/nonexistent} timeout 4 "$bin" "https://$1:18443" pclink.ini </dev/null >out.txt 2>&1 || true
  cat out.txt
}
expect() {
  if grep -q "$2" <<<"$3"; then echo "ok: $1"; else echo "FAILED: $1"; echo "$3"; exit 1; fi
}
expect "a trusted certificate connects" "Connected to tls-check's overlay" "$(run localhost "$dir/cert.pem")"
expect "an untrusted certificate is refused" "certificate was not trusted" "$(run localhost)"
expect "a certificate for another name is refused" "certificate was not trusted" "$(run 127.0.0.1 "$dir/cert.pem")"
