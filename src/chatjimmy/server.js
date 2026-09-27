// Serve a fetch-style handler ((Request) => Promise<Response>) over node:http, in-process.
// bro starts this before spawning the harness and closes it when the harness exits; binding to
// 127.0.0.1 on an ephemeral port keeps it private to this machine and this launch.

import http from 'node:http';

export function serveFetch(handler, { host = '127.0.0.1', port = 0 } = {}) {
  const server = http.createServer(async (req, res) => {
    try {
      const url = `http://${req.headers.host || `${host}:${port}`}${req.url}`;
      const hasBody = req.method !== 'GET' && req.method !== 'HEAD';
      const chunks = [];
      if (hasBody) for await (const c of req) chunks.push(c);
      const request = new Request(url, {
        method: req.method,
        headers: Object.entries(req.headers).flatMap(([k, v]) => (Array.isArray(v) ? v.map((x) => [k, x]) : [[k, v]])),
        body: hasBody && chunks.length ? Buffer.concat(chunks) : undefined
      });
      const response = await handler(request);
      res.writeHead(response.status, Object.fromEntries(response.headers));
      if (!response.body || req.method === 'HEAD') return res.end();
      const reader = response.body.getReader();
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        res.write(value);
      }
      res.end();
    } catch (err) {
      if (!res.headersSent) res.writeHead(500, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ type: 'error', error: { type: 'api_error', message: String(err?.message || err) } }));
    }
  });
  server.keepAliveTimeout = 65_000;
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, host, () => {
      const { port: actual } = server.address();
      resolve({
        url: `http://${host}:${actual}`,
        port: actual,
        close: () => new Promise((r) => server.close(() => r()))
      });
    });
  });
}
