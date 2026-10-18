<template>
  <div class="dsp-page">
    <div class="dsp-page-header">
      <h2 class="dsp-page-title">{{ t('profile.title') }}</h2>
    </div>

    <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 16px" class="profile-grid">
      <div class="dsp-card" style="padding: 20px">
        <strong>{{ t('profile.account') }}</strong>
        <el-descriptions :column="1" border style="margin-top: 14px">
          <el-descriptions-item :label="t('auth.username')">{{ user?.userName }}</el-descriptions-item>
          <el-descriptions-item :label="t('profile.role')">
            <el-tag size="small" :type="user?.role === 'admin' ? 'warning' : 'info'" effect="plain">
              {{ user?.role === 'admin' ? t('users.roleAdmin') : t('users.roleUser') }}
            </el-tag>
          </el-descriptions-item>
          <el-descriptions-item :label="t('profile.email')">
            {{ user?.email || '—' }}
            <el-tag v-if="user?.email && !user.emailVerified" size="small" type="warning" style="margin-left: 6px">
              {{ t('profile.emailUnverified') }}
            </el-tag>
          </el-descriptions-item>
          <el-descriptions-item :label="t('profile.quota')">
            <div style="width: 100%">
              <div style="display: flex; justify-content: space-between; font-size: 12px">
                <span>{{ formatBytes(user?.usedBytes) }} / {{ formatBytes(user?.quotaBytes) }}</span>
                <span class="dsp-muted">{{ t('profile.reserved') }} {{ formatBytes(user?.reservedBytes) }}</span>
              </div>
              <el-progress :percentage="usagePercent" :stroke-width="8" :show-text="false" style="margin-top: 6px" />
            </div>
          </el-descriptions-item>
          <el-descriptions-item :label="t('users.createdAt')">{{ formatTime(user?.createdAt) }}</el-descriptions-item>
          <el-descriptions-item :label="t('users.lastLogin')">{{ formatTime(user?.lastLoginAt) }}</el-descriptions-item>
        </el-descriptions>

        <el-divider />
        <div style="display: flex; align-items: center; justify-content: space-between">
          <span>{{ t('profile.locale') }}</span>
          <el-radio-group :model-value="locale" @change="changeLocale">
            <el-radio-button value="zh-CN">{{ t('common.chinese') }}</el-radio-button>
            <el-radio-button value="en-US">{{ t('common.english') }}</el-radio-button>
          </el-radio-group>
        </div>
      </div>

      <div style="display: grid; gap: 16px; align-content: start">
        <div class="dsp-card" style="padding: 20px">
          <strong>{{ t('auth.changePassword') }}</strong>
          <div class="dsp-muted" style="font-size: 12px; margin: 6px 0 12px">{{ t('auth.changePasswordHint') }}</div>
          <el-form label-position="top">
            <el-form-item :label="t('auth.oldPassword')">
              <el-input v-model="pwd.old" type="password" show-password autocomplete="current-password" />
            </el-form-item>
            <el-form-item :label="t('auth.newPassword')">
              <el-input v-model="pwd.next" type="password" show-password autocomplete="new-password" />
              <PasswordChecklist :password="pwd.next" :user-name="user?.userName" :email="user?.email || ''" />
            </el-form-item>
            <el-button type="primary" :loading="savingPwd" @click="changePassword">{{ t('common.save') }}</el-button>
          </el-form>
        </div>

        <div class="dsp-card" style="padding: 20px">
          <strong>{{ t('auth.changeEmail') }}</strong>
          <div class="dsp-muted" style="font-size: 12px; margin: 6px 0 12px">{{ t('auth.changeEmailHint') }}</div>
          <el-form label-position="top">
            <el-form-item :label="t('auth.newEmail')">
              <el-input v-model="emailForm.email" :placeholder="t('auth.emailPlaceholder')" />
            </el-form-item>
            <el-form-item :label="t('auth.password')">
              <el-input v-model="emailForm.password" type="password" show-password autocomplete="current-password" />
            </el-form-item>
            <div style="display: flex; gap: 10px">
              <el-button :loading="sendingEmail" @click="requestEmailChange">{{ t('auth.sendCode') }}</el-button>
              <el-input v-model="emailForm.code" maxlength="6" :placeholder="t('auth.codePlaceholder')" style="width: 140px" />
              <el-button type="primary" :loading="savingEmail" @click="confirmEmailChange">{{ t('common.confirm') }}</el-button>
            </div>
          </el-form>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import PasswordChecklist from '../components/PasswordChecklist.vue'
import { api, showError, unwrap } from '../api/client'
import { envelopeHeaders, nonce, seal } from '../api/crypto'
import { useAuthStore } from '../stores/auth'
import { applyLocale, type Locale } from '../i18n'
import { formatBytes, formatTime } from '../utils/format'

const { t, locale } = useI18n()
const router = useRouter()
const auth = useAuthStore()

const user = computed(() => auth.user)
const usagePercent = computed(() => {
  if (!user.value?.quotaBytes) return 0
  const used = user.value.usedBytes + (user.value.reservedBytes || 0)
  return Math.min(100, Math.round((used / user.value.quotaBytes) * 100))
})

const pwd = reactive({ old: '', next: '' })
const emailForm = reactive({ email: '', password: '', code: '' })
const savingPwd = ref(false)
const sendingEmail = ref(false)
const savingEmail = ref(false)

async function changeLocale(value: string | number | boolean | undefined) {
  const next = value === 'en-US' ? 'en-US' : 'zh-CN'
  applyLocale(next as Locale)
  try {
    await auth.setLocale(next as Locale)
    ElMessage.success(t('profile.localeSaved'))
  } catch (error) {
    showError(error)
  }
}

async function changePassword() {
  if (!pwd.old || !pwd.next) {
    ElMessage.warning(t('common.required'))
    return
  }
  savingPwd.value = true
  try {
    const sealed = await seal([pwd.old, pwd.next])
    await unwrap(
      api.post(
        '/auth/password',
        {
          oldPassword: sealed.values[0],
          newPassword: sealed.values[1],
          timestamp: Date.now(),
          nonce: nonce(),
        },
        { headers: envelopeHeaders(sealed) },
      ),
    )
    ElMessage.success(t('auth.changePasswordSuccess'))
    await auth.logout()
    router.push('/login')
  } catch (error) {
    showError(error)
  } finally {
    savingPwd.value = false
  }
}

async function requestEmailChange() {
  if (!emailForm.email || !emailForm.password) {
    ElMessage.warning(t('common.required'))
    return
  }
  sendingEmail.value = true
  try {
    const sealed = await seal([emailForm.password])
    await unwrap(
      api.post(
        '/auth/email/change/request',
        {
          email: emailForm.email.trim(),
          password: sealed.values[0],
          timestamp: Date.now(),
          nonce: nonce(),
        },
        { headers: envelopeHeaders(sealed) },
      ),
    )
    ElMessage.success(t('profile.emailChangeSent'))
  } catch (error) {
    showError(error)
  } finally {
    sendingEmail.value = false
  }
}

async function confirmEmailChange() {
  if (!emailForm.code) {
    ElMessage.warning(t('auth.codeInvalid'))
    return
  }
  savingEmail.value = true
  try {
    await unwrap(api.post('/auth/email/change', { code: emailForm.code.trim() }))
    ElMessage.success(t('auth.changeEmailSuccess'))
    await auth.fetchMe()
    emailForm.code = ''
    emailForm.password = ''
  } catch (error) {
    showError(error)
  } finally {
    savingEmail.value = false
  }
}

onMounted(() => {
  auth.fetchMe()
})
</script>

<style scoped>
@media (max-width: 1024px) {
  .profile-grid {
    grid-template-columns: 1fr !important;
  }
}
</style>
