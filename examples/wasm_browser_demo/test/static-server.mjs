import { createReadStream, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const port = Number(process.env.PORT ?? 8765);

const contentTypes = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".wasm", "application/wasm"],
  [".css", "text/css; charset=utf-8"],
]);

function resolvePath(urlPath) {
  const decoded = decodeURIComponent(urlPath.split("?")[0]);
  const relative = normalize(decoded.replace(/^\/+/, ""));
  const candidate = resolve(join(root, relative));
  if (candidate !== root && !candidate.startsWith(`${root}${sep}`)) {
    return null;
  }
  return candidate;
}

createServer((request, response) => {
  const filePath = resolvePath(request.url ?? "/");
  if (!filePath) {
    response.writeHead(403);
    response.end("forbidden");
    return;
  }

  let target = filePath;
  try {
    const stat = statSync(target);
    if (stat.isDirectory()) {
      target = join(target, "index.html");
    }
  } catch {
    response.writeHead(404);
    response.end("not found");
    return;
  }

  response.writeHead(200, {
    "content-type": contentTypes.get(extname(target)) ?? "application/octet-stream",
  });
  createReadStream(target).pipe(response);
}).listen(port, "127.0.0.1");
