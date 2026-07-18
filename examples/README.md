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
should resolve to, and an existing wildcard certificate and key:

```sh
cp .env.example .env
# Edit .env with your DNS zone, reachable IP, certificate, and key paths.
mise run example-stack
```

Replace the documentation values with a real delegated zone, a reachable LAN,
tailnet, or public IPv4 address, and the locally held certificate for
`*.xip.example.com`. The proxy listens on `0.0.0.0:18443` and gives every
app its own hostname:

```text
https://expo-192-0-2-10.xip.example.com:18443
https://vite-192-0-2-10.xip.example.com:18443
https://webpack-192-0-2-10.xip.example.com:18443
https://fastify-192-0-2-10.xip.example.com:18443
https://spring-192-0-2-10.xip.example.com:18443
https://axum-192-0-2-10.xip.example.com:18443
```

The service name and encoded IP share one DNS label. This lets a normal
`*.xip.example.com` wildcard certificate cover every generated hostname.
Certificate issuance and renewal stay outside `lazy`; the stack only reads the
paths supplied in the ignored `.env` file. Mise loads that file for the
validation task and tmuxp stack.

`mise run example-stack` starts tmuxp. Each app installs or fetches missing
dependencies when it is first activated. Invalid proxy settings are reported in
the proxy pane.

## Notes

- Every runner uses `--daemon-timeout 10` so it can register while the proxy is
  starting without tmux-specific polling loops.
- Upstream ports are allocated by the daemon when each app is activated and
  released when it stops.
- Vite and Expo are launched directly through `pnpm exec` so `lazy` can inject
  port flags automatically.
- The JavaScript apps rely on pnpm's automatic install-on-run behavior.
- Webpack, Fastify, Spring, and Axum read `PORT` from the environment.
- Spring also maps `PORT` to `SERVER_PORT` in `application.properties`.
