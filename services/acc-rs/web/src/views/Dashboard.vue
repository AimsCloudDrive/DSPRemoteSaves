<template>
  <div class="dsp-page">
    <div class="dsp-page-header">
      <div>
        <h2 class="dsp-page-title">{{ t('common.dashboard') }}</h2>
        <div class="dsp-page-subtitle">{{ t('common.console') }}</div>
      </div>
      <el-button :loading="loading" @click="load">{{ t('common.refresh') }}</el-button>
    </div>

    <div class="dsp-stat-grid">
      <div class="dsp-card dsp-stat">
        <div class="dsp-stat-label">{{ t('dashboard.usersTotal') }}</div>
        <div class="dsp-stat-value">{{ stats?.users.total ?? '—' }}</div>
        <div class="dsp-stat-extra">
          {{ t('dashboard.usersActive') }} {{ stats?.users.active ?? 0 }} ·
          {{ t('dashboard.usersDisabled') }} {{ stats?.users.disabled ?? 0 }}
        </div>
      </div>
      <div class="dsp-card dsp-stat">
        <div class="dsp-stat-label">{{ t('dashboard.files') }}</div>
        <div class="dsp-stat-value">{{ stats?.saves.files ?? '—' }}</div>
        <div class="dsp-stat-extra">{{ t('dashboard.versions') }} {{ stats?.saves.versions ?? 0 }}</div>
      </div>
      <div class="dsp-card dsp-stat">
        <div class="dsp-stat-label">{{ t('dashboard.storageUsed') }}</div>
        <div class="dsp-stat-value">{{ formatBytes(stats?.saves.bytes) }}</div>
        <div class="dsp-stat-extra">{{ t('dashboard.diskFree') }} {{ formatBytes(stats?.disk.freeBytes) }}</div>
      </div>
      <div class="dsp-card dsp-stat">
        <div class="dsp-stat-label">{{ t('dashboard.uptime') }}</div>
        <div class="dsp-stat-value">{{ uptime }}</div>
        <div class="dsp-stat-extra dsp-mono">{{ stats?.activeKeyId || '—' }}</div>
      </div>
    </div>

    <el-alert
      v-if="stats && !stats.backup.enabled"
      type="info"
      show-icon
      :closable="false"
      style="margin-top: 16px"
      :title="t('settings.backupDisabled')"
    />
    <el-alert
      v-else-if="stats?.backup.lastError"
      type="error"
      show-icon
      :closable="false"
      style="margin-top: 16px"
      :title="`${t('dashboard.backupError')}: ${stats.backup.lastError}`"
    />
    <el-alert
      v-else-if="stats?.backup.lastOkAt"
      type="success"
      show-icon
      :closable="false"
      style="margin-top: 16px"
      :title="`${t('dashboard.backupOk')}: ${formatTime(stats.backup.lastOkAt)}`"
    />

    <div style="display: grid; grid-template-columns: 2fr 1fr; gap: 16px; margin-top: 20px" class="dash-grid">
      <div class="dsp-card" style="padding: 18px 20px">
        <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 10px">
          <strong>{{ t('dashboard.recentAudit') }}</strong>
          <el-button text type="primary" @click="router.push('/audit')">{{ t('dashboard.goAudit') }}</el-button>
        </div>
        <el-table :data="stats?.recentAudit || []" size="small" :empty-text="t('common.empty')">
          <el-table-column prop="actorName" :label="t('audit.actor')" width="130" />
          <el-table-column prop="action" :label="t('audit.action')" min-width="170">
            <template #default="{ row }">
              <span class="dsp-mono">{{ row.action }}</span>
            </template>
          </el-table-column>
          <el-table-column :label="t('audit.result')" width="90">
            <template #default="{ row }">
              <el-tag size="small" :type="row.result === 'success' ? 'success' : row.result === 'failure' ? 'danger' : 'info'">
                {{ row.result }}
              </el-tag>
            </template>
          </el-table-column>
          <el-table-column :label="t('common.time')" width="170">
            <template #default="{ row }">{{ formatTime(row.createdAt) }}</template>
          </el-table-column>
        </el-table>
      </div>

      <div class="dsp-card" style="padding: 18px 20px">
        <strong>{{ t('dashboard.quickActions') }}</strong>
        <div style="display: grid; gap: 10px; margin-top: 14px">
          <el-button @click="router.push('/users')">{{ t('dashboard.goUsers') }}</el-button>
          <el-button @click="router.push('/saves')">{{ t('dashboard.goSaves') }}</el-button>
          <el-button @click="router.push('/settings')">{{ t('common.settings') }}</el-button>
        </div>
        <el-divider />
        <div class="dsp-muted" style="font-size: 12px; line-height: 1.8">
          <div>{{ t('audit.retention') }}</div>
          <div>{{ t('settings.lastBackup') }}: {{ formatTime(stats?.backup.lastOkAt) }}</div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { api, showError, unwrap } from '../api/client'
import { formatBytes, formatTime } from '../utils/format'

interface Overview {
  users: { total: number; active: number; disabled: number }
  saves: { files: number; versions: number; bytes: number }
  disk: { freeBytes?: number; minFreeBytes: number }
  backup: {
    enabled: boolean
    running: boolean
    lastRunAt?: number
    lastOkAt?: number
    lastError?: string
  }
  recentAudit: Array<{ id: string; actorName: string; action: string; result: string; createdAt: number }>
  activeKeyId: string
  serverTime: number
  uptimeSeconds: number
}

const { t } = useI18n()
const router = useRouter()
const stats = ref<Overview | null>(null)
const loading = ref(false)

const uptime = computed(() => {
  if (!stats.value) return '—'
  const seconds = stats.value.uptimeSeconds ?? 0
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes} min`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} h ${minutes % 60} min`
  return `${Math.floor(hours / 24)} d ${hours % 24} h`
})

async function load() {
  loading.value = true
  try {
    stats.value = await unwrap<Overview>(api.get('/admin/stats/overview'))
  } catch (error) {
    showError(error)
  } finally {
    loading.value = false
  }
}

onMounted(load)
</script>

<style scoped>
@media (max-width: 1024px) {
  .dash-grid {
    grid-template-columns: 1fr !important;
  }
}
</style>
