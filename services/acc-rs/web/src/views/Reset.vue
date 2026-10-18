<template>
  <div class="dsp-auth">
    <div class="dsp-auth-card">
      <h1 class="dsp-auth-title">{{ t('auth.resetTitle') }}</h1>
      <div class="dsp-auth-subtitle">{{ t('auth.resetSubtitle') }}</div>

      <el-form label-position="top" @submit.prevent="submit">
        <el-form-item :label="t('auth.email')">
          <el-input v-model="email" size="large" :placeholder="t('auth.emailPlaceholder')" />
        </el-form-item>
        <el-form-item :label="t('auth.code')">
          <div style="display: flex; gap: 10px; width: 100%">
            <el-input v-model="code" size="large" maxlength="6" :placeholder="t('auth.codePlaceholder')" />
            <EmailCodeButton :email="email" purpose="reset" />
          </div>
        </el-form-item>
        <el-form-item :label="t('auth.newPassword')">
          <el-input v-model="password" type="password" size="large" show-password autocomplete="new-password" />
          <PasswordChecklist :password="password" :email="email" />
        </el-form-item>

        <el-alert v-if="errorText" :title="errorText" type="error" show-icon :closable="false" style="margin-bottom: 14px" />

        <el-button type="primary" size="large" style="width: 100%" :loading="loading" @click="submit">
          {{ t('auth.resetSubmit') }}
        </el-button>
      </el-form>

      <div style="margin-top: 16px; font-size: 13px; text-align: center">
        <router-link to="/login">{{ t('auth.login') }}</router-link>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import EmailCodeButton from '../components/EmailCodeButton.vue'
import PasswordChecklist from '../components/PasswordChecklist.vue'
import { api } from '../api/client'
import { envelopeHeaders, nonce, seal } from '../api/crypto'
import i18n from '../i18n'

const { t } = useI18n()
const router = useRouter()

const email = ref('')
const code = ref('')
const password = ref('')
const loading = ref(false)
const errorText = ref('')

async function submit() {
  if (loading.value) return
  errorText.value = ''
  if (!email.value || !code.value || !password.value) {
    errorText.value = t('common.required')
    return
  }
  loading.value = true
  try {
    const sealed = await seal([password.value])
    const response = await api.post(
      '/auth/password/reset',
      {
        email: email.value.trim(),
        code: code.value.trim(),
        password: sealed.values[0],
        timestamp: Date.now(),
        nonce: nonce(),
      },
      { headers: envelopeHeaders(sealed) },
    )
    if (response.data.code !== 0) {
      throw { messageKey: response.data.messageKey, payload: response.data.payload }
    }
    ElMessage.success(t('auth.resetSuccess'))
    router.push('/login')
  } catch (error) {
    const info = error as { messageKey?: string; payload?: { missing?: string[] } }
    if (info.messageKey === 'password_weak' && info.payload?.missing?.length) {
      errorText.value = info.payload.missing.map((key) => t(`pwd.${key}`)).join('；')
    } else {
      errorText.value = info.messageKey
        ? i18n.global.t(`errors.${info.messageKey}`)
        : (error as Error).message
    }
  } finally {
    loading.value = false
  }
}
</script>
