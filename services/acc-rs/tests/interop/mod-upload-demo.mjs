// 模拟 C# 模组端的完整上传流程（与 DSPRemoteSaves.cs 相同的协议）：
//   公钥获取 → 国密封装密码 → init → 分片(uploadToken) → complete → file-list → download 校验
// 用法：
//   BASE=http://47.109.110.125:9999/dsp.saves/api \
//   MOD_USER=admin MOD_PASS='...' node mod-upload-demo.mjs
import pkg from 'sm-crypto'
import { createHash, randomBytes } from 'node:crypto'
import { writeFile, rm } from 'node:fs/promises'

const { sm2, sm4 } = pkg
const BASE = process.env.BASE || 'http://127.0.0.1:9999/dsp.saves/api'
const USER = process.env.MOD_USER || 'admin'
const PASS = process.env.MOD_PASS || ''
const FILE_NAME = process.env.MOD_FILE || '星系演示存档-01.zip'
const SIZE = Number(process.env.MOD_SIZE || 512 * 1024) // 512KB
const CHUNK = Number(process.env.MOD_CHUNK || 256 * 1024) // 256KB

const sha256 = (buf) => createHash('sha256').update(buf).digest('hex')

// ---- 与模组一致的按请求国密封装 ----
let publicKey = null

async function loadPublicKey() {
  const r = await fetch(`${BASE}/crypto/public-key`)
  const b = await r.json()
  if (b.code !== 0) throw new Error(`获取公钥失败: ${JSON.stringify(b)}`)
  publicKey = b.payload
  console.log(`[1] 公钥已获取 keyId=${publicKey.keyId}`)
}

function seal() {
  const key = randomBytes(16).toString('hex')
  const smkey = sm2.doEncrypt(key, publicKey.publicKey, 1) // C1C3C2，无 04 前缀
  return {
    smkey,
    password: sm4.encrypt(PASS, key), // SM4-ECB/PKCS7
    headers: { smkey, smkeyid: publicKey.keyId },
  }
}

async function postSealed(path, payload) {
  const s = seal()
  const r = await fetch(`${BASE}${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...s.headers },
    body: JSON.stringify({
      userName: USER,
      password: s.password,
      timestamp: Date.now(),
      nonce: randomBytes(8).toString('hex'),
      ...payload,
    }),
  })
  const b = await r.json().catch(() => ({}))
  if (b.code !== 0) throw new Error(`${path} 失败: HTTP ${r.status} ${JSON.stringify(b)}`)
  return b.payload
}

async function main() {
  if (!PASS) throw new Error('请通过 MOD_PASS 提供密码')
  await loadPublicKey()

  // 模拟一个"存档"：伪随机内容（可复现）
  const content = Buffer.alloc(SIZE)
  let seed = 0x9e3779b9
  for (let i = 0; i < SIZE; i++) {
    seed = (seed * 1103515245 + 12345) >>> 0
    content[i] = seed & 0xff
  }
  console.log(`[2] 模拟存档 ${FILE_NAME}，${SIZE} 字节，sha256=${sha256(content).slice(0, 16)}…`)

  // init
  const data = (await postSealed('/upload/init', {
    fileName: FILE_NAME,
    fileSize: SIZE,
    chunkSize: CHUNK,
  })).data
  const { fileId, chunkSize, totalChunks, uploadToken } = data
  console.log(`[3] init 成功 fileId=${fileId} chunkSize=${chunkSize} totalChunks=${totalChunks}（uploadToken 已签发，仅此次会话有效）`)

  // 分片上传：只带 uploadToken，不再携带密码
  for (let i = 0; i < totalChunks; i++) {
    const chunk = content.subarray(i * chunkSize, Math.min((i + 1) * chunkSize, SIZE))
    const r = await fetch(`${BASE}/upload/chunk?fileId=${fileId}&chunkIndex=${i}`, {
      method: 'POST',
      headers: { 'content-type': 'application/octet-stream', uploadToken },
      body: chunk,
    })
    const b = await r.json().catch(() => ({}))
    if (b.code !== 0) throw new Error(`chunk ${i} 失败: ${JSON.stringify(b)}`)
    console.log(`[4] chunk ${i + 1}/${totalChunks} 已上传（${chunk.length} 字节）`)
  }

  // complete
  await postSealed('/upload/complete', { fileId })
  console.log('[5] complete 成功，服务端已合并并落库')

  // file-list
  const files = (await postSealed('/download/file-list', {})).files
  const row = files.find((f) => f.fileName === FILE_NAME)
  console.log(`[6] file-list 共 ${files.length} 个文件；本文件：`, row)

  // download 回读校验
  const s = seal()
  const r = await fetch(`${BASE}/download/download`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...s.headers },
    body: JSON.stringify({
      userName: USER,
      password: s.password,
      fileName: FILE_NAME,
      timestamp: Date.now(),
      nonce: randomBytes(8).toString('hex'),
    }),
  })
  if (!r.ok) throw new Error(`下载失败 HTTP ${r.status}`)
  const back = Buffer.from(await r.arrayBuffer())
  const same = back.equals(content)
  console.log(`[7] 下载回读 ${back.length} 字节，内容比对 ${same ? '一致 ✓' : '不一致 ✗'}`)
  if (!same) throw new Error('下载内容与上传不一致')

  // 留一份本地副本便于查看
  await writeFile('mod-upload-demo.bin', back)
  console.log(`\n结果：模组协议上传 → 版本落库 → 列表 → 回读 全部成功（本地副本 mod-upload-demo.bin）`)
}

main()
  .catch((e) => {
    console.error('演示失败:', e.message)
    process.exitCode = 1
  })
  .finally(() => rm('mod-upload-demo.bin', { force: true }))
