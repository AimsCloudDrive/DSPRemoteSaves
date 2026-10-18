// 校验 Rust 侧生成的向量能否被 sm-crypto 正确解开（反向互通）
import pkg from 'sm-crypto'
const { sm2, sm4 } = pkg
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const file = resolve(here, '../fixtures/rust-vectors.json')
const data = JSON.parse(readFileSync(file, 'utf8'))

let ok = 0
for (const c of data.cases) {
  // Rust 侧输出带 04 未压缩点前缀，sm-crypto 的 doDecrypt 期望去掉该前缀
  const cipher = c.smkey.startsWith('04') ? c.smkey.slice(2) : c.smkey
  const sm4Key = sm2.doDecrypt(cipher, data.privateKey, 1) // C1C3C2
  if (sm4Key !== c.sm4Key) throw new Error(`sm4 key mismatch: ${sm4Key} != ${c.sm4Key}`)
  const plain = sm4.decrypt(c.password, sm4Key)
  if (plain !== c.plain) throw new Error(`plain mismatch: ${plain} != ${c.plain}`)
  ok++
}
console.log(`rust -> js verified: ${ok}/${data.cases.length}`)
