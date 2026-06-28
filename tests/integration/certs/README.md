# Integration test certificates

These are **test-only, self-signed** certificates used exclusively inside the
Docker integration test network (`tests/integration/docker-compose.yml`). They
carry **no security risk**: the private keys never protect real traffic and the
CA is trusted only inside the throwaway `devenv` test container.

| File              | Purpose                                                         |
| ----------------- | -------------------------------------------------------------- |
| `ca.key`          | Test CA private key (RSA 2048)                                  |
| `ca.crt`          | Self-signed test CA certificate (CN `ghbrk-test-ca`, 10 years) |
| `mock-github.key` | Server private key for the mock GitHub API (RSA 2048)          |
| `mock-github.crt` | Server certificate signed by the test CA (CN/SAN `mock-github`)|

`Dockerfile.devenv` copies `ca.crt` into the system trust store so `gh`/`curl`
inside the container accept the mock's TLS certificate without `-k`.

## Validity / rotation

Both certificates are issued with a **10-year** validity (`-days 3650`). They
**must be regenerated before they expire**. Run the commands below from the
repository root to rotate them.

## Regeneration commands

```bash
mkdir -p tests/integration/certs

# CA key + self-signed CA cert (10-year validity)
openssl genrsa -out tests/integration/certs/ca.key 2048
openssl req -new -x509 -key tests/integration/certs/ca.key \
  -out tests/integration/certs/ca.crt \
  -days 3650 \
  -subj "/CN=ghbrk-test-ca"

# Server key
openssl genrsa -out tests/integration/certs/mock-github.key 2048

# Server CSR
openssl req -new \
  -key tests/integration/certs/mock-github.key \
  -out tests/integration/certs/mock-github.csr \
  -subj "/CN=mock-github"

# Server cert signed by the CA with SAN DNS:mock-github
cat > /tmp/mock-github-ext.cnf <<EOF
subjectAltName=DNS:mock-github
EOF
openssl x509 -req \
  -in tests/integration/certs/mock-github.csr \
  -CA tests/integration/certs/ca.crt \
  -CAkey tests/integration/certs/ca.key \
  -CAcreateserial \
  -out tests/integration/certs/mock-github.crt \
  -days 3650 \
  -extfile /tmp/mock-github-ext.cnf

# Clean up intermediate artefacts
rm -f tests/integration/certs/mock-github.csr \
      tests/integration/certs/ca.srl \
      /tmp/mock-github-ext.cnf
```

Verify the result:

```bash
openssl verify -CAfile tests/integration/certs/ca.crt \
  tests/integration/certs/mock-github.crt
openssl x509 -in tests/integration/certs/mock-github.crt -noout -ext subjectAltName
```
