# Lazy Real-World Stack Example

This directory sketches a mixed development stack run through `lazy` and `tmuxp`.
Each pane registers a dormant process with the daemon. Visiting the generated
hostname starts only that app.

## Run

From the repository root:

```sh
mise trust
mise install
```

Point the example at an xip-style DNS zone, the IPv4 address that its hostnames
should resolve to, and an existing wildcard certificate and key. The checked-in
example configuration is loopback-only:

```sh
cp .env.example .env
# Edit .env with your DNS zone, loopback IP, certificate, and key paths.
mise run example-stack
```

Replace the documentation values with a real delegated zone and the locally
held certificate for `*.xip.example.com`. By default the proxy listens on
`127.0.0.1:18443` and gives every app its own hostname:

```text
https://expo-127-0-0-1.xip.example.com:18443
https://vite-127-0-0-1.xip.example.com:18443
https://webpack-127-0-0-1.xip.example.com:18443
https://fastify-127-0-0-1.xip.example.com:18443
https://spring-127-0-0-1.xip.example.com:18443
https://axum-127-0-0-1.xip.example.com:18443
```

The service name and encoded IP share one DNS label. This lets a normal
`*.xip.example.com` wildcard certificate cover every generated hostname.
Certificate issuance and renewal stay outside `lazy`; the stack only reads the
paths supplied in the ignored `.env` file. Mise loads that file for the
validation task and tmuxp stack.

`lazy` has no client authentication: every client that can reach the listener
can activate and access every registered app. To share the example on a trusted
LAN or tailnet, set both `LAZY_EXAMPLE_PROXY` and `LAZY_EXAMPLE_XIP_IP` to the
same specific reachable IP and restrict TCP port 18443 to intended peers with a
firewall or tailnet ACL. Direct public-internet exposure is unsupported. Use an
authenticated gateway in front of a loopback-bound proxy instead.

`mise run example-stack` starts tmuxp. Each app installs or fetches missing
dependencies when it is first activated. Invalid proxy settings are reported in
the proxy pane.

## Notes

- Every runner uses `--daemon-timeout 10` so it can register while the proxy is
  starting without tmux-specific polling loops.
- Upstream ports are allocated by the daemon when each app is activated and
  released when it stops.
- The Vite package script is resolved automatically; Expo is launched directly
  through `pnpm exec`.
- The JavaScript apps rely on pnpm's automatic install-on-run behavior.
- Webpack, Fastify, Spring, and Axum read `PORT` from the environment.
- Spring also maps `PORT` to `SERVER_PORT` in `application.properties`.
