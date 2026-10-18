<template>
  <div class="dsp-page">
    <div class="dsp-page-header">
      <div>
        <h2 class="dsp-page-title">{{ t('settings.title') }}</h2>
        <div class="dsp-page-subtitle">{{ t('common.console') }}</div>
      </div>
      <el-button :loading="loading" @click="load">{{ t('common.refresh') }}</el-button>
    </div>

    <div style="display: grid; grid-template-columns: 1.4fr 1fr; gap: 16px" class="settings-grid">
      <div class="dsp-card" style="padding: 20px">
        <strong>{{ t('settings.title') }}</strong>
        <el-form label-position="top" style="margin-top: 14px">
          <el-form-item :label="`${t('settings.defaultQuota')} (GB)`">
            <el-input-number v-model="form.quotaGb" :min="0.1" :step="0.5" :precision="2" style="width: 220px" />
          </el-form-item>
          <el-form-item :label="`${t('settings.maxFileSize')} (GB)`">
            <el-input-number v-model="form.maxFileGb" :min="1" :max="10" :step="1" style="width: 220px" />
          </el-form-item>
          <el-form-item :label="t('settings.allowedExtensions')">
            <el-input v-model="form.extensions" :placeholder="t('settings.extensionsHint')" />
          </el-form-item>
          <el-button type="primary" :loading="saving" @click="save">{{ t('settings.save') }}</el-button>
        </el-form>
      </div>

      <div style="display: grid; gap: 16px; align-content: start">
        <div class="dsp-card" style="padding: 20px">
          <strong>{{ t('settings.crypto') }}</strong>
          <div class="dsp-muted" style="font-size: 12px; margin-top: 8px">{{ t('settings.keyId') }}</div>
          <div class="dsp-mono" style="margin: 4px 0 12px">{{ overview?.activeKeyId || '—' }}</div>
          <el-button type="warning" plain :loading="rotating" @click="rotate">{{ t('settings.rotate') }}</el-button>
        </div>

        <div class="dsp-card" style="padding: 20px">
          <strong>{{ t('settings.backup') }}</strong>
          <div style="margin-top: 10px">
            <el-tag :type="overview?.backup.enabled ? 'success' : 'info'">
              {{ overview?.backup.enabled ? t('settings.backupEnabled') : t('settings.backupDisabled') }}
            </el-tag>
          </div>
          <div class="dsp-muted" style="font-size: 12px; margin-top: 10px">
            {{ t('settings.lastBackup') }}: {{ formatTime(overview?.backup.lastOkAt) }}
          </div>
          <div v-if="overview?.backup.lastError" class="dsp-muted" style="font-size: 12px; color: #dc2626">
            {{ overview.backup.lastError }}
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { ElMessage, ElMessageBox } from 'element-plus'
import { api, showError, unwrap } from '../api/client'
import { formatTime } from '../utils/format'

interface Settings {
  defaultQuotaBytes: number
  maxFileSize: number
  allowedExtensions: string[]
}

interface Overview {
  activeKeyId: string
  backup: { enabled: boolean; lastOkAt?: number; lastError?: string }
}

const { t } = useI18n()
const loading = ref(false)
const saving = ref(false)
const rotating = ref(false)
const overview = ref<Overview | null>(null)
const form = reactive({ quotaGb: 1, maxFileGb: 2, extensions: '.dsv,.moddsv,.server' })

async function load() {
  loading.value = true
  try {
    const [settings, stats] = await Promise.all([
      unwrap<Settings>(api.get('/admin/settings')),
      unwrap<Overview>(api.get('/admin/stats/overview')),
    ])
    form.quotaGb = Number((settings.defaultQuotaBytes / 1024 ** 3).toFixed(2))
    form.maxFileGb = Number((settings.maxFileSize / 1024 ** 3).toFixed(2))
    form.extensions = settings.allowedExtensions.join(',')
    overview.value = stats
  } catch (error) {
    showError(error)
  } finally {
    loading.value = false
  }
}

async function save() {
  saving.value = true
  try {
    await unwrap(
      api.put('/admin/settings', {
        defaultQuotaBytes: Math.round(form.quotaGb * 1024 ** 3),
        maxFileSize: Math.round(form.maxFileGb * 1024 ** 3),
        allowedExtensions: form.extensions
          .split(',')
          .map((item) => item.trim())
          .filter(Boolean),
      }),
    )
    ElMessage.success(t('settings.saved'))
    load()
  } catch (error) {
    showError(error)
  } finally {
    saving.value = false
  }
}

async function rotate() {
  try {
    await ElMessageBox.confirm(t('settings.rotateConfirm'), t('settings.rotate'), { type: 'warning' })
  } catch {
    return
  }
  rotating.value = true
  try {
    const data = await unwrap<{ keyId: string }>(api.post('/admin/crypto/rotate'))
    ElMessage.success(t('settings.rotatedHint', { id: data.keyId }))
    load()
  } catch (error) {
    showError(error)
  } finally {
    rotating.value = false
  }
}

onMounted(load)
</script>

<style scoped>
@media (max-width: 1024px) {
  .settings-grid {
    grid-template-columns: 1fr !important;
  }
}
</style>
