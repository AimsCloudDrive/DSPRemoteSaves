import pkg from 'sm-crypto'
import { api } from './client'

const { sm2, sm4 } = pkg

export interface PublicKeyInfo {
  keyId: string
  publicKey: string
  sm2Mode: string
  sm4Mode: string
  serverTime: number
}

export interface Sealed {
  keyId: string
  smkey: string
  values: string[]
}

const CACHE_KEY = 'dsp.sm.pubkey'
const TTL = 24 * 3600 * 1000

let memory: (PublicKeyInfo & { fetchedAt: number }) | null = null

function readCache(): (PublicKeyInfo & { fetchedAt: number }) | null {
  if (memory && Date.now() - memory.fetchedAt < TTL) return memory
  try {
    const raw = sessionStorage.getItem(CACHE_KEY)
    if (!raw) return null
    const parsed = JSON.parse(raw) as PublicKeyInfo & { fetchedAt: number }
    if (!parsed.publicKey || !parsed.keyId) return null
    if (Date.now() - parsed.fetchedAt > TTL) return null
    memory = parsed
    return parsed
  } catch {
    return null
  }
}

export async function getPublicKey(force = false): Promise<PublicKeyInfo> {
  if (!force) {
    const cached = readCache()
    if (cached) return cached
  }
  const response = await api.get('/crypto/public-key')
  const body = response.data
  if (body.code !== 0 || !body.payload) {
    throw { code: body.code, messageKey: body.messageKey || 'internal', message: body.message }
  }
  const info = body.payload as PublicKeyInfo
  const withTime = { ...info, fetchedAt: Date.now() }
  memory = withTime
  sessionStorage.setItem(CACHE_KEY, JSON.stringify(withTime))
  return info
}

export async function refreshPublicKey(force = true): Promise<PublicKeyInfo> {
  sessionStorage.removeItem(CACHE_KEY)
  memory = null
  return getPublicKey(force)
}

function randomHex(bytes: number): string {
  const buffer = new Uint8Array(bytes)
  crypto.getRandomValues(buffer)
  return Array.from(buffer, (b) => b.toString(16).padStart(2, '0')).join('')
}

/**
 * 国密信封：SM2(C1C3C2) 加密 32 位 hex 的 SM4 密钥，SM4-ECB/PKCS7 加密每个字段。
 * 与后端 `sm-envelope`、C# BouncyCastle 实现逐字节对齐。
 */
export async function seal(values: string[]): Promise<Sealed> {
  const { keyId, publicKey } = await getPublicKey()
  const key = randomHex(16)
  const smkey = sm2.doEncrypt(key, publicKey, 1)
  return { keyId, smkey, values: values.map((value) => sm4.encrypt(value, key)) }
}

export function nonce(): string {
  return randomHex(16)
}

export function envelopeHeaders(sealed: Sealed): Record<string, string> {
  return { smkey: sealed.smkey, smkeyid: sealed.keyId }
}
