const lazyHost = process.env.LAZY_URL ? new URL(process.env.LAZY_URL).hostname : undefined;

export default {
  server: {
    allowedHosts: lazyHost ? [lazyHost] : [],
  },
};
