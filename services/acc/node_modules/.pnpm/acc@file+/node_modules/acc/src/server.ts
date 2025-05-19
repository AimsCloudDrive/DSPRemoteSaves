import express from "express";
import cors from "cors";
import type { RequestHandler } from "express";

export type ServerRoute<T extends string | RegExp = any> = {
  path: T;
  method?: "get" | "post";
  children?: ServerRoute[];
  handlers?: RequestHandler[];
};

export function createServer(
  port: number,
  option: {
    routes?: ServerRoute[];
    createHandle?: () => void;
    middles?: RequestHandler[] | RequestHandler;
  } = {}
): Express.Application {
  const server = express();
  let { createHandle, routes, middles } = option;
  [
    cors({
      origin: "*",
    }),
    express.json(),
    middles || [],
  ]
    .flat()
    .reduce<typeof server>((a, b) => a.use(b), server);
  if (routes) {
    const parseRoute = (routes: ServerRoute[], parentPath: string) => {
      for (const route of routes) {
        let { path, children, method, handlers = [] } = route;
        path = parentPath + path;
        if (method) {
          const r = server[method].bind(server);
          r(path, ...handlers);
        }
        if (children) {
          parseRoute(children, path);
        }
      }
    };
    parseRoute(routes, "");
  }
  server.listen(port, () => {
    typeof createHandle === "function" && createHandle();
  });
  return server;
}
