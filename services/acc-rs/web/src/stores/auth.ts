import { defineStore } from 'pinia'
import axios from 'axios'
import { api, apiBase } from '../api/client'
import { envelopeHeaders, nonce, seal } from '../api/crypto'

export interface CurrentUser {
  id: string
  userName: string
  email?: string | null
  emailVerified: boolean
  role: 'admin' | 'user'
  status: string
  quotaBytes: number
  usedBytes: number
  reservedBytes: number
  mustChangePassword: boolean
  locale: string
  createdAt: number
  lastLoginAt?: number | null
}

export type Audience = 'user' | 'admin'

const TOKEN_KEY = 'dsp.token'
const USER_KEY = 'dsp.user'
const AUD_KEY = 'dsp.audience'

function loadUser(): CurrentUser | null {
  try {
    const raw = localStorage.getItem(USER_KEY)
    return raw ? (JSON.parse(raw) as CurrentUser) : null
  } catch {
    return null
  }
}

export const useAuthStore = defineStore('auth', {
  state: () => ({
    token: localStorage.getItem(TOKEN_KEY) || '',
    audience: (localStorage.getItem(AUD_KEY) as Audience) || 'user',
    user: loadUser(),
  }),
  getters: {
    isLoggedIn: (state) => Boolean(state.token),
    isAdmin: (state) => state.user?.role === 'admin',
    /// 是否持有管理端受众的 token（控制台接口只认这个）
    canAdmin: (state) => state.user?.role === 'admin' && state.audience === 'admin',
  },
  actions: {
    persist() {
      if (this.token) {
        localStorage.setItem(TOKEN_KEY, this.token)
        localStorage.setItem(AUD_KEY, this.audience)
        localStorage.setItem(USER_KEY, JSON.stringify(this.user))
      } else {
        localStorage.removeItem(TOKEN_KEY)
        localStorage.removeItem(AUD_KEY)
        localStorage.removeItem(USER_KEY)
      }
    },
    applySession(payload: { token: string; user: CurrentUser }, audience: Audience) {
      this.token = payload.token
      this.user = payload.user
      this.audience = audience
      this.persist()
    },
    async login(userName: string, password: string, admin = false) {
      const sealed = await seal([password])
      const url = admin ? '/admin/auth/login' : '/auth/login'
      const response = await api.post(
        url,
        {
          userName,
          password: sealed.values[0],
          timestamp: Date.now(),
          nonce: nonce(),
        },
        { headers: envelopeHeaders(sealed) },
      )
      if (response.data.code !== 0) {
        throw {
          code: response.data.code,
          messageKey: response.data.messageKey,
          message: response.data.message,
        }
      }
      this.applySession(response.data.payload, admin ? 'admin' : 'user')
      return response.data.payload.user as CurrentUser
    },
    async register(input: {
      userName: string
      email: string
      code: string
      password: string
    }) {
      const sealed = await seal([input.password])
      const response = await api.post(
        '/auth/register',
        {
          userName: input.userName,
          email: input.email,
          code: input.code,
          password: sealed.values[0],
          timestamp: Date.now(),
          nonce: nonce(),
        },
        { headers: envelopeHeaders(sealed) },
      )
      if (response.data.code !== 0) {
        throw {
          code: response.data.code,
          messageKey: response.data.messageKey,
          message: response.data.message,
        }
      }
      this.applySession(response.data.payload, 'user')
    },
    async refresh(): Promise<boolean> {
      try {
        const url = this.audience === 'admin' ? '/admin/auth/refresh' : '/auth/refresh'
        const response = await axios.post(
          `${apiBase()}${url}`,
          {},
          { withCredentials: true, timeout: 15_000 },
        )
        if (response.data?.code === 0) {
          this.applySession(response.data.payload, this.audience)
          return true
        }
        return false
      } catch {
        this.clear()
        return false
      }
    },
    async fetchMe() {
      if (!this.token) return null
      try {
        const response = await api.get('/auth/me')
        if (response.data.code === 0) {
          this.user = response.data.payload as CurrentUser
          this.persist()
          return this.user
        }
      } catch {
        /* 拦截器已处理 */
      }
      return this.user
    },
    async setLocale(locale: 'zh-CN' | 'en-US') {
      await api.patch('/auth/me', { locale })
      if (this.user) {
        this.user.locale = locale
        this.persist()
      }
    },
    async logout() {
      const url = this.audience === 'admin' ? '/admin/auth/logout' : '/auth/logout'
      try {
        await api.post(url, {})
      } catch {
        /* 即使失败也要清本地状态 */
      }
      this.clear()
    },
    clear() {
      this.token = ''
      this.user = null
      this.audience = 'user'
      this.persist()
    },
  },
})
