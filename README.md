# lazy

`lazy` starts development servers when they receive traffic and stops you from
keeping every project running all the time. It runs a local proxy, assigns each
service a URL, and starts the matching command on the first request.

`lazy` supports macOS and Linux.

## Installation

Prebuilt binaries are available from
[GitHub Releases](https://github.com/ineentho/lazy/releases). Create a local bin
directory first:

```sh
mkdir -p ~/.local/bin
```

### macOS

Apple Silicon:

```sh
curl -fL https://github.com/ineentho/lazy/releases/latest/download/lazy-aarch64-apple-darwin -o ~/.local/bin/lazy
chmod +x ~/.local/bin/lazy
```

Intel:

```sh
curl -fL https://github.com/ineentho/lazy/releases/latest/download/lazy-x86_64-apple-darwin -o ~/.local/bin/lazy
chmod +x ~/.local/bin/lazy
```

### Linux

x86-64:

```sh
curl -fL https://github.com/ineentho/lazy/releases/latest/download/lazy-x86_64-unknown-linux-musl -o ~/.local/bin/lazy
chmod +x ~/.local/bin/lazy
```

ARM64:

```sh
curl -fL https://github.com/ineentho/lazy/releases/latest/download/lazy-aarch64-unknown-linux-musl -o ~/.local/bin/lazy
chmod +x ~/.local/bin/lazy
```

Make sure `~/.local/bin` is on your `PATH`.

To install from source instead, use Rust 1.88 or newer:

```sh
cargo install --git https://github.com/ineentho/lazy --locked
```

## Basic usage

Start the proxy:

```sh
lazy proxy
```

In another terminal, register a development server:

```sh
lazy http vite -- pnpm run dev
```

Open <http://vite.localhost:8080>. The first request starts the server and
`lazy` forwards the request when it is ready. The runner stays open and owns
the development server; stop it with `Ctrl-C`.

Manage registered services from any terminal:

```sh
lazy status
lazy start vite
lazy stop vite
```

By default, commands run in the directory where `lazy http` was started.
`lazy` provides `PORT`, `HOST`, and `LAZY_URL` to the command and recognizes
common development frameworks automatically.

Run `lazy help` or `lazy help <command>` for all commands and options.

## Running on ports 80 and 443

Lazy supports launchd socket activation on macOS and systemd socket activation
on Linux. The service manager binds the privileged port while Lazy, its control
socket, and all registered runners stay under the developer account. See
[DAEMONS.md](DAEMONS.md) for complete setup instructions.

## Xip-style service URLs

For URLs reachable through an xip-style DNS zone, start the proxy with the
zone, encoded IPv4 address, and an existing wildcard certificate:

```sh
lazy proxy \
  --listen 127.0.0.1:443 \
  --xip-domain xip.example.com \
  --xip-ip 127.0.0.1 \
  --cert /path/to/xip.example.com.crt \
  --key /path/to/xip.example.com.key
```

A service named `vite` is then available at:

```text
https://vite-127-0-0-1.xip.example.com
```

Xip hostnames may include a variable prefix before the registered service
name. For example, `acme-vite-127-0-0-1.xip.example.com` also routes to the
`vite` service. The original `Host` header is preserved so the service can use
the prefix for tenant, environment, or branch routing.

The DNS zone must resolve the encoded address, and the certificate must cover
`*.xip.example.com`. Certificate creation and renewal are handled outside
`lazy`. See the [example stack](examples/README.md) for a multi-service setup.

## Development

```sh
mise trust
mise install
mise run test
```

## License

Licensed under either the [Apache License 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT), at your option.
