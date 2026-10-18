// 校验 zh-CN 与 en-US 的 key 集合完全一致（CI 用）
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))

function flatten(obj, prefix = '') {
  const out = []
  for (const [key, value] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${key}` : key
    if (value && typeof value === 'object') out.push(...flatten(value, path))
    else out.push(path)
  }
  return out
}

function load(name) {
  const text = readFileSync(resolve(here, `../src/i18n/${name}.ts`), 'utf8')
  const json = text.replace(/^export default/m, 'export default')
  // 用动态 import 需要编译，这里直接做轻量转换：去掉类型与注释后 eval
  const body = text.replace(/^export default\s*/, '')
  // eslint-disable-next-line no-new-func
  const value = new Function(`return (${body.replace(/;\s*$/, '')})`)()
  void json
  return value
}

const zh = flatten(load('zh-CN'))
const en = flatten(load('en-US'))

const missingInEn = zh.filter((key) => !en.includes(key))
const missingInZh = en.filter((key) => !zh.includes(key))

if (missingInEn.length || missingInZh.length) {
  console.error('i18n key mismatch')
  if (missingInEn.length) console.error('missing in en-US:', missingInEn)
  if (missingInZh.length) console.error('missing in zh-CN:', missingInZh)
  process.exit(1)
}

console.log(`i18n OK: ${zh.length} keys in both locales`)
