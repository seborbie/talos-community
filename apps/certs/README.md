# Relay TLS certificates

This directory contains the default host inputs for the Docker relay. Compose mounts the certificate
and key as two exact read-only files, not this directory. Certificate and key files are ignored
deliberately; never commit a private key.

The default Compose paths require:

- `local-dev-relay-fullchain.pem` — the server certificate and any required intermediate chain;
- `local-dev-relay-key.pem` — its unencrypted PEM private key.

The certificate's subject alternative names must include the hostname in `RMM_RELAY_URL`, and the
agent/viewer machines must trust its issuer. `bun run dev` and `bun run community:up` fail fast when
the two host files are missing; the relay also rejects an invalid certificate/key pair at startup.

## Same-host local development

One practical option is [`mkcert`](https://github.com/FiloSottile/mkcert), which creates a local CA
and installs it in the host trust store. After installing it, run from the repository root:

```sh
mkcert -install
mkcert \
  -cert-file apps/certs/local-dev-relay-fullchain.pem \
  -key-file apps/certs/local-dev-relay-key.pem \
  localhost 127.0.0.1 ::1
```

Then set `RMM_RELAY_URL=localhost:17443` in `apps/.env`. This is suitable only when the agent and
viewer use the same trust store and can reach that host. For another machine, generate a certificate
for a reachable hostname and install/trust the issuing CA on every client. Containers do not inherit
the host's local trust store; if the optional AI runner must connect through this local CA, copy the
mkcert root certificate (shown by `mkcert -CAROOT`) to `local-dev-relay-ca.pem` here and set
`TALOS_AI_RUNNER_RELAY_CA_PATH=/.certs/local-dev-relay-ca.pem` plus
`TALOS_AI_RUNNER_RELAY_CA_HOST_PATH=../apps/certs/local-dev-relay-ca.pem` in `apps/.env`. Compose
mounts only that CA file into the AI runner; it never mounts the relay private-key directory there.

## Publicly trusted deployment certificate

For a remotely reachable relay, use a certificate issued for your relay DNS name by a CA trusted by
the supported endpoint operating systems. Place its chain and key under the default names above and
set `RMM_RELAY_URL` to that DNS name (including a non-443 port when applicable). Certificate
issuance, DNS, port forwarding, renewal, key permissions, and rotation remain deployment-owner
responsibilities.

## Custom host files or container filenames

The Community launcher passes `apps/.env` to Compose explicitly. Put these overrides in that file
(shell values take precedence):

```dotenv
RMM_RELAY_TLS_CERT_HOST_PATH=/absolute/private/relay-certs/relay-chain.pem
RMM_RELAY_TLS_KEY_HOST_PATH=/absolute/private/relay-certs/relay-key.pem
RMM_RELAY_TLS_CERT_PATH=/.certs/relay-chain.pem
RMM_RELAY_TLS_KEY_PATH=/.certs/relay-key.pem
```

The two container paths must remain below `/.certs/`. Relative host file paths are resolved from
`infra/`, matching Docker Compose's volume-path rules. Exact-file mounts ensure the relay cannot read
an updater-signing PFX or PEM key that happens to live beside its TLS inputs.
