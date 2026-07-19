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

## Security model

`lazy` is an unauthenticated development proxy. **Any client that can reach its
listener can activate and access any registered HTTP service whose hostname it
requests.** Service names and generated URLs are not credentials. Development
servers often expose source maps, debug endpoints, local data, and write-capable
APIs, so keep the default loopback listener unless network sharing is necessary.

TLS encrypts traffic and lets clients authenticate the server certificate. It
does not authenticate clients: `lazy` does not provide passwords, sessions,
authorization rules, or mutual TLS.

For LAN or tailnet use, bind a specific trusted interface rather than all
interfaces and restrict the port to intended peers with a host firewall or
tailnet ACL:

```sh
cargo run -- proxy \
  --listen 100.64.0.10:8443
```

`lazy` prints a warning when listening outside loopback because the listener
itself provides no access control. Direct public-internet exposure is
unsupported. If public access is required, keep `lazy` on loopback and place an
authenticated, rate-limited gateway in front of it.

The trust boundaries are:

- **Network clients:** untrusted unless admitted by a firewall, private-tailnet
  policy, or authenticated gateway. An admitted client has full HTTP access to
  routed development services and can wake dormant services.
- **Control socket:** `~/.lazy/lazy.sock` is restricted to the current OS user.
  Same-user processes are trusted and can register, start, stop, and inspect
  services.
- **Runner commands:** commands registered with `lazy http` or `lazy worker`
  execute with the user's privileges and are fully trusted. Explicit command
  flags can make an upstream listen beyond loopback.
- **Applications:** upstream traffic is plain HTTP over loopback. Applications
  remain responsible for authentication, authorization, CSRF protection, and
  safe debug configuration; `lazy` does not sanitize requests or responses.

TLS handshakes, HTTP request headers, and upstream connection establishment are
time-bounded, and request headers are limited to 64 KiB. Established proxy
connections have no idle timeout so WebSockets, server-sent events, and other
long-lived development connections continue to work.

## Installation

Prebuilt binaries for Apple Silicon and Intel Macs, plus static ARM64 and
x86-64 Linux binaries, are attached to each
[GitHub release](https://github.com/ineentho/lazy/releases).

Create the destination directory if needed:

```sh
mkdir -p ~/.local/bin
```

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

The destination directory must be on your `PATH`. Use `SHA256SUMS` from the
release to verify a binary before installing it.

### From source

With a Rust toolchain installed, build and install the latest revision directly
from the public repository:

```sh
cargo install --git https://github.com/ineentho/lazy --locked
```

## Xip-style DNS and TLS

An authoritative xip-style DNS zone can route multiple service hostnames
without creating one DNS record per service. This loopback-only example uses
the zone `xip.example.com` and the address `127.0.0.1`:

```sh
cargo run -- proxy \
  --listen 127.0.0.1:443 \
  --xip-domain xip.example.com \
  --xip-ip 127.0.0.1 \
  --cert /path/to/xip.example.com.crt \
  --key /path/to/xip.example.com.key
```

Registering services named `vite` and `api` publishes these URLs:

```text
https://vite-127-0-0-1.xip.example.com
https://api-127-0-0-1.xip.example.com
```

The DNS server must resolve hostnames containing the encoded IPv4 address to
that address. The service name and address deliberately share one DNS label so
a certificate for `*.xip.example.com` covers every generated hostname.

`lazy` terminates TLS with the supplied PEM certificate and key, then proxies
to each service over plain HTTP on `127.0.0.1`. Certificate issuance, renewal,
and private-key storage remain the responsibility of the xip/ACME system;
`lazy` never calls its API.

To share xip services over a trusted LAN or tailnet, use the same specific
reachable address for `--listen` and `--xip-ip`, and allow the listener port
only for intended peers in the firewall or tailnet ACL. Do not publish the
listener directly to the internet.

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

## Releases

[`release-plz`](https://release-plz.dev/) maintains a release PR with the next
version and changelog. Merging that PR creates the matching `vX.Y.Z` tag and
GitHub release, then dispatches the release workflow to build, smoke-test, and
attach all four platform binaries plus `SHA256SUMS`.

The repository setting **Allow GitHub Actions to create and approve pull
requests** must be enabled so the workflow can maintain the release PR. Manual
`vX.Y.Z` tags remain supported as a fallback and run the same binary workflow.

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT), at your option.
