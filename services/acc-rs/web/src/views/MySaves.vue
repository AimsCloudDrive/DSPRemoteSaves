<template>
  <div class="dsp-page">
    <div class="dsp-page-header">
      <div>
        <h2 class="dsp-page-title">{{ t('saves.myTitle') }}</h2>
        <div class="dsp-page-subtitle">
          {{ t('users.used') }} {{ formatBytes(data?.usedBytes) }} / {{ formatBytes(data?.quotaBytes) }}
          · {{ t('users.reserved') }} {{ formatBytes(data?.reservedBytes) }}
        </div>
      </div>
      <div style="display: flex; gap: 8px">
        <el-button :loading="loading" @click="load">{{ t('common.refresh') }}</el-button>
        <el-button type="primary" :loading="uploading" @click="pickFile">{{ t('saves.upload') }}</el-button>
      </div>
    </div>

    <el-alert
      v-if="uploading"
      type="info"
      :closable="false"
      show-icon
      :title="t('saves.uploadProgress', { n: progress })"
      style="margin-bottom: 14px"
    />
    <el-alert v-else type="info" :closable="false" :title="t('saves.uploadHint')" style="margin-bottom: 14px" />

    <div class="dsp-card" style="padding: 16px">
      <el-table v-loading="loading" :data="data?.files || []" stripe :empty-text="t('saves.emptyMine')">
        <el-table-column prop="fileName" :label="t('saves.fileName')" min-width="240" />
        <el-table-column :label="t('saves.size')" width="120">
          <template #default="{ row }">{{ formatBytes(row.sizeBytes) }}</template>
        </el-table-column>
        <el-table-column :label="t('saves.current')" width="100">
          <template #default="{ row }">#{{ row.currentSeq }}</template>
        </el-table-column>
        <el-table-column :label="t('saves.mtime')" width="180">
          <template #default="{ row }">{{ formatTime(row.mtimeMs) }}</template>
        </el-table-column>
        <el-table-column :label="t('common.actions')" width="260" fixed="right">
          <template #default="{ row }">
            <el-button text type="primary" size="small" @click="downloadCurrent(row)">{{ t('saves.download') }}</el-button>
            <el-button text size="small" @click="openVersions(row)">{{ t('saves.versions') }}</el-button>
            <el-button text type="danger" size="small" @click="openDelete(row)">{{ t('common.delete') }}</el-button>
          </template>
        </el-table-column>
      </el-table>
    </div>

    <el-drawer v-model="drawerVisible" :title="`${t('saves.versions')} · ${currentFile?.fileName || ''}`" size="520px">
      <div v-loading="versionsLoading">
        <div v-if="versions.length === 0" class="dsp-muted" style="padding: 24px 0">{{ t('saves.noVersions') }}</div>
        <div v-for="version in versions" :key="version.id" class="dsp-version-row">
          <div>
            <div style="font-weight: 600">
              #{{ version.seq }}
              <el-tag v-if="version.current" size="small" type="success" style="margin-left: 6px">
                {{ t('saves.current') }}
              </el-tag>
            </div>
            <div class="dsp-muted" style="font-size: 12px">
              {{ formatTime(version.createdAt) }} · {{ formatBytes(version.sizeBytes) }} ·
              {{ sourceLabel(version.source) }}
            </div>
            <div class="dsp-mono dsp-muted" style="font-size: 11px">{{ shortHash(version.sm3, 16) }}</div>
          </div>
          <div style="display: flex; gap: 6px">
            <el-button size="small" @click="downloadVersion(version)">{{ t('saves.download') }}</el-button>
            <el-button size="small" :disabled="version.current" @click="rollback(version)">
              {{ t('saves.rollback') }}
            </el-button>
          </div>
        </div>
      </div>
    </el-drawer>

    <el-dialog v-model="deleteVisible" :title="t('saves.deleteTitle')" width="480px">
      <div style="margin-bottom: 12px"><strong>{{ deleting?.fileName }}</strong></div>
      <el-alert type="warning" show-icon :closable="false" :title="t('saves.deleteCurrentWarning')" />
      <el-checkbox v-model="deleteAll" style="margin-top: 14px">{{ t('saves.deleteAll') }}</el-checkbox>
      <el-alert v-if="deleteAll" type="error" show-icon :closable="false" :title="t('saves.deleteAllWarning')" style="margin-top: 10px" />
      <template #footer>
        <el-button @click="deleteVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="danger" :loading="saving" @click="submitDelete">{{ t('common.delete') }}</el-button>
      </template>
    </el-dialog>

    <input ref="fileInput" type="file" style="display: none" @change="onFilePicked" />
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { ElMessage, ElMessageBox } from 'element-plus'
import { api, apiBase, showError, unwrap } from '../api/client'
import { useAuthStore } from '../stores/auth'
import { formatBytes, formatTime, shortHash } from '../utils/format'

interface SaveRow {
  id: string
  fileName: string
  sizeBytes: number
  mtimeMs: number
  currentSeq: number
  currentVersionId?: string
  sm3?: string
}

interface SaveList {
  files: SaveRow[]
  totalBytes: number
  quotaBytes: number
  usedBytes: number
  reservedBytes: number
}

interface Version {
  id: string
  seq: number
  sizeBytes: number
  sm3?: string
  createdAt: number
  source: string
  current: boolean
}

const { t } = useI18n()
const auth = useAuthStore()
const data = ref<SaveList | null>(null)
const loading = ref(false)
const uploading = ref(false)
const progress = ref(0)
const fileInput = ref<HTMLInputElement | null>(null)

const drawerVisible = ref(false)
const versionsLoading = ref(false)
const versions = ref<Version[]>([])
const currentFile = ref<SaveRow | null>(null)

const deleteVisible = ref(false)
const deleteAll = ref(false)
const deleting = ref<SaveRow | null>(null)
const saving = ref(false)

function sourceLabel(source: string) {
  if (source === 'mod') return t('saves.sourceMod')
  if (source === 'admin') return t('saves.sourceAdmin')
  return t('saves.sourceUser')
}

async function load() {
  loading.value = true
  try {
    data.value = await unwrap<SaveList>(api.get('/user/saves'))
  } catch (error) {
    showError(error)
  } finally {
    loading.value = false
  }
}

function pickFile() {
  fileInput.value?.click()
}

async function onFilePicked(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  uploading.value = true
  progress.value = 0
  try {
    await api.put(`/user/saves/${encodeURIComponent(file.name)}`, file, {
      headers: {
        'Content-Type': 'application/octet-stream',
        Authorization: `Bearer ${auth.token}`,
      },
      timeout: 0,
      onUploadProgress: (event) => {
        if (event.total) progress.value = Math.round((event.loaded / event.total) * 100)
      },
    })
    ElMessage.success(t('saves.uploadSuccess'))
    load()
  } catch (error) {
    showError(error)
  } finally {
    uploading.value = false
    progress.value = 0
  }
}

async function downloadBlob(url: string, filename: string) {
  const response = await api.get(url, { responseType: 'blob' })
  const blob = new Blob([response.data], { type: 'application/octet-stream' })
  const objectUrl = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = objectUrl
  link.download = filename
  link.click()
  URL.revokeObjectURL(objectUrl)
}

async function downloadCurrent(row: SaveRow) {
  if (!row.currentVersionId) return
  try {
    await downloadBlob(
      `/user/saves/${encodeURIComponent(row.fileName)}/versions/${row.currentVersionId}/download`,
      row.fileName,
    )
  } catch (error) {
    showError(error)
  }
}

async function openVersions(row: SaveRow) {
  currentFile.value = row
  drawerVisible.value = true
  versionsLoading.value = true
  try {
    const result = await unwrap<{ versions: Version[] }>(
      api.get(`/user/saves/${encodeURIComponent(row.fileName)}/versions`),
    )
    versions.value = result.versions
  } catch (error) {
    showError(error)
  } finally {
    versionsLoading.value = false
  }
}

async function downloadVersion(version: Version) {
  if (!currentFile.value) return
  try {
    await downloadBlob(
      `/user/saves/${encodeURIComponent(currentFile.value.fileName)}/versions/${version.id}/download`,
      `${version.seq}_${currentFile.value.fileName}`,
    )
  } catch (error) {
    showError(error)
  }
}

async function rollback(version: Version) {
  if (!currentFile.value) return
  try {
    await ElMessageBox.confirm(
      t('saves.rollbackConfirm', { name: currentFile.value.fileName, seq: version.seq }),
      t('saves.rollbackTitle'),
      { type: 'warning' },
    )
  } catch {
    return
  }
  try {
    await unwrap(
      api.post(
        `/user/saves/${encodeURIComponent(currentFile.value.fileName)}/versions/${version.id}/rollback`,
      ),
    )
    ElMessage.success(t('saves.rolledBack', { seq: version.seq }))
    drawerVisible.value = false
    load()
  } catch (error) {
    showError(error)
  }
}

function openDelete(row: SaveRow) {
  deleting.value = row
  deleteAll.value = false
  deleteVisible.value = true
}

async function submitDelete() {
  if (!deleting.value) return
  saving.value = true
  try {
    const result = await unwrap<{ freedBytes: number }>(
      api.delete(`/user/saves/${encodeURIComponent(deleting.value.fileName)}`, {
        params: { deleteVersions: deleteAll.value },
      }),
    )
    ElMessage.success(t('saves.deleted', { size: formatBytes(result.freedBytes) }))
    deleteVisible.value = false
    load()
  } catch (error) {
    showError(error)
  } finally {
    saving.value = false
  }
}

onMounted(load)

// apiBase 仅用于类型完整，实际请求走 axios 实例
void apiBase
</script>
