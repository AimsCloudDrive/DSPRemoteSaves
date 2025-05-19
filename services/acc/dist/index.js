"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const bcrypt_1 = __importDefault(require("bcrypt"));
const fs_1 = __importDefault(require("fs"));
const mongodb_1 = require("mongodb");
const path_1 = __importDefault(require("path"));
const uuid_1 = require("uuid");
const mergeChunk_1 = require("./mergeChunk");
const server_1 = require("./server");
function assert(condition, message = "") {
    if (!condition)
        throw Error(message);
}
class CodeResult {
    constructor(code, message, payload) {
        this.code = code;
        if (payload) {
            if (typeof message === "object" && message !== null) {
                throw Error();
            }
            if (message != undefined) {
                this.message = message;
            }
            this.payload = payload;
        }
        else if (message != undefined) {
            if (typeof message === "object") {
                this.payload = message;
            }
            else {
                this.message = message;
            }
        }
        else {
        }
    }
}
const saltRounds = 10;
async function createUser(users, user) {
    const checked = await checkUser(users, user);
    if (checked.code !== 1) {
        return new CodeResult(1, "用户已存在");
    }
    let { userName, password } = user;
    password = await bcrypt_1.default.hash(password, saltRounds);
    return await users
        .insertOne({ userName, password, cache: {}, login: false })
        .then((v) => !!v.insertedId
        ? new CodeResult(0, "创建成功")
        : new CodeResult(1, "创建失败"), (e) => new CodeResult(1, "创建失败"));
}
/**
 * {code: 0 | 1 | 2}
 * 0: 用户名密码正确，1：用户不存在，2：用户名或密码错误
 * @param users
 * @param user
 * @param options
 * @returns
 */
async function checkUser(users, { userName, password = "", }, options) {
    const finded = await users.findOne({
        userName: { $eq: userName },
    });
    if (!finded) {
        return new CodeResult(1, "用户不存在");
    }
    else if ((typeof options === "object" ? options.comparePassword : options) !==
        false &&
        !bcrypt_1.default.compareSync(password, finded.password)) {
        return new CodeResult(2, "用户名或密码错误");
    }
    else {
        return new CodeResult(0, { user: finded });
    }
}
new mongodb_1.MongoClient("mongodb://never.aims.nevermonarch.cn:57857/", {
    auth: {
        username: "root",
        password: "123456",
    },
})
    .connect()
    .then(async (client) => {
    const db = client.db("DSPCloudSaves");
    const users = db.collection("Users");
    (0, server_1.createServer)(9999, {
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
}, () => {
    console.log("content Error");
});
const baseSavePath = "./saves";
const uploads = Object.create(null);
const init_upload = (users) => {
    return {
        path: "/init",
        method: "post",
        handlers: [
            async (request, response) => {
                const { fileName, fileSize, chunkSize, userName, password, ttl = 5 * 60, } = request.body;
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
                const fileId = (0, uuid_1.v4)();
                const totalChunks = Math.ceil(fileSize / chunkSize);
                const tempDir = path_1.default.resolve(baseSavePath, "temp", fileId);
                fs_1.default.mkdirSync(tempDir, { recursive: true });
                uploads[fileId] = {
                    userName,
                    fileName: path_1.default.basename(fileName),
                    fileSize,
                    chunkSize,
                    totalChunks,
                    uploadedChunks: new Set(),
                    createdAt: Date.now(),
                    ttl,
                    cancelClearId: setTimeout(() => {
                        const tempDir = path_1.default.resolve(baseSavePath, "temp", fileId);
                        try {
                            fs_1.default.existsSync(tempDir) && fs_1.default.rmSync(tempDir, { recursive: true });
                        }
                        finally {
                            delete uploads[fileId];
                        }
                    }, ttl * 1000),
                };
                const { cancelClearId, ..._upload } = uploads[fileId];
                await users.updateOne({
                    userName: { $eq: userName },
                    password: { $eq: password },
                }, {
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
                });
                response
                    .status(200)
                    .json(new CodeResult(0, { data: { fileId, chunkSize, totalChunks } }));
            },
        ],
    };
};
const upload_chunk = (users) => {
    return {
        path: "/chunk",
        method: "post",
        handlers: [
            (request, response) => {
                const fileId = request.query.fileId;
                const _chunkIndex = request.query.chunkIndex;
                const chunkIndex = typeof _chunkIndex === "string" ? parseInt(_chunkIndex) : Number.NaN;
                if (!fileId || Number.isNaN(chunkIndex)) {
                    response.status(400).json(new CodeResult(1, "参数错误"));
                    return;
                }
                const upload = uploads[fileId];
                if (!upload || chunkIndex < 0 || chunkIndex >= upload.totalChunks) {
                    response.status(400).json(new CodeResult(1, "无效请求"));
                    return;
                }
                const tempDir = path_1.default.resolve(baseSavePath, "temp", fileId);
                const chunkPath = path_1.default.resolve(tempDir, `${chunkIndex}.chunk`);
                const writeStream = fs_1.default.createWriteStream(chunkPath);
                request
                    .pipe(writeStream)
                    .on("finish", async () => {
                    upload.uploadedChunks.add(chunkIndex);
                    const checked = await checkUser(users, upload, false);
                    if (checked.code !== 0) {
                        throw new Error();
                    }
                    const { cancelClearId, ..._upload } = upload;
                    users.updateOne({
                        userName: { $eq: upload.userName },
                    }, {
                        $set: {
                            cache: {
                                ...(checked.payload?.user?.cache || {}),
                                [fileId]: {
                                    ..._upload,
                                    uploadedChunks: [...uploads[fileId].uploadedChunks],
                                },
                            },
                        },
                    });
                    response.status(200).json(new CodeResult(0));
                })
                    .on("error", (e) => response.status(500).json(new CodeResult(1, "分片保存失败", {
                    error: { name: e.name, message: e.message },
                })));
            },
        ],
    };
};
const complete_upload = (users) => {
    return {
        path: "/complete",
        method: "post",
        handlers: [
            async (request, response) => {
                const { fileId } = request.body;
                const upload = uploads[fileId];
                if (!upload || upload.uploadedChunks.size !== upload.totalChunks) {
                    response.status(400).json(new CodeResult(1, "上传未完成", {
                        nots: new Array(upload.totalChunks)
                            .fill(null)
                            .reduce((nots, _, index) => (!upload.uploadedChunks.has(index) && nots.push(index), nots), []),
                    }));
                    return;
                }
                // ./saves/temp/fileId
                const tempDir = path_1.default.resolve(baseSavePath, "temp", fileId);
                // ./saves/users/xxx
                const finalDir = path_1.default.resolve(baseSavePath, "users", path_1.default.basename(upload.userName));
                try {
                    if (!fs_1.default.existsSync(finalDir)) {
                        fs_1.default.mkdirSync(finalDir, { recursive: true });
                    }
                    const finalPath = path_1.default.resolve(finalDir, path_1.default.basename(upload.fileName));
                    (0, mergeChunk_1.mergeChunks)(tempDir, finalPath, upload.totalChunks)
                        .then(() => {
                        response.status(200).json(new CodeResult(0));
                    })
                        .finally(async () => {
                        try {
                            fs_1.default.rmSync(tempDir, { recursive: true, force: true });
                        }
                        finally {
                            const checked = await checkUser(users, upload, false);
                            assert(checked.code === 0);
                            delete checked.payload?.user?.cache[fileId];
                            await users.updateOne({ userName: { $eq: upload.userName } }, { $set: { cache: checked.payload?.user?.cache || {} } });
                            uploads[fileId].cancelClearId &&
                                clearTimeout(uploads[fileId].cancelClearId);
                            delete uploads[fileId];
                        }
                    });
                }
                catch (err) {
                    response.status(500).json(new CodeResult(1, "合并失败"));
                }
            },
        ],
    };
};
const file_list = (users) => {
    return {
        path: "/file-list",
        handlers: [
            async (request, response) => {
                const { userName, password } = request.body;
                console.log("file-list-1", userName, password);
                if (!userName || !password) {
                    response.status(400);
                    response.send({ code: 1, message: "用户名或密码不能为空" });
                    return;
                }
                console.log("file-list-2", userName, password);
                // 检查用户是否存在
                const checked = await checkUser(users, { userName, password });
                if (checked.code !== 0) {
                    checked.code = 1;
                    response.status(400).json(checked);
                    return;
                }
                console.log("file-list-3", userName, password);
                const savepath = path_1.default.resolve(baseSavePath, "users", path_1.default.basename(userName));
                let created = false;
                if (!fs_1.default.existsSync(savepath)) {
                    // 用户存在但目录不存在，创建目录
                    fs_1.default.mkdirSync(savepath, { recursive: true });
                    created = true;
                }
                const files = (created ? [] : fs_1.default.readdirSync(savepath)).map((fileName) => {
                    const stats = fs_1.default.statSync(path_1.default.resolve(savepath, fileName));
                    return {
                        fileName,
                        fileSize: stats.size,
                    };
                });
                console.log("file-list-4", userName, password);
                response.status(200).send(new CodeResult(0, { files }));
                console.log("file-list-5", userName, password);
            },
        ],
        method: "post",
    };
};
const download = (users) => {
    return {
        path: "/download",
        handlers: [
            async (request, response) => {
                try {
                    const { userName, password, fileName, isChunk } = request.body;
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
                    const filePath = path_1.default.resolve(baseSavePath, "users", path_1.default.basename(userName), path_1.default.basename(fileName));
                    // 检查文件是否存在
                    if (!fs_1.default.existsSync(filePath)) {
                        response.status(404).json(new CodeResult(1, "文件不存在"));
                        return;
                    }
                    // 获取文件状态
                    const stats = fs_1.default.statSync(filePath);
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
                        const fileStream = fs_1.default.createReadStream(filePath, { start, end });
                        fileStream.pipe(response);
                    }
                    else {
                        // 普通文件下载
                        response.writeHead(200, {
                            "Content-Length": fileSize,
                            "Content-Type": "application/octet-stream",
                            "Content-Disposition": `attachment; filename="${fileName}"`,
                        });
                        fs_1.default.createReadStream(filePath).pipe(response);
                    }
                }
                catch (error) {
                    response.status(500).json(new CodeResult(1, "文件下载失败"));
                }
            },
        ],
        method: "post",
    };
};
const create_user = (users) => {
    return {
        path: "/create",
        method: "post",
        handlers: [
            async (request, response) => {
                const created = await createUser(users, request.body);
                if (created.code === 0) {
                    response.status(200);
                }
                else {
                    response.status(500);
                }
                response.json(created);
            },
        ],
    };
};
const update_user = (users) => {
    return {
        path: "/update",
        method: "post",
        handlers: [
            async (request, response) => {
                try {
                    const { userName, password, newPassword } = request.body;
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
                    const hashedPassword = await bcrypt_1.default.hash(newPassword, saltRounds);
                    // 更新数据库
                    const result = await users.updateOne({ userName: { $eq: userName } }, { $set: { password: hashedPassword } });
                    if (result.modifiedCount === 1) {
                        response.json(new CodeResult(0, "密码更新成功"));
                    }
                    else {
                        response.status(500).json(new CodeResult(1, "密码更新失败"));
                    }
                }
                catch (error) {
                    response.status(500).json(new CodeResult(1, "服务器内部错误"));
                }
            },
        ],
    };
};
const delete_user = (users) => {
    return {
        path: "/delete",
        method: "post",
        handlers: [
            async (request, response) => {
                try {
                    const { userName, password } = request.body;
                    // 参数校验
                    if (!userName || !password) {
                        response.status(400).json(new CodeResult(1, "缺少必要参数"));
                        return;
                    }
                    // 验证用户身份
                    const checked = await checkUser(users, { userName, password });
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
                    const userDir = path_1.default.resolve(baseSavePath, "users", path_1.default.basename(userName));
                    if (fs_1.default.existsSync(userDir)) {
                        fs_1.default.rmSync(userDir, { recursive: true, force: true });
                    }
                    // 清理缓存中的上传记录
                    Object.keys(uploads).forEach((fileId) => {
                        if (uploads[fileId].userName === userName) {
                            clearTimeout(uploads[fileId].cancelClearId);
                            const tempDir = path_1.default.resolve(baseSavePath, "temp", fileId);
                            fs_1.default.existsSync(tempDir) && fs_1.default.rmSync(tempDir, { recursive: true });
                            delete uploads[fileId];
                        }
                    });
                    response.json(new CodeResult(0, "用户删除成功"));
                }
                catch (error) {
                    console.error("删除用户失败:", error);
                    response.status(500).json(new CodeResult(1, "用户删除失败"));
                }
            },
        ],
    };
};
const login_user = (users) => {
    return {
        path: "/login",
        method: "post",
        handlers: [
            async (request, response) => {
                try {
                    const { userName, password } = request.body;
                    // 参数校验
                    if (!userName || !password) {
                        response.status(400).json(new CodeResult(1, "缺少必要参数"));
                        return;
                    }
                    // 验证用户身份
                    const checked = await checkUser(users, { userName, password });
                    if (checked.code !== 0) {
                        checked.code = 1;
                        response.status(401).json(checked);
                        return;
                    }
                    const result = await users.updateOne({ userName: { $eq: userName } }, { $set: { login: true } });
                    if (result.modifiedCount > 0) {
                        response.status(200).json(new CodeResult(0, "登录成功"));
                    }
                    else {
                        response.status(500).send(new CodeResult(0, "登录失败"));
                    }
                }
                catch (error) {
                    response.status(500).send(new CodeResult(0, "登录失败"));
                }
            },
        ],
    };
};
const logout_user = (users) => {
    return {
        path: "/logout",
        method: "post",
        handlers: [
            async (request, response) => {
                try {
                    const { userName, password } = request.body;
                    // 参数校验
                    if (!userName || !password) {
                        response.status(400).json(new CodeResult(1, "缺少必要参数"));
                        return;
                    }
                    // 验证用户身份
                    const checked = await checkUser(users, { userName, password });
                    if (checked.code !== 0) {
                        checked.code = 1;
                        response.status(401).json(checked);
                        return;
                    }
                    const result = await users.updateOne({ userName: { $eq: userName } }, { $set: { login: false } });
                    if (result.modifiedCount > 0) {
                        response.status(200).json(new CodeResult(0, "登出成功"));
                    }
                    else {
                        response.status(500).send(new CodeResult(1));
                    }
                }
                catch (error) {
                    response.status(500).send(new CodeResult(1));
                }
            },
        ],
    };
};
