# TLS test fixtures

Throwaway certificates used **only** by `ferris-server`'s TLS/mTLS tests
(`src/tls.rs` unit tests and `tests/mtls.rs`). They are generated with
`openssl`, valid for ten years, and grant no real authority — the private keys
are committed on purpose so the test suite is hermetic (no `openssl` at test
time) and deterministic.

| file | role |
|---|---|
| `ca.crt` / `ca.key` | trusted test root CA |
| `server.crt` / `server.key` | server cert, `CN=localhost`, SAN `localhost`/`127.0.0.1` |
| `client.crt` / `client.key` | trusted client cert, `O=Solmec`, `CN=SOLMEC-001` |
| `rogue-ca.crt` / `rogue-ca.key` | a CA the server does **not** trust |
| `rogue-client.crt` / `rogue-client.key` | client cert signed by the rogue CA |

Regenerate with:

```
openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.crt -days 3650 \
  -subj "/O=Ferris/CN=Ferris Test Root CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"
openssl req -newkey rsa:2048 -nodes -keyout server.key -out server.csr -subj "/O=Ferris/CN=localhost"
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out server.crt -days 3650 \
  -extfile server.ext
openssl req -newkey rsa:2048 -nodes -keyout client.key -out client.csr -subj "/O=Solmec/CN=SOLMEC-001"
openssl x509 -req -in client.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out client.crt -days 3650 \
  -extfile client.ext
```
