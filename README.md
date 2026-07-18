# lazy

`lazy` is an on-demand development process manager. HTTP services register with
a local proxy but remain dormant until their hostname receives traffic. The
proxy starts the matching command, waits for its loopback port to become ready,
and then forwards the connection.

## Localhost routing

Start the proxy:

```sh
cargo run -- proxy
```

Register a development server in another terminal:

```sh
cargo run -- http vite -- pnpm dlx vite dev
```

The service is available at `http://vite.localhost:8080`. Visiting it activates
the dormant Vite process. Its upstream port is allocated at activation time
and injected into the command. `lazy status`, `lazy start vite`, and
`lazy stop vite` provide manual control.

When a process manager starts runners alongside the proxy, let each runner
wait briefly for the daemon instead of adding shell polling:

```sh
cargo run -- http vite --daemon-timeout 10 -- pnpm dlx vite dev
cargo run -- worker jobs --daemon-timeout 10 -- ./run-jobs
```

Without `--daemon-timeout`, both commands retain their immediate connection
behavior.

## Installation

Prebuilt binaries for Apple Silicon and Intel Macs, plus static ARM64 and
x86-64 Linux binaries, are attached to each
[GitHub release](https://github.com/ineentho/lazy/releases).

### Linux

For x86-64:

```sh
curl -fL https://github.com/ineentho/lazy/releases/latest/download/lazy-x86_64-unknown-linux-musl > ~/.local/bin/lazy && chmod +x ~/.local/bin/lazy
```

For ARM64:

```sh
curl -fL https://github.com/ineentho/lazy/releases/latest/download/lazy-aarch64-unknown-linux-musl > ~/.local/bin/lazy && chmod +x ~/.local/bin/lazy
```

The Linux binaries are statically linked with musl and work on Ubuntu, Debian,
Fedora, Arch, Alpine, and other common distributions.

### macOS

For Apple Silicon:

```sh
curl -fL https://github.com/ineentho/lazy/releases/latest/download/lazy-aarch64-apple-darwin > ~/.local/bin/lazy && chmod +x ~/.local/bin/lazy
```

For Intel:

```sh
curl -fL https://github.com/ineentho/lazy/releases/latest/download/lazy-x86_64-apple-darwin > ~/.local/bin/lazy && chmod +x ~/.local/bin/lazy
```

The destination directory must already exist and be on your `PATH`. Use
`SHA256SUMS` from the release to verify a binary before installing it.

## Xip-style DNS and TLS

An authoritative xip-style DNS zone can expose the same proxy to other
machines without creating one DNS record per service. Given the zone
`xip.example.com` and the address `192.0.2.10`, start the proxy with:

```sh
cargo run -- proxy \
  --listen 0.0.0.0:443 \
  --xip-domain xip.example.com \
  --xip-ip 192.0.2.10 \
  --cert /path/to/xip.example.com.crt \
  --key /path/to/xip.example.com.key
```

Registering services named `vite` and `api` publishes these URLs:

```text
https://vite-192-0-2-10.xip.example.com
https://api-192-0-2-10.xip.example.com
```

The DNS server must resolve hostnames containing the encoded IPv4 address to
that address. The service name and address deliberately share one DNS label so
a certificate for `*.xip.example.com` covers every generated hostname.

`lazy` terminates TLS with the supplied PEM certificate and key, then proxies
to each service over plain HTTP on `127.0.0.1`. Certificate issuance, renewal,
and private-key storage remain the responsibility of the xip/ACME system;
`lazy` never calls its API.

The `--xip-domain` and `--xip-ip` options are required together. They cannot be
combined with `--suffix` or the older `--route-host` path-routing mode. Service
names used in xip mode must be lowercase DNS labels, and the service plus
encoded address must fit in a single 63-character label.

## Real-world example

The [`examples`](examples/README.md) workspace demonstrates Expo, Vite,
Webpack, Fastify, Spring Boot, and Axum behind one TLS proxy.

## Development

```sh
mise trust
mise install
mise run test
```
