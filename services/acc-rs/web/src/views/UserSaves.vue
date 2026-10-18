<template>
  <div class="dsp-page">
    <div class="dsp-page-header">
      <div>
        <h2 class="dsp-page-title">{{ t('saves.title') }} · {{ user?.userName }}</h2>
        <div class="dsp-page-subtitle">
          {{ t('users.quota') }} {{ formatBytes(user?.quotaBytes) }} · {{ t('users.used') }}
          {{ formatBytes(user?.usedBytes) }} · {{ t('users.reserved') }} {{ formatBytes(user?.reservedBytes) }}
        </div>
      </div>
      <div style="display: flex; gap: 8px">
        <el-button @click="router.back()">{{ t('common.back') }}</el-button>
        <el-button :loading="loading" @click="load">{{ t('common.refresh') }}</el-button>
      </div>
    </div>

    <div class="dsp-card" style="padding: 16px">
      <el-table v-loading="loading" :data="files" stripe :empty-text="t('common.empty')">
        <el-table-column prop="fileName" :label="t('saves.fileName')" min-width="220" />
        <el-table-column :label="t('saves.size')" width="120">
          <template #default="{ row }">{{ formatBytes(row.sizeBytes) }}</template>
        </el-table-column>
        <el-table-column :label="t('saves.seq')" width="90">
          <template #default="{ row }">#{{ row.currentSeq }}</template>
        </el-table-column>
        <el-table-column :label="t('saves.mtime')" width="180">
          <template #default="{ row }">{{ formatTime(row.mtimeMs) }}</template>
        </el-table-column>
        <el-table-column :label="t('saves.sm3')" width="140">
          <template #default="{ row }">
            <span class="dsp-mono">{{ shortHash(row.sm3) }}</span>
          </template>
        </el-table-column>
        <el-table-column :label="t('common.actions')" width="200" fixed="right">
          <template #default="{ row }">
            <el-button text type="primary" size="small" @click="openVersions(row)">{{ t('saves.versions') }}</el-button>
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
          <el-button
            size="small"
            :disabled="version.current"
            @click="rollback(version)"
          >
            {{ t('saves.rollback') }}
          </el-button>
        </div>
      </div>
    </el-drawer>

    <el-dialog v-model="deleteVisible" :title="t('saves.deleteTitle')" width="480px">
      <div style="margin-bottom: 12px">
        <strong>{{ deleting?.fileName }}</strong>
      </div>
      <el-alert type="warning" show-icon :closable="false" :title="t('saves.deleteCurrentWarning')" />
      <el-checkbox v-model="deleteAll" style="margin-top: 14px">{{ t('saves.deleteAll') }}</el-checkbox>
      <el-alert v-if="deleteAll" type="error" show-icon :closable="false" :title="t('saves.deleteAllWarning')" style="margin-top: 10px" />
      <template #footer>
        <el-button @click="deleteVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="danger" :loading="saving" @click="submitDelete">{{ t('common.delete') }}</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import { api, showError, unwrap } from '../api/client'
import { formatBytes, formatTime, shortHash } from '../utils/format'

interface SaveRow {
  id: string
  fileName: string
  sizeBytes: number
  mtimeMs: number
  sm3?: string
  currentSeq: number
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
const route = useRoute()
const router = useRouter()
const userId = route.params.id as string

const user = ref<{ userName: string; quotaBytes: number; usedBytes: number; reservedBytes: number } | null>(null)
const files = ref<SaveRow[]>([])
const loading = ref(false)

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
    const data = await unwrap<{ user: typeof user.value; files: SaveRow[] }>(
      api.get(`/admin/users/${userId}/saves`),
    )
    user.value = data.user
    files.value = data.files
  } catch (error) {
    showError(error)
  } finally {
    loading.value = false
  }
}

async function openVersions(row: SaveRow) {
  currentFile.value = row
  drawerVisible.value = true
  versionsLoading.value = true
  try {
    const data = await unwrap<{ versions: Version[] }>(
      api.get(`/admin/users/${userId}/saves/${encodeURIComponent(row.fileName)}/versions`),
    )
    versions.value = data.versions
  } catch (error) {
    showError(error)
  } finally {
    versionsLoading.value = false
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
        `/admin/users/${userId}/saves/${encodeURIComponent(currentFile.value.fileName)}/versions/${version.id}/rollback`,
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
    await unwrap(
      api.delete(`/admin/users/${userId}/saves/${encodeURIComponent(deleting.value.fileName)}`, {
        params: { deleteVersions: deleteAll.value },
      }),
    )
    ElMessage.success(t('common.success'))
    deleteVisible.value = false
    load()
  } catch (error) {
    showError(error)
  } finally {
    saving.value = false
  }
}

onMounted(load)
</script>
