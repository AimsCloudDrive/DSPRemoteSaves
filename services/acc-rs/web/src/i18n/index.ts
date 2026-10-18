import { createI18n } from 'vue-i18n'
import dayjs from 'dayjs'
import 'dayjs/locale/zh-cn'
import zhCN from './zh-CN'
import enUS from './en-US'

export type Locale = 'zh-CN' | 'en-US'

const STORAGE_KEY = 'dsp.locale'

export function storedLocale(): Locale {
  const value = localStorage.getItem(STORAGE_KEY)
  if (value === 'en-US' || value === 'zh-CN') return value
  return navigator.language.toLowerCase().startsWith('en') ? 'en-US' : 'zh-CN'
}

export function persistLocale(locale: Locale) {
  localStorage.setItem(STORAGE_KEY, locale)
}

export const i18n = createI18n({
  legacy: false,
  locale: storedLocale(),
  fallbackLocale: 'zh-CN',
  messages: { 'zh-CN': zhCN, 'en-US': enUS },
  missingWarn: import.meta.env.DEV,
  fallbackWarn: false,
})

export function applyLocale(locale: Locale) {
  i18n.global.locale.value = locale
  persistLocale(locale)
  document.documentElement.lang = locale
  dayjs.locale(locale === 'zh-CN' ? 'zh-cn' : 'en')
}

applyLocale(storedLocale())

export default i18n
