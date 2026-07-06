# Lazy Real-World Stack Example

This directory sketches a mixed development stack run through `lazy` and `tmuxp`.
Each pane registers a dormant process with the daemon. Visiting the generated
hostname starts only that app.

## Run

From the repository root:

```sh
mise trust
mise install
mise run example-stack
```

The proxy listens on `0.0.0.0:18443` with a Tailscale certificate and serves
apps under path prefixes on your node's Tailscale DNS name:

```text
https://<your-tailscale-node>.ts.net:18443/expo/
https://<your-tailscale-node>.ts.net:18443/vite/
https://<your-tailscale-node>.ts.net:18443/webpack/
https://<your-tailscale-node>.ts.net:18443/fastify/
https://<your-tailscale-node>.ts.net:18443/spring/
https://<your-tailscale-node>.ts.net:18443/axum/
```

`example-prepare` uses `tailscale cert` to write a certificate and key into
`.lazy-example/`, then `example-stack` starts the proxy with that cert. The
actual hostname is written to `.lazy-example/tailscale-domain`.

To prepare dependencies without opening tmux:

```sh
mise run example-prepare
```

`mise run example-stack` depends on `example-prepare`, so the full workspace
installs or refreshes dependencies before tmuxp starts.

## Notes

- Vite and Expo are launched directly through `npx` so `lazy` can inject port
  flags automatically.
- Webpack, Fastify, Spring, and Axum read `PORT` from the environment.
- Spring also maps `PORT` to `SERVER_PORT` in `application.properties`.
