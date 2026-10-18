<template>
  <ul class="dsp-checklist">
    <li v-for="item in items" :key="item.key" :class="item.ok ? 'ok' : 'bad'">
      <el-icon v-if="item.ok"><CircleCheck /></el-icon>
      <el-icon v-else><CircleClose /></el-icon>
      <span>{{ t(`pwd.${item.key}`) }}</span>
    </li>
  </ul>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { CircleCheck, CircleClose } from '@element-plus/icons-vue'

const props = defineProps<{
  password: string
  userName?: string
  email?: string
}>()

const { t } = useI18n()

const COMMON = new Set([
  'password123',
  'password1234',
  '1234567890',
  'qwerty123',
  'abcd1234',
  'admin123',
  '1qaz2wsx3edc',
])

const items = computed(() => {
  const pw = props.password || ''
  const length = pw.length >= 10 && pw.length <= 64
  const letter = /[A-Za-z]/.test(pw)
  const digit = /\d/.test(pw)
  const special = /[!-/:-@[-`{-~]/.test(pw)
  const lower = pw.toLowerCase()
  const user = (props.userName || '').toLowerCase()
  const local = (props.email || '').split('@')[0].toLowerCase()
  const personal =
    pw.length > 0 &&
    !(user && lower.includes(user)) &&
    !(local && lower.includes(local))
  const common = pw.length > 0 && !COMMON.has(lower)

  return [
    { key: 'length', ok: length },
    { key: 'letter', ok: letter },
    { key: 'digit', ok: digit },
    { key: 'special', ok: special },
    { key: 'personal', ok: personal },
    { key: 'common', ok: common },
  ]
})

defineExpose({ allOk: computed(() => items.value.every((item) => item.ok)) })
</script>
