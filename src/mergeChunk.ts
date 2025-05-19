import fs from "node:fs";
import path from "node:path";

export async function mergeChunks(
  tempDir: string,
  finalPath: string,
  totalChunks: number
) {
  const writeStream = fs.createWriteStream(finalPath);

  for (let i = 0; i < totalChunks; i++) {
    const chunkPath = path.resolve(tempDir, `${i}.chunk`);
    await new Promise<void>((resolve, reject) => {
      const readStream = fs.createReadStream(chunkPath);
      readStream.pipe(writeStream, { end: false });
      readStream.on("end", resolve);
      readStream.on("error", reject);
    });
  }

  writeStream.end();
}
