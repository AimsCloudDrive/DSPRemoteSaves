declare module 'sm-crypto' {
  export const sm2: {
    generateKeyPairHex(): { privateKey: string; publicKey: string }
    doEncrypt(msg: string | number[], publicKey: string, cipherMode?: number): string
    doDecrypt(cipher: string, privateKey: string, cipherMode?: number, options?: { output?: string }): string
    doSignature(msg: string, privateKey: string, options?: Record<string, unknown>): string
    doVerifySignature(msg: string, signHex: string, publicKey: string, options?: Record<string, unknown>): boolean
  }
  export const sm3: (input: string | number[]) => string
  export const sm4: {
    encrypt(input: string | number[], key: string, options?: Record<string, unknown>): string
    decrypt(input: string | number[], key: string, options?: Record<string, unknown>): string
  }
  const _default: { sm2: typeof sm2; sm3: typeof sm3; sm4: typeof sm4 }
  export default _default
}
