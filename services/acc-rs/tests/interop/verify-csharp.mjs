// 校验 C#（BepInEx 插件内自实现）生成的国密信封可被 sm-crypto 解开
import pkg from 'sm-crypto'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const { sm2, sm4 } = pkg
const here = dirname(fileURLToPath(import.meta.url))

const fixture = JSON.parse(readFileSync(resolve(here, '../fixtures/sm-vectors.json'), 'utf8'))
const data = JSON.parse(readFileSync(resolve(here, '../fixtures/csharp-vectors.json'), 'utf8'))

let ok = 0
for (const c of data.cases) {
  if (!c.smkey.startsWith('04')) throw new Error('C# SM2 密文缺少 04 未压缩点前缀')
  if (c.smkey.length !== 2 * (1 + 64 + 32 + 32)) throw new Error(`长度异常: ${c.smkey.length}`)
  const sm4Key = sm2.doDecrypt(c.smkey.slice(2), fixture.privateKey, 1)
  if (!sm4Key) throw new Error('SM2 解密失败')
  const plain = sm4.decrypt(c.password, sm4Key)
  if (plain !== c.plain) throw new Error(`明文不一致: ${JSON.stringify(plain)} != ${JSON.stringify(c.plain)}`)
  ok++
}

console.log(`C# -> JS verified: ${ok}/${data.cases.length}`)
