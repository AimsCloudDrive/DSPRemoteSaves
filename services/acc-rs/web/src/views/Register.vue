<template>
  <div class="dsp-auth">
    <div class="dsp-auth-card">
      <h1 class="dsp-auth-title">{{ t('auth.registerTitle') }}</h1>
      <div class="dsp-auth-subtitle">{{ t('auth.registerSubtitle') }}</div>

      <el-form label-position="top" @submit.prevent="submit">
        <el-form-item :label="t('auth.username')">
          <el-input v-model="userName" size="large" :placeholder="t('auth.usernamePlaceholder')" />
        </el-form-item>
        <el-form-item :label="t('auth.email')">
          <el-input v-model="email" size="large" :placeholder="t('auth.emailPlaceholder')" />
        </el-form-item>
        <el-form-item :label="t('auth.code')">
          <div style="display: flex; gap: 10px; width: 100%">
            <el-input v-model="code" size="large" maxlength="6" :placeholder="t('auth.codePlaceholder')" />
            <EmailCodeButton :email="email" purpose="register" />
          </div>
        </el-form-item>
        <el-form-item :label="t('auth.password')">
          <el-input v-model="password" type="password" size="large" show-password autocomplete="new-password" />
          <PasswordChecklist :password="password" :user-name="userName" :email="email" />
        </el-form-item>
        <el-form-item :label="t('auth.confirmPassword')">
          <el-input v-model="confirm" type="password" size="large" show-password autocomplete="new-password" />
        </el-form-item>

        <el-alert v-if="errorText" :title="errorText" type="error" show-icon :closable="false" style="margin-bottom: 14px" />

        <el-button type="primary" size="large" style="width: 100%" :loading="loading" @click="submit">
          {{ t('auth.register') }}
        </el-button>
      </el-form>

      <div style="margin-top: 16px; font-size: 13px; text-align: center">
        {{ t('auth.hasAccount') }}
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
import { useAuthStore } from '../stores/auth'
import i18n from '../i18n'

const { t } = useI18n()
const router = useRouter()
const auth = useAuthStore()

const userName = ref('')
const email = ref('')
const code = ref('')
const password = ref('')
const confirm = ref('')
const loading = ref(false)
const errorText = ref('')

async function submit() {
  if (loading.value) return
  errorText.value = ''
  if (!userName.value.trim() || !email.value || !code.value || !password.value) {
    errorText.value = t('common.required')
    return
  }
  if (password.value !== confirm.value) {
    errorText.value = t('auth.passwordMismatch')
    return
  }
  loading.value = true
  try {
    await auth.register({
      userName: userName.value.trim(),
      email: email.value.trim(),
      code: code.value.trim(),
      password: password.value,
    })
    ElMessage.success(t('auth.registerSuccess'))
    router.push('/my-saves')
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
