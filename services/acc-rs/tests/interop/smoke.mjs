// 端到端契约冒烟测试：国密登录 → 分片上传 → 版本 → 回退 → 删除 → 安全用例
// 用法：node tests/smoke.mjs   （需先启动 acc-server，默认 http://127.0.0.1:9999/dsp.saves/api）
import pkg from 'sm-crypto'
import { createHash, randomBytes } from 'node:crypto'

const { sm2, sm4 } = pkg
const BASE = process.env.BASE || 'http://127.0.0.1:9999/dsp.saves/api'
const USER = process.env.SMOKE_USER || 'admin'
const PASS = process.env.SMOKE_PASS || ''

let passed = 0
let failed = 0

function check(name, ok, extra = '') {
  if (ok) {
    passed++
    console.log(`  ✓ ${name}`)
  } else {
    failed++
    console.error(`  ✗ ${name} ${extra}`)
  }
}

const sha256 = (buf) => createHash('sha256').update(buf).digest('hex')

let publicKey = null

async function loadPublicKey() {
  const response = await fetch(`${BASE}/crypto/public-key`)
  const body = await response.json()
  if (body.code !== 0) throw new Error(`public-key failed: ${JSON.stringify(body)}`)
  publicKey = body.payload
  return publicKey
}

function seal(values) {
  const key = randomBytes(16).toString('hex')
  const smkey = sm2.doEncrypt(key, publicKey.publicKey, 1)
  return {
    smkey,
    values: values.map((value) => sm4.encrypt(value, key)),
    headers: { smkey, smkeyid: publicKey.keyId },
  }
}

async function call(method, path, { body, headers = {}, raw, contentType } = {}) {
  const init = { method, headers: { ...headers } }
  if (raw !== undefined) {
    init.body = raw
    init.headers['content-type'] = contentType || 'application/octet-stream'
  } else if (body !== undefined) {
    init.body = JSON.stringify(body)
    init.headers['content-type'] = 'application/json'
  }
  const response = await fetch(`${BASE}${path}`, init)
  return response
}

async function json(response) {
  const text = await response.text()
  try {
    return JSON.parse(text)
  } catch {
    return { raw: text }
  }
}

function authBody(extra = {}) {
  const sealed = seal([PASS])
  return {
    body: { userName: USER, password: sealed.values[0], timestamp: Date.now(), nonce: randomBytes(8).toString('hex'), ...extra },
    headers: sealed.headers,
  }
}

async function login() {
  const { body, headers } = authBody()
  const response = await call('POST', '/auth/login', { body, headers })
  const data = await json(response)
  if (data.code !== 0) throw new Error(`login failed: ${JSON.stringify(data)}`)
  return data.payload
}

async function uploadFile(token, fileName, content, chunkSize = 65536) {
  const { body, headers } = authBody({ fileName, fileSize: content.length, chunkSize })
  const initResponse = await call('POST', '/upload/init', { body, headers })
  const initData = await json(initResponse)
  if (initData.code !== 0) throw new Error(`init failed: ${JSON.stringify(initData)}`)
  const info = initData.payload.data

  const total = info.totalChunks
  for (let index = 0; index < total; index++) {
    const start = index * info.chunkSize
    const slice = content.subarray(start, Math.min(start + info.chunkSize, content.length))
    const response = await call(
      'POST',
      `/upload/chunk?fileId=${encodeURIComponent(info.fileId)}&chunkIndex=${index}`,
      { raw: slice, headers: { uploadToken: info.uploadToken } },
    )
    if (response.status !== 200) {
      throw new Error(`chunk ${index} failed: ${response.status} ${await response.text()}`)
    }
  }

  const completeAuth = authBody({ fileId: info.fileId })
  const completeResponse = await call('POST', '/upload/complete', {
    body: completeAuth.body,
    headers: completeAuth.headers,
  })
  const completeData = await json(completeResponse)
  if (completeData.code !== 0) throw new Error(`complete failed: ${JSON.stringify(completeData)}`)
  return info
}

async function download(fileName, isChunk = false, range) {
  const { body, headers } = authBody({ fileName, isChunk })
  if (range) headers.Range = range
  const response = await call('POST', '/download/download', { body, headers })
  return response
}

async function main() {
  console.log(`Smoke test against ${BASE}`)

  console.log('\n[1] 健康检查与公钥')
  const health = await call('GET', '/healthz')
  check('healthz 200', health.status === 200)
  const ready = await call('GET', '/readyz')
  const readyBody = await json(ready)
  check('readyz mongo ok', readyBody.mongo === true, JSON.stringify(readyBody))
  await loadPublicKey()
  check('publicKey 以 04 开头且 130 hex', publicKey.publicKey.startsWith('04') && publicKey.publicKey.length === 130)
  check('keyId 16 hex', /^[0-9a-f]{16}$/.test(publicKey.keyId))
  check('模式标注', publicKey.sm2Mode === 'C1C3C2' && publicKey.sm4Mode === 'ECB/PKCS7')

  console.log('\n[2] 登录与身份')
  const session = await login()
  check('返回 token 与用户', Boolean(session.token && session.user?.userName === USER))
  const me = await call('GET', '/auth/me', { headers: { authorization: `Bearer ${session.token}` } })
  const meBody = await json(me)
  check('auth/me 正常', meBody.code === 0 && meBody.payload.userName === USER, JSON.stringify(meBody))

  console.log('\n[3] 明文/重放/时钟/密钥安全用例')
  const plain = await call('POST', '/auth/login', {
    body: { userName: USER, password: PASS, timestamp: Date.now() },
  })
  const plainBody = await json(plain)
  check('明文密码被拒 (2006)', plainBody.code === 2006, JSON.stringify(plainBody))

  const replaySealed = seal([PASS])
  const replayBody = {
    userName: USER,
    password: replaySealed.values[0],
    timestamp: Date.now(),
    nonce: 'fixed-nonce',
  }
  const first = await call('POST', '/auth/login', { body: replayBody, headers: replaySealed.headers })
  const firstBody = await json(first)
  const second = await call('POST', '/auth/login', { body: replayBody, headers: replaySealed.headers })
  const secondBody = await json(second)
  check('首次登录成功', firstBody.code === 0, JSON.stringify(firstBody))
  check('重放被拒 (2004)', secondBody.code === 2004, JSON.stringify(secondBody))

  const skewed = seal([PASS])
  const skewResponse = await call('POST', '/auth/login', {
    body: {
      userName: USER,
      password: skewed.values[0],
      timestamp: Date.now() - 301_000,
      nonce: 'skew',
    },
    headers: skewed.headers,
  })
  const skewBody = await json(skewResponse)
  check('时间戳超窗 (2003)', skewBody.code === 2003, JSON.stringify(skewBody))

  const unknown = seal([PASS])
  const unknownResponse = await call('POST', '/auth/login', {
    body: { userName: USER, password: unknown.values[0], timestamp: Date.now(), nonce: 'unknown-key' },
    headers: { ...unknown.headers, smkeyid: 'deadbeefdeadbeef' },
  })
  const unknownBody = await json(unknownResponse)
  check('未知 keyId (2005)', unknownBody.code === 2005, JSON.stringify(unknownBody))

  console.log('\n[4] 上传 → 列表 → 下载')
  const name = `smoke-${Date.now()}.dsv`
  const v1 = randomBytes(200_000)
  await uploadFile(session.token, name, v1)

  const listResponse = await call('POST', '/download/file-list', authBodyPayload())
  function authBodyPayload() {
    const sealed = seal([PASS])
    return {
      body: {
        userName: USER,
        password: sealed.values[0],
        timestamp: Date.now(),
        syncFileExtensions: ['.dsv'],
      },
      headers: sealed.headers,
    }
  }
  const listBody = await json(listResponse)
  check('file-list code=0', listBody.code === 0, JSON.stringify(listBody))
  const entry = listBody.payload?.files?.find((file) => file.fileName === name)
  check('列表包含新文件', Boolean(entry))
  check('lastModified 为毫秒', typeof entry?.lastModified === 'number' && entry.lastModified > 1e12)

  const full = await download(name)
  const fullBuffer = Buffer.from(await full.arrayBuffer())
  check('整文件下载 200', full.status === 200)
  check('内容与 SHA256 一致', sha256(fullBuffer) === sha256(v1), `${fullBuffer.length} vs ${v1.length}`)

  const ranged = await download(name, true, 'bytes=0-99')
  const rangedBuffer = Buffer.from(await ranged.arrayBuffer())
  check('Range 返回 206', ranged.status === 206)
  check('Content-Range 正确', ranged.headers.get('content-range') === `bytes 0-99/${v1.length}`)
  check('Range 内容一致', rangedBuffer.length === 100 && sha256(rangedBuffer) === sha256(v1.subarray(0, 100)))

  console.log('\n[5] 版本：覆盖 → 回退 → 删除当前回退')
  const v2 = randomBytes(150_000)
  await uploadFile(session.token, name, v2)

  const versionsResponse = await call(
    'GET',
    `/user/saves/${encodeURIComponent(name)}/versions`,
    { headers: { authorization: `Bearer ${session.token}` } },
  )
  const versionsBody = await json(versionsResponse)
  check('版本列表 2 条', versionsBody.payload?.versions?.length === 2, JSON.stringify(versionsBody))
  const seq1 = versionsBody.payload.versions.find((version) => version.seq === 1)
  const seq2 = versionsBody.payload.versions.find((version) => version.seq === 2)

  const rollback = await call(
    'POST',
    `/user/saves/${encodeURIComponent(name)}/versions/${seq1.id}/rollback`,
    { headers: { authorization: `Bearer ${session.token}` } },
  )
  const rollbackBody = await json(rollback)
  check('回退成功', rollbackBody.code === 0, JSON.stringify(rollbackBody))

  const afterRollback = Buffer.from(await (await download(name)).arrayBuffer())
  check('回退后内容 = v1', sha256(afterRollback) === sha256(v1))

  // 删除当前（回退到上一版：此时 current=seq1，之前无更早版本 → 文件整体删除）
  const del = await call('DELETE', `/user/saves/${encodeURIComponent(name)}?deleteVersions=false`, {
    headers: { authorization: `Bearer ${session.token}` },
  })
  const delBody = await json(del)
  check('删除当前成功', delBody.code === 0, JSON.stringify(delBody))
  const afterDelete = await download(name)
  check('无更早版本时文件消失', afterDelete.status === 404 || (await json(afterDelete)).code === 3007)

  console.log('\n[6] 重新上传后彻底删除')
  await uploadFile(session.token, name, v1)
  const purge = await call('DELETE', `/user/saves/${encodeURIComponent(name)}?deleteVersions=true`, {
    headers: { authorization: `Bearer ${session.token}` },
  })
  const purgeBody = await json(purge)
  check('彻底删除成功', purgeBody.code === 0, JSON.stringify(purgeBody))

  console.log('\n[7] 审计与统计（管理端受众）')
  const adminSealed = seal([PASS])
  const adminLogin = await call('POST', '/admin/auth/login', {
    body: {
      userName: USER,
      password: adminSealed.values[0],
      timestamp: Date.now(),
      nonce: randomBytes(8).toString('hex'),
    },
    headers: adminSealed.headers,
  })
  const adminBody = await json(adminLogin)
  check('管理端登录成功', adminBody.code === 0, JSON.stringify(adminBody))

  const userTokenAsAdmin = await call('GET', '/admin/users?page=1', {
    headers: { authorization: `Bearer ${session.token}` },
  })
  check('用户受众 token 被管理端拒绝', userTokenAsAdmin.status === 401 || (await json(userTokenAsAdmin)).code === 1002)

  const adminToken = adminBody.payload?.token
  const audit = await call('GET', '/admin/audit?page=1&pageSize=5', {
    headers: { authorization: `Bearer ${adminToken}` },
  })
  const auditBody = await json(audit)
  check('审计可读', auditBody.code === 0 && auditBody.payload.items.length > 0, JSON.stringify(auditBody))

  const overview = await call('GET', '/admin/stats/overview', {
    headers: { authorization: `Bearer ${adminToken}` },
  })
  const overviewBody = await json(overview)
  check('概览可读', overviewBody.code === 0 && overviewBody.payload.users.total >= 1, JSON.stringify(overviewBody))

  console.log(`\n结果：${passed} 通过，${failed} 失败`)
  if (failed > 0) process.exit(1)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
