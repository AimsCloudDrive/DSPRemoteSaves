import axios, { type AxiosError, type AxiosRequestConfig } from 'axios'
import { ElMessage } from 'element-plus'
import i18n from '../i18n'
import { useAuthStore } from '../stores/auth'
import { refreshPublicKey } from './crypto'

export interface ApiEnvelope<T = unknown> {
  code: number
  messageKey?: string
  message?: string
  payload?: T
  traceId?: string
  detail?: string
}

export interface ApiErrorInfo {
  code: number
  messageKey: string
  message: string
  payload?: Record<string, unknown>
  status?: number
}

const baseURL = import.meta.env.VITE_API_BASE || '/dsp.saves/api'

export const api = axios.create({
  baseURL,
  withCredentials: true,
  timeout: 30_000,
})

api.interceptors.request.use((config) => {
  const auth = useAuthStore()
  if (auth.token) {
    config.headers = config.headers ?? {}
    config.headers.Authorization = `Bearer ${auth.token}`
  }
  return config
})

function normalizeError(error: AxiosError<ApiEnvelope>): ApiErrorInfo {
  const data = error.response?.data
  if (data && typeof data === 'object' && 'code' in data) {
    return {
      code: data.code,
      messageKey: data.messageKey || 'internal',
      message: data.message || i18n.global.t(`errors.${data.messageKey || 'internal'}`),
      payload: data.payload as Record<string, unknown> | undefined,
      status: error.response?.status,
    }
  }
  return {
    code: -1,
    messageKey: 'network',
    message: i18n.global.t('errors.network'),
    status: error.response?.status,
  }
}

let refreshing: Promise<boolean> | null = null

api.interceptors.response.use(
  (response) => response,
  async (error: AxiosError<ApiEnvelope>) => {
    const status = error.response?.status
    const code = error.response?.data?.code
    const config = error.config as (AxiosRequestConfig & { _retry?: boolean }) | undefined
    const url = config?.url || ''

    // access token 过期：刷新一次后重放
    if (
      config &&
      !config._retry &&
      (status === 401 || code === 1002) &&
      !url.includes('/auth/login') &&
      !url.includes('/auth/refresh')
    ) {
      config._retry = true
      refreshing = refreshing ?? useAuthStore().refresh()
      const ok = await refreshing.finally(() => {
        refreshing = null
      })
      if (ok) {
        return api.request(config)
      }
    }

    // 服务端密钥轮换：刷新公钥，提示用户重试
    if (code === 2005) {
      await refreshPublicKey(true)
    }

    return Promise.reject(normalizeError(error))
  },
)

export async function unwrap<T>(promise: Promise<{ data: ApiEnvelope<T> }>): Promise<T> {
  const response = await promise
  const body = response.data
  if (body.code !== 0) {
    throw {
      code: body.code,
      messageKey: body.messageKey || 'internal',
      message: body.message || i18n.global.t(`errors.${body.messageKey || 'internal'}`),
      payload: body.payload as Record<string, unknown> | undefined,
    } satisfies ApiErrorInfo
  }
  return body.payload as T
}

export function showError(error: unknown) {
  const info = error as ApiErrorInfo
  const key = info?.messageKey
  const text = key ? i18n.global.t(`errors.${key}`) : info?.message || i18n.global.t('common.failed')
  ElMessage.error(text)
}

export function errorPayload<T = Record<string, unknown>>(error: unknown): T | undefined {
  return (error as ApiErrorInfo)?.payload as T | undefined
}

export function apiBase(): string {
  return baseURL
}
