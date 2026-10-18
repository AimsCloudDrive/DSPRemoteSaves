// 生成 JS(sm-crypto) 侧的国密互通向量：
//   smkey = SM2(C1C3C2, sm4Key 的 32 位 hex 文本)
//   password = SM4-ECB-PKCS7(明文, sm4Key)
// 用法：node gen-vectors.mjs
import pkg from 'sm-crypto'
const { sm2, sm4 } = pkg
import { randomBytes } from 'node:crypto'
import { writeFileSync, mkdirSync, readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const outFile = resolve(here, '../fixtures/sm-vectors.json')

const randomHex = (bytes) => randomBytes(bytes).toString('hex')

const fixed = [
  'a', 'A1!', '1234567890', 'Abcdef123!', 'a'.repeat(15), 'a'.repeat(16),
  'a'.repeat(17), 'a'.repeat(31), 'a'.repeat(32), 'a'.repeat(64),
  '密码Abc123!@#', '带中文的密码2026！', 'emoji🔐🚀Pass123!', ' 空格开头 End!9 ',
  'p@ssw0rd\u0000nul', 'Line\nBreak9!', '\\slash/and"quote\'9',
]

function randomPassword() {
  const pools = [
    'abcdefghijklmnopqrstuvwxyz',
    'ABCDEFGHIJKLMNOPQRSTUVWXYZ',
    '0123456789',
    '!@#$%^&*()_+-=[]{};:,.<>?',
  ]
  const len = 8 + Math.floor(Math.random() * 60)
  let s = ''
  for (const p of pools) s += p[Math.floor(Math.random() * p.length)]
  while (s.length < len) {
    const pool = pools[Math.floor(Math.random() * pools.length)]
    s += pool[Math.floor(Math.random() * pool.length)]
  }
  return s
}

const keys = sm2.generateKeyPairHex()

const cases = []
for (const plain of fixed) cases.push(plain)
for (let i = 0; i < 1000; i++) cases.push(randomPassword())

const vectors = cases.map((plain) => {
  const sm4Key = randomHex(16)
  const smkey = sm2.doEncrypt(sm4Key, keys.publicKey, 1) // 1 = C1C3C2
  const password = sm4.encrypt(plain, sm4Key) // 默认 ECB + PKCS#7，输出 hex
  return { plain, sm4Key, smkey, password }
})

mkdirSync(dirname(outFile), { recursive: true })
writeFileSync(
  outFile,
  JSON.stringify(
    {
      generator: 'sm-crypto@' + JSON.parse(readFileSync(resolve(here, 'node_modules/sm-crypto/package.json'), 'utf8')).version,
      sm2Mode: 'C1C3C2',
      sm4Mode: 'ECB/PKCS7',
      privateKey: keys.privateKey,
      publicKey: keys.publicKey,
      cases: vectors,
    },
    null,
    2,
  ),
)

console.log(`wrote ${vectors.length} vectors -> ${outFile}`)
console.log('publicKey =', keys.publicKey.slice(0, 20) + '...')
