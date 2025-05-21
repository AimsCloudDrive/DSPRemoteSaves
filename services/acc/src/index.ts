import bcrypt from "bcrypt";
import fs from "fs";
import type { Collection } from "mongodb";
import { MongoClient } from "mongodb";
import path from "path";
import { v4 as uuidv4 } from "uuid";
import { mergeChunks } from "./mergeChunk";
import { createServer, type ServerRoute } from "./server";
import { UserAPI, DownloadAPI, UploadAPI, Response } from "./type";

function assert(condition: unknown, message: string = ""): asserts condition {
  if (!condition) throw Error(message);
}

class CodeResult<
  C extends number = number,
  Payload extends { [K in string]: any } = {}
> {
  declare code: C;
  declare message: string | undefined;
  declare payload: Payload | undefined;
  constructor(
    code: C,
    message?: string | undefined | null | Payload,
    payload?: Payload | undefined | null
  ) {
    this.code = code;
    if (payload) {
      if (typeof message === "object" && message !== null) {
        throw Error();
      }
      if (message != undefined) {
        this.message = message;
      }
      this.payload = payload;
    } else if (message != undefined) {
      if (typeof message === "object") {
        this.payload = message;
      } else {
        this.message = message;
      }
    } else {
    }
  }
}

const saltRounds: number = 10;
async function createUser(
  users: Collection<IUser>,
  user: UserAPI.UserRequestBody
) {
  const checked = await checkUser(users, user);
  if (checked.code !== 1) {
    return new CodeResult(1, "用户已存在");
  }
  let { userName, password } = user;
  password = await bcrypt.hash(password, saltRounds);
  return await users
    .insertOne({ userName, password, cache: {}, login: false })
    .then(
      (v) =>
        !!v.insertedId
          ? new CodeResult(0, "创建成功")
          : new CodeResult(1, "创建失败"),
      (e) => new CodeResult(1, "创建失败")
    );
}
/**
 * {code: 0 | 1 | 2}
 * 0: 用户名密码正确，1：用户不存在，2：用户名或密码错误
 * @param users
 * @param user
 * @param options
 * @returns
 */
async function checkUser(
  users: Collection<IUser>,
  {
    userName,
    password = "",
  }: Pick<UserAPI.UserRequestBody, "userName"> &
    Partial<Pick<UserAPI.UserRequestBody, "password">>,
  options?: boolean | { comparePassword?: boolean }
) {
  const finded = await users.findOne({
    userName: { $eq: userName },
  });
  if (!finded) {
    return new CodeResult(1, "用户不存在");
  } else if (
    (typeof options === "object" ? options.comparePassword : options) !==
      false &&
    !bcrypt.compareSync(password, finded.password)
  ) {
    return new CodeResult(2, "用户名或密码错误");
  } else {
    return new CodeResult(0, { user: finded });
  }
}

new MongoClient("mongodb://never.aims.nevermonarch.cn:57857/", {
  auth: {
    username: "root",
    password: "123456",
  },
})
  .connect()
  .then(
    async (client) => {
      const db = client.db("DSPCloudSaves");
      const users = db.collection<IUser>("Users");
      createServer(9999, {
        routes: [
          {
            path: "/dsp.saves/api",
            children: [
              {
                path: "/users",
                children: [
                  login_user(users),
                  logout_user(users),
                  create_user(users),
                  update_user(users),
                  delete_user(users),
                ],
              },
              {
                path: "/upload",
                children: [
                  init_upload(users),
                  upload_chunk(users),
                  complete_upload(users),
                ],
              },
              {
                path: "/download",
                children: [file_list(users), download(users)],
              },
            ],
          },
        ],
        createHandle: () => {
          console.log("server startting on 9999");
        },
        middles: (request, _, next) => {
          console.log(request.url);
          next();
        },
      });
    },
    () => {
      console.log("content Error");
    }
  );

interface IUser extends UserAPI.UserRequestBody {
  cache: {
    [K in string]: Omit<
      FileInitUploadInfo,
      "userName" | "uploadedChunks" | "cancelClearId"
    > & {
      uploadedChunks: number[];
    };
  };
  login: boolean;
}

const baseSavePath = "./saves";

interface FileInitUploadInfo {
  userName: string;
  fileName: string;
  fileSize: number;
  chunkSize: number;
  totalChunks: number;
  uploadedChunks: Set<number>;
  createdAt: number;
  /** 单位： 秒s */
  ttl: number;
  cancelClearId: NodeJS.Timeout | undefined;
}
const uploads: {
  [K in string]: FileInitUploadInfo;
} = Object.create(null);

type Handler = (users: Collection<IUser>) => ServerRoute;

const init_upload: Handler = (users) => {
  return {
    path: "/init",
    method: "post",
    handlers: [
      async (request, response) => {
        const {
          fileName,
          fileSize,
          chunkSize,
          userName,
          password,
          ttl = 5 * 60,
        } = request.body as UploadAPI.InitRequestBody;
        const userChecked = await checkUser(users, { userName, password });
        if (userChecked.code !== 0) {
          userChecked.code = 1;
          response.status(400).json(userChecked);
          return;
        }
        if (!fileName || !fileSize || !chunkSize) {
          response.status(400).json(new CodeResult(1, "参数错误"));
          return;
        }
        const fileId = uuidv4();
        const totalChunks = Math.ceil(fileSize / chunkSize);
        const tempDir = path.resolve(baseSavePath, "temp", fileId);

        fs.mkdirSync(tempDir, { recursive: true });

        uploads[fileId] = {
          userName,
          fileName: path.basename(fileName),
          fileSize,
          chunkSize,
          totalChunks,
          uploadedChunks: new Set(),
          createdAt: Date.now(),
          ttl,
          cancelClearId: setTimeout(() => {
            const tempDir = path.resolve(baseSavePath, "temp", fileId);
            try {
              fs.existsSync(tempDir) && fs.rmSync(tempDir, { recursive: true });
            } finally {
              delete uploads[fileId];
            }
          }, ttl * 1000),
        };
        const { cancelClearId, ..._upload } = uploads[fileId];

        await users.updateOne(
          {
            userName: { $eq: userName },
            password: { $eq: password },
          },
          {
            $set: {
              userName,
              password,
              cache: {
                ...(userChecked?.payload?.user?.cache || {}),
                [fileId]: {
                  ..._upload,
                  uploadedChunks: [...uploads[fileId].uploadedChunks],
                },
              },
            },
          }
        );

        response
          .status(200)
          .json(
            new CodeResult(0, { data: { fileId, chunkSize, totalChunks } })
          );
      },
    ],
  };
};
const upload_chunk: Handler = (users) => {
  return {
    path: "/chunk",
    method: "post",
    handlers: [
      (request, response) => {
        const { fileId, chunkIndex: _chunkIndex } =
          request.query as unknown as UploadAPI.ChunkRequestQuery;
        const chunkIndex =
          typeof _chunkIndex === "string" ? parseInt(_chunkIndex) : Number.NaN;

        if (!fileId || Number.isNaN(chunkIndex)) {
          response.status(400).json(new CodeResult(1, "参数错误"));
          return;
        }

        const upload = uploads[fileId];
        if (!upload || chunkIndex < 0 || chunkIndex >= upload.totalChunks) {
          response.status(400).json(new CodeResult(1, "无效请求"));
          return;
        }

        const tempDir = path.resolve(baseSavePath, "temp", fileId);
        const chunkPath = path.resolve(tempDir, `${chunkIndex}.chunk`);
        const writeStream = fs.createWriteStream(chunkPath);

        request
          .pipe(writeStream)
          .on("finish", async () => {
            writeStream.close();
            upload.uploadedChunks.add(chunkIndex);
            const checked = await checkUser(users, upload, false);
            if (checked.code !== 0) {
              throw new Error();
            }
            const { cancelClearId, ..._upload } = upload;
            await users.updateOne(
              {
                userName: { $eq: upload.userName },
              },
              {
                $set: {
                  cache: {
                    ...(checked.payload?.user?.cache || {}),
                    [fileId]: {
                      ..._upload,
                      uploadedChunks: [...uploads[fileId].uploadedChunks],
                    },
                  },
                },
              }
            );
            response.status(200).json(new CodeResult(0));
          })
          .on("error", (e) => {
            writeStream.close();
            response.status(500).json(
              new CodeResult(1, "分片保存失败", {
                error: { name: e.name, message: e.message },
              })
            );
          });
      },
    ],
  };
};
const complete_upload: Handler = (users) => {
  return {
    path: "/complete",
    method: "post",
    handlers: [
      async (request, response) => {
        const { fileId } = request.body as UploadAPI.CompleteRequestBody;
        const upload = uploads[fileId];

        if (!upload || upload.uploadedChunks.size !== upload.totalChunks) {
          response.status(400).json(
            new CodeResult(1, "上传未完成", {
              nots: new Array(upload.totalChunks)
                .fill(null)
                .reduce<number[]>(
                  (nots, _, index) => (
                    !upload.uploadedChunks.has(index) && nots.push(index), nots
                  ),
                  []
                ),
            })
          );
          return;
        }

        // ./saves/temp/fileId
        const tempDir = path.resolve(baseSavePath, "temp", fileId);
        // ./saves/users/xxx
        const finalDir = path.resolve(
          baseSavePath,
          "users",
          path.basename(upload.userName)
        );

        try {
          if (!fs.existsSync(finalDir)) {
            fs.mkdirSync(finalDir, { recursive: true });
          }
          const finalPath = path.resolve(
            finalDir,
            path.basename(upload.fileName)
          );
          mergeChunks(tempDir, finalPath, upload.totalChunks)
            .then(() => {
              response.status(200).json(new CodeResult(0));
            })
            .finally(async () => {
              try {
                fs.rmSync(tempDir, { recursive: true, force: true });
              } finally {
                const checked = await checkUser(users, upload, false);
                assert(checked.code === 0);
                delete checked.payload?.user?.cache[fileId];

                await users.updateOne(
                  { userName: { $eq: upload.userName } },
                  { $set: { cache: checked.payload?.user?.cache || {} } }
                );
                uploads[fileId].cancelClearId &&
                  clearTimeout(uploads[fileId].cancelClearId);
                delete uploads[fileId];
              }
            });
        } catch (err) {
          response.status(500).json(new CodeResult(1, "合并失败"));
        }
      },
    ],
  };
};
const file_list: Handler = (users) => {
  return {
    path: "/file-list",
    handlers: [
      async (request, response) => {
        const { userName, password, syncFileExtensions } =
          request.body as DownloadAPI.FileListRequestBody;
        if (!userName || !password) {
          response.status(400);
          response.json({ code: 1, message: "用户名或密码不能为空" });
          return;
        }
        // 检查用户是否存在
        const checked = await checkUser(users, { userName, password });
        if (checked.code !== 0) {
          checked.code = 1;
          response.status(400).json(checked);
          return;
        }
        const savepath = path.resolve(
          baseSavePath,
          "users",
          path.basename(userName)
        );
        let created = false;
        if (!fs.existsSync(savepath)) {
          // 用户存在但目录不存在，创建目录
          fs.mkdirSync(savepath, { recursive: true });
          created = true;
        }

        const extensions = new Set<string>(
          syncFileExtensions == undefined
            ? []
            : (Array.isArray(syncFileExtensions)
                ? syncFileExtensions
                : String(syncFileExtensions).split(",")
              ).map((ext) => ext.trim().toLowerCase())
        );
        const files = (created ? [] : fs.readdirSync(savepath))
          .filter((fileName) =>
            syncFileExtensions == undefined
              ? true
              : extensions.has(path.extname(fileName).toLowerCase())
          )
          .map((fileName) => {
            const stats = fs.statSync(path.resolve(savepath, fileName));
            return {
              fileName,
              fileSize: stats.size,
            };
          });
        response
          .status(200)
          .json(new CodeResult<0, Response.FileListResponse>(0, { files }));
      },
    ],
    method: "post",
  };
};
const download: Handler = (users) => {
  return {
    path: "/download",
    handlers: [
      async (request, response) => {
        try {
          const { userName, password, fileName, isChunk } =
            request.body as DownloadAPI.DownloadRequestBody;

          // 验证必要参数
          if (!userName || !password || !fileName) {
            response.status(400).json(new CodeResult(1, "缺少必要参数"));
            return;
          }

          // 验证用户身份
          const checked = await checkUser(users, request.body);
          if (checked.code !== 0) {
            checked.code = 1;
            response.status(400).json(checked);
            return;
          }

          // 构建文件路径
          const filePath = path.resolve(
            baseSavePath,
            "users",
            path.basename(userName),
            path.basename(fileName)
          );

          // 检查文件是否存在
          if (!fs.existsSync(filePath)) {
            response.status(404).json(new CodeResult(1, "文件不存在"));
            return;
          }

          // 获取文件状态
          const stats = fs.statSync(filePath);
          const fileSize = stats.size;

          // 处理分片下载
          if (isChunk) {
            const range = request.headers.range;
            if (!range) {
              response.status(400).json(new CodeResult(1, "需要Range请求头"));
              return;
            }

            // 解析范围参数
            const parts = range.replace(/bytes=/, "").split("-");
            const start = parseInt(parts[0], 10);
            const end = parts[1] ? parseInt(parts[1], 10) : fileSize - 1;

            // 验证范围有效性
            if (start >= fileSize || end >= fileSize || start > end) {
              response
                .status(416)
                .json(new CodeResult(1, "请求范围不符合要求"));
              return;
            }

            // 设置分片下载头
            response.writeHead(206, {
              "Content-Range": `bytes ${start}-${end}/${fileSize}`,
              "Accept-Ranges": "bytes",
              "Content-Length": end - start + 1,
              "Content-Type": "application/octet-stream",
            });

            // 创建文件流
            const fileStream = fs.createReadStream(filePath, { start, end });
            fileStream.pipe(response);
          } else {
            // 普通文件下载
            response.writeHead(200, {
              "Content-Length": fileSize,
              "Content-Type": "application/octet-stream",
              "Content-Disposition": `attachment; filename=""`,
            });

            fs.createReadStream(filePath).pipe(response);
          }
        } catch (error) {
          console.log(error);
          response.status(500).json(new CodeResult(1, "文件下载失败"));
        }
      },
    ],
    method: "post",
  };
};
const create_user: Handler = (users) => {
  return {
    path: "/create",
    method: "post",
    handlers: [
      async (request, response) => {
        const created = await createUser(
          users,
          request.body as UserAPI.UserRequestBody
        );
        if (created.code === 0) {
          response.status(200);
        } else {
          response.status(500);
        }
        response.json(created);
      },
    ],
  };
};
const update_user: Handler = (users) => {
  return {
    path: "/update",
    method: "post",
    handlers: [
      async (request, response) => {
        try {
          const { userName, password, newPassword } =
            request.body as UserAPI.UpdateRequestBody;

          // 参数校验
          if (!userName || !password || !newPassword) {
            response.status(400).json(new CodeResult(1, "缺少必要参数"));
            return;
          }

          // 验证旧密码
          const checked = await checkUser(users, { userName, password });
          if (checked.code !== 0) {
            response.status(401).json(new CodeResult(1, "身份验证失败"));
            return;
          }

          // 生成新密码哈希
          const hashedPassword = await bcrypt.hash(newPassword, saltRounds);

          // 更新数据库
          const result = await users.updateOne(
            { userName: { $eq: userName } },
            { $set: { password: hashedPassword } }
          );

          if (result.modifiedCount === 1) {
            response.json(new CodeResult(0, "密码更新成功"));
          } else {
            response.status(500).json(new CodeResult(1, "密码更新失败"));
          }
        } catch (error) {
          response.status(500).json(new CodeResult(1, "服务器内部错误"));
        }
      },
    ],
  };
};
const delete_user: Handler = (users) => {
  return {
    path: "/delete",
    method: "post",
    handlers: [
      async (request, response) => {
        try {
          const { userName, password } =
            request.body as UserAPI.UserRequestBody;

          // 参数校验
          if (!userName || !password) {
            response.status(400).json(new CodeResult(1, "缺少必要参数"));
            return;
          }

          // 验证用户身份
          const checked = await checkUser(users, request.body);
          if (checked.code !== 0) {
            checked.code = 1;
            response.status(401).json(checked);
            return;
          }

          // 删除数据库记录
          const deleteResult = await users.deleteOne({
            userName: { $eq: userName },
          });

          if (deleteResult.deletedCount === 0) {
            response.status(404).json(new CodeResult(1, "用户不存在"));
            return;
          }

          // 清理用户文件
          const userDir = path.resolve(
            baseSavePath,
            "users",
            path.basename(userName)
          );

          if (fs.existsSync(userDir)) {
            fs.rmSync(userDir, { recursive: true, force: true });
          }

          // 清理缓存中的上传记录
          Object.keys(uploads).forEach((fileId) => {
            if (uploads[fileId].userName === userName) {
              clearTimeout(uploads[fileId].cancelClearId);
              const tempDir = path.resolve(baseSavePath, "temp", fileId);
              fs.existsSync(tempDir) && fs.rmSync(tempDir, { recursive: true });
              delete uploads[fileId];
            }
          });

          response.json(new CodeResult(0, "用户删除成功"));
        } catch (error) {
          console.error("删除用户失败:", error);
          response.status(500).json(new CodeResult(1, "用户删除失败"));
        }
      },
    ],
  };
};
const login_user: Handler = (users) => {
  return {
    path: "/login",
    method: "post",
    handlers: [
      async (request, response) => {
        try {
          const { userName, password } =
            request.body as UserAPI.UserRequestBody;

          // 参数校验
          if (!userName || !password) {
            response.status(400).json(new CodeResult(1, "缺少必要参数"));
            return;
          }

          // 验证用户身份
          const checked = await checkUser(users, request.body);
          if (checked.code !== 0) {
            checked.code = 1;
            response.status(401).json(checked);
            return;
          }
          const result = await users.updateOne(
            { userName: { $eq: userName } },
            { $set: { login: true } }
          );
          if (result.matchedCount > 0) {
            response.status(200).json(new CodeResult(0, "登录成功"));
          } else {
            response.status(500).json(new CodeResult(0, "登录失败"));
          }
        } catch (error) {
          response.status(500).json(new CodeResult(0, "登录失败"));
        }
      },
    ],
  };
};
const logout_user: Handler = (users) => {
  return {
    path: "/logout",
    method: "post",
    handlers: [
      async (request, response) => {
        try {
          const { userName, password } =
            request.body as UserAPI.UserRequestBody;

          // 参数校验
          if (!userName || !password) {
            response.status(400).json(new CodeResult(1, "缺少必要参数"));
            return;
          }

          // 验证用户身份
          const checked = await checkUser(users, request.body);
          if (checked.code !== 0) {
            checked.code = 1;
            response.status(401).json(checked);
            return;
          }
          const result = await users.updateOne(
            { userName: { $eq: userName } },
            { $set: { login: false } }
          );
          if (result.modifiedCount > 0) {
            response.status(200).json(new CodeResult(0, "登出成功"));
          } else {
            response.status(500).json(new CodeResult(1));
          }
        } catch (error) {
          response.status(500).json(new CodeResult(1));
        }
      },
    ],
  };
};
