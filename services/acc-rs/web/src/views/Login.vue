<template>
  <div class="dsp-auth">
    <div class="dsp-auth-card">
      <div class="dsp-logo" style="margin-bottom: 18px">
        <div class="dsp-logo-mark">D</div>
        <span>{{ t('common.appName') }}</span>
      </div>
      <h1 class="dsp-auth-title">{{ admin ? t('auth.adminLoginTitle') : t('auth.loginTitle') }}</h1>
      <div class="dsp-auth-subtitle">
        {{ admin ? t('auth.adminLoginSubtitle') : t('auth.loginSubtitle') }}
      </div>

      <el-radio-group v-model="admin" style="margin-bottom: 18px" @change="reset">
        <el-radio-button :value="false">{{ t('common.portal') }}</el-radio-button>
        <el-radio-button :value="true">{{ t('common.console') }}</el-radio-button>
      </el-radio-group>

      <el-form label-position="top" @submit.prevent="submit">
        <el-form-item :label="t('auth.username')">
          <el-input
            v-model="userName"
            size="large"
            autocomplete="username"
            :placeholder="t('auth.usernamePlaceholder')"
            @keyup.enter="submit"
          />
        </el-form-item>
        <el-form-item :label="t('auth.password')">
          <el-input
            v-model="password"
            type="password"
            size="large"
            show-password
            autocomplete="current-password"
            :placeholder="t('auth.passwordPlaceholder')"
            @keyup.enter="submit"
          />
        </el-form-item>

        <el-alert v-if="errorText" :title="errorText" type="error" show-icon :closable="false" style="margin-bottom: 14px" />

        <el-button type="primary" size="large" style="width: 100%" :loading="loading" @click="submit">
          {{ t('auth.login') }}
        </el-button>
      </el-form>

      <div style="display: flex; justify-content: space-between; margin-top: 16px; font-size: 13px">
        <router-link to="/register">{{ t('common.portal') }} · {{ t('auth.register') }}</router-link>
        <router-link to="/reset">{{ t('auth.forgot') }}</router-link>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { useAuthStore } from '../stores/auth'
import i18n from '../i18n'

const { t } = useI18n()
const router = useRouter()
const route = useRoute()
const auth = useAuthStore()

const admin = ref(route.query.admin === '1')
const userName = ref('')
const password = ref('')
const loading = ref(false)
const errorText = ref('')

function reset() {
  errorText.value = ''
}

async function submit() {
  if (loading.value) return
  errorText.value = ''
  if (!userName.value.trim() || !password.value) {
    errorText.value = t('common.required')
    return
  }
  loading.value = true
  try {
    const user = await auth.login(userName.value.trim(), password.value, admin.value)
    ElMessage.success(t('auth.loginSuccess'))
    if (user.mustChangePassword) {
      router.push('/profile')
      return
    }
    const redirect = route.query.redirect as string | undefined
    if (redirect) {
      router.push(redirect)
    } else {
      router.push(admin.value ? '/' : '/my-saves')
    }
  } catch (error) {
    const key = (error as { messageKey?: string }).messageKey
    errorText.value = key ? i18n.global.t(`errors.${key}`) : (error as Error).message || t('common.failed')
  } finally {
    loading.value = false
  }
}
</script>
