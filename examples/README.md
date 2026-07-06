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

The proxy listens on `127.0.0.1:18080` and uses `.localhost` names:

```text
http://expo.localhost:18080
http://vite.localhost:18080
http://webpack.localhost:18080
http://fastify.localhost:18080
http://spring.localhost:18080
http://axum.localhost:18080
```

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
