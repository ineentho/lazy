import Fastify from "fastify";

const app = Fastify({ logger: true });
const port = Number(process.env.PORT || 3000);
const host = process.env.HOST || "127.0.0.1";

app.get("/", async () => ({
  app: "fastify",
  message: "Hello from Fastify",
  lazyUrl: process.env.LAZY_URL,
}));

await app.listen({ host, port });
