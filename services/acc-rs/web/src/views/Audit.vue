<template>
  <div class="dsp-page">
    <div class="dsp-page-header">
      <div>
        <h2 class="dsp-page-title">{{ t('audit.title') }}</h2>
        <div class="dsp-page-subtitle">{{ t('audit.retention') }}</div>
      </div>
      <div style="display: flex; gap: 8px">
        <el-button :loading="exporting" @click="exportCsv">{{ t('audit.export') }}</el-button>
        <el-button :loading="loading" @click="load">{{ t('common.refresh') }}</el-button>
      </div>
    </div>

    <div class="dsp-card" style="padding: 16px">
      <div class="dsp-toolbar">
        <el-input v-model="query.actor" :placeholder="t('audit.actor')" clearable style="width: 180px" @keyup.enter="reload" />
        <el-select v-model="query.action" clearable filterable :placeholder="t('audit.action')" style="width: 220px">
          <el-option :label="t('audit.allActions')" value="" />
          <el-option v-for="action in actions" :key="action" :label="action" :value="action" />
        </el-select>
        <el-select v-model="query.result" clearable :placeholder="t('audit.result')" style="width: 140px">
          <el-option :label="t('common.all')" value="" />
          <el-option :label="t('audit.success')" value="success" />
          <el-option :label="t('audit.failure')" value="failure" />
          <el-option :label="t('audit.skipped')" value="skipped" />
        </el-select>
        <el-date-picker
          v-model="range"
          type="datetimerange"
          :start-placeholder="t('audit.from')"
          :end-placeholder="t('audit.to')"
          style="width: 360px"
        />
        <el-button type="primary" @click="reload">{{ t('common.search') }}</el-button>
      </div>

      <el-table v-loading="loading" :data="rows" stripe :empty-text="t('audit.empty')">
        <el-table-column :label="t('common.time')" width="170">
          <template #default="{ row }">{{ formatTime(row.createdAt) }}</template>
        </el-table-column>
        <el-table-column prop="actorName" :label="t('audit.actor')" width="140" />
        <el-table-column prop="action" :label="t('audit.action')" min-width="190">
          <template #default="{ row }"><span class="dsp-mono">{{ row.action }}</span></template>
        </el-table-column>
        <el-table-column :label="t('audit.result')" width="100">
          <template #default="{ row }">
            <el-tag size="small" :type="row.result === 'success' ? 'success' : row.result === 'failure' ? 'danger' : 'info'">
              {{ row.result }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column :label="t('audit.target')" min-width="180">
          <template #default="{ row }">
            <span class="dsp-muted">{{ row.targetType }}</span>
            <span v-if="row.targetId" class="dsp-mono" style="margin-left: 6px">{{ shortHash(row.targetId, 8) }}</span>
          </template>
        </el-table-column>
        <el-table-column prop="ip" :label="t('audit.ip')" width="140" />
        <el-table-column :label="t('common.detail')" width="90" fixed="right">
          <template #default="{ row }">
            <el-button text type="primary" size="small" @click="detail = row">{{ t('common.detail') }}</el-button>
          </template>
        </el-table-column>
      </el-table>

      <div style="display: flex; justify-content: flex-end; margin-top: 14px">
        <el-pagination
          v-model:current-page="query.page"
          v-model:page-size="query.pageSize"
          :total="total"
          :page-sizes="[20, 50, 100]"
          layout="total, sizes, prev, pager, next"
          @change="load"
        />
      </div>
    </div>

    <el-drawer v-model="detailVisible" :title="t('common.detail')" size="460px">
      <el-descriptions :column="1" border v-if="detail">
        <el-descriptions-item :label="t('common.time')">{{ formatTime(detail.createdAt) }}</el-descriptions-item>
        <el-descriptions-item :label="t('audit.actor')">{{ detail.actorName }}</el-descriptions-item>
        <el-descriptions-item :label="t('audit.action')">{{ detail.action }}</el-descriptions-item>
        <el-descriptions-item :label="t('audit.result')">{{ detail.result }}</el-descriptions-item>
        <el-descriptions-item :label="t('audit.target')">
          {{ detail.targetType }} {{ detail.targetId || '' }}
        </el-descriptions-item>
        <el-descriptions-item :label="t('audit.ip')">{{ detail.ip }}</el-descriptions-item>
        <el-descriptions-item :label="t('audit.detail')">
          <span class="dsp-mono" style="white-space: pre-wrap">{{ detail.detail || '—' }}</span>
        </el-descriptions-item>
      </el-descriptions>
    </el-drawer>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useAuthStore } from '../stores/auth'
import { api, showError, unwrap } from '../api/client'
import { formatTime, shortHash } from '../utils/format'

interface Row {
  id: string
  actorName: string
  action: string
  targetType: string
  targetId?: string | null
  result: string
  detail?: string | null
  ip: string
  createdAt: number
}

const { t } = useI18n()
const auth = useAuthStore()
const rows = ref<Row[]>([])
const total = ref(0)
const loading = ref(false)
const exporting = ref(false)
const detail = ref<Row | null>(null)
const detailVisible = computed({
  get: () => detail.value !== null,
  set: (value: boolean) => {
    if (!value) detail.value = null
  },
})
const range = ref<[Date, Date] | null>(null)
const query = reactive({ page: 1, pageSize: 20, actor: '', action: '', result: '' })

const actions = [
  'auth.login',
  'auth.register',
  'auth.password.change',
  'auth.password.reset',
  'auth.email.change',
  'admin.login',
  'admin.user.create',
  'admin.user.update',
  'admin.user.delete',
  'admin.user.purge',
  'admin.user.reset_password',
  'admin.save.rollback',
  'admin.save.delete',
  'admin.settings.update',
  'admin.crypto.rotate',
  'admin.audit.export',
  'save.upload',
  'save.upload.user',
  'save.delete.current',
  'save.delete.all',
  'save.rollback',
  'email_code.send',
]

function params() {
  return {
    page: query.page,
    pageSize: query.pageSize,
    actor: query.actor || undefined,
    action: query.action || undefined,
    result: query.result || undefined,
    fromMs: range.value?.[0]?.getTime(),
    toMs: range.value?.[1]?.getTime(),
  }
}

async function load() {
  loading.value = true
  try {
    const data = await unwrap<{ items: Row[]; total: number }>(api.get('/admin/audit', { params: params() }))
    rows.value = data.items
    total.value = data.total
  } catch (error) {
    showError(error)
  } finally {
    loading.value = false
  }
}

function reload() {
  query.page = 1
  load()
}

async function exportCsv() {
  exporting.value = true
  try {
    const response = await api.get('/admin/audit/export.csv', {
      params: params(),
      responseType: 'blob',
    })
    const blob = new Blob([response.data], { type: 'text/csv;charset=utf-8' })
    const url = URL.createObjectURL(blob)
    const link = document.createElement('a')
    link.href = url
    link.download = `audit-${new Date().toISOString().slice(0, 10)}.csv`
    link.click()
    URL.revokeObjectURL(url)
  } catch (error) {
    showError(error)
  } finally {
    exporting.value = false
  }
}

onMounted(() => {
  if (!auth.token) return
  load()
})
</script>
