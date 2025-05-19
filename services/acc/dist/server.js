"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.createServer = void 0;
const express_1 = __importDefault(require("express"));
const cors_1 = __importDefault(require("cors"));
function createServer(port, option = {}) {
    const server = (0, express_1.default)();
    let { createHandle, routes, middles } = option;
    [
        (0, cors_1.default)({
            origin: "*",
        }),
        express_1.default.json(),
        middles || [],
    ]
        .flat()
        .reduce((a, b) => a.use(b), server);
    if (routes) {
        const parseRoute = (routes, parentPath) => {
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
exports.createServer = createServer;
