<template>
  <el-button :disabled="counting || disabled" :loading="sending" @click="send">
    {{ counting ? t('auth.resendIn', { n: remaining }) : t('auth.sendCode') }}
  </el-button>
</template>

<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { ElMessage } from 'element-plus'
import { api, showError } from '../api/client'

const props = defineProps<{
  email: string
  purpose: 'register' | 'reset' | 'change_email'
  disabled?: boolean
}>()

const emit = defineEmits<{ (e: 'sent'): void }>()

const { t } = useI18n()
const sending = ref(false)
const counting = ref(false)
const remaining = ref(0)
let timer: number | undefined

function start(seconds: number) {
  counting.value = true
  remaining.value = Math.max(1, seconds)
  window.clearInterval(timer)
  timer = window.setInterval(() => {
    remaining.value -= 1
    if (remaining.value <= 0) {
      counting.value = false
      window.clearInterval(timer)
    }
  }, 1000)
}

async function send() {
  if (!props.email) {
    ElMessage.warning(t('auth.emailInvalid'))
    return
  }
  sending.value = true
  try {
    const response = await api.post('/auth/email-code', {
      email: props.email,
      purpose: props.purpose,
    })
    if (response.data.code !== 0) {
      throw {
        code: response.data.code,
        messageKey: response.data.messageKey,
        message: response.data.message,
        payload: response.data.payload,
      }
    }
    const retry = (response.data.payload?.retryAfterSeconds as number) || 60
    ElMessage.success(t('auth.codeSent'))
    start(retry)
    emit('sent')
  } catch (error) {
    const payload = (error as { payload?: { retryAfterSeconds?: number } }).payload
    if (payload?.retryAfterSeconds) start(payload.retryAfterSeconds)
    showError(error)
  } finally {
    sending.value = false
  }
}

onBeforeUnmount(() => window.clearInterval(timer))
</script>
