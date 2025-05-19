"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.mergeChunks = void 0;
const node_fs_1 = __importDefault(require("node:fs"));
const node_path_1 = __importDefault(require("node:path"));
async function mergeChunks(tempDir, finalPath, totalChunks) {
    const writeStream = node_fs_1.default.createWriteStream(finalPath);
    for (let i = 0; i < totalChunks; i++) {
        const chunkPath = node_path_1.default.resolve(tempDir, `${i}.chunk`);
        await new Promise((resolve, reject) => {
            const readStream = node_fs_1.default.createReadStream(chunkPath);
            readStream.pipe(writeStream, { end: false });
            readStream.on("end", resolve);
            readStream.on("error", reject);
        });
    }
    writeStream.end();
}
exports.mergeChunks = mergeChunks;
