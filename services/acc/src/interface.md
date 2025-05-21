# DSP 云存档同步服务接口文档

## 概述

本服务提供基于用户认证的存档文件云同步功能，包含用户管理、分块上传、断点续传、差异同步等核心功能。所有接口均通过 `/dsp.saves/api` 路径访问。

## 接口列表

### 用户管理

| 接口路径      | 方法 | 描述         | 请求体 |
| ------------- | ---- | ------------ | ------ |
| /users/create | POST | 创建新用户   |        |
| /users/login  | POST | 用户登录     |        |
| /users/logout | POST | 用户登出     |        |
| /users/update | POST | 更新用户密码 |        |
| /users/delete | POST | 删除用户账户 |        |

### 文件上传

| 接口路径         | 方法 | 描述               |
| ---------------- | ---- | ------------------ |
| /upload/init     | POST | 初始化分块上传任务 |
| /upload/chunk    | POST | 上传文件分块       |
| /upload/complete | POST | 完成分块上传       |

### 文件下载

| 接口路径            | 方法 | 描述               |
| ------------------- | ---- | ------------------ |
| /download/file-list | POST | 获取可同步文件列表 |
| /download           | POST | 下载文件/分块下载  |

---

## 接口详情

### 1. 创建用户

**请求参数**:

```json
{
  "userName": "string", // 必填
  "password": "string" // 必填
}
```

**响应示例**:

```json
{
  "code": 0, // 0-成功 1-用户已存在
  "message": "创建成功"
}
```

### 2. 初始化分块上传

**请求参数**:

```json
{
  "userName": "string",    // 必填
  "password": "string",    // 必填
  "fileName": "string",    // 必填
  "fileSize": number,      // 必填（单位：字节）
  "chunkSize": number,     // 必填（单位：字节）
  "ttl": 300              // 可选，上传会话有效期（秒）
}
```

**响应示例**:

```json
{
  "code": 0,
  "payload": {
    "fileId": "uuid", // 上传会话ID
    "chunkSize": 5242880, // 实际使用的分块大小
    "totalChunks": 10 // 总分块数
  }
}
```

### 3. 上传文件分块

**请求格式**:

```txt
POST /upload/chunk?fileId=xxx&chunkIndex=0
[二进制分片数据]
```

**响应示例**:

```json
{
  "code": 0 // 0-成功 1-参数错误
}
```

### 4. 获取文件列表

**请求参数**:

```json
{
  "userName": "string", // 必填
  "password": "string", // 必填
  "syncFileExtensions": ".dsv" // 可选，扩展名过滤（逗号分隔）
}
```

**响应示例**:

```json
{
  "code": 0,
  "payload": {
    "files": [
      {
        "fileName": "save1.dsv",
        "fileSize": 10485760
      }
    ]
  }
}
```

### 5. 分块下载

**请求头**:

```txt
Range: bytes=0-1048575
```

**响应头**:

```txt
Content-Range: bytes 0-1048575/10485760
Content-Length: 1048576
```

## 存储结构

```txt
./saves/
├── temp/            // 上传临时目录
│   └── [fileId]/
└── users/
    └── [userName]/  // 用户存档目录
        └── *.dsv
```
