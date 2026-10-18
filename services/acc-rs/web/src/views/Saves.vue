<template>
  <div class="dsp-page">
    <div class="dsp-page-header">
      <div>
        <h2 class="dsp-page-title">{{ t('saves.title') }}</h2>
        <div class="dsp-page-subtitle">{{ t('common.total', { n: total }) }}</div>
      </div>
      <el-button :loading="loading" @click="load">{{ t('common.refresh') }}</el-button>
    </div>

    <div class="dsp-card" style="padding: 16px">
      <div class="dsp-toolbar">
        <el-input
          v-model="query.userName"
          :placeholder="t('saves.userNamePlaceholder')"
          clearable
          style="width: 240px"
          @keyup.enter="reload"
        />
        <el-input
          v-model="query.fileName"
          :placeholder="t('saves.searchPlaceholder')"
          clearable
          style="width: 240px"
          @keyup.enter="reload"
        />
        <el-button type="primary" @click="reload">{{ t('common.search') }}</el-button>
      </div>

      <el-table v-loading="loading" :data="rows" stripe :empty-text="t('common.empty')">
        <el-table-column prop="userName" :label="t('saves.owner')" width="160">
          <template #default="{ row }">
            <el-link type="primary" @click="router.push(`/users/${row.userId}/saves`)">{{ row.userName }}</el-link>
          </template>
        </el-table-column>
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
        <el-table-column :label="t('common.actions')" width="120" fixed="right">
          <template #default="{ row }">
            <el-button text type="primary" size="small" @click="router.push(`/users/${row.userId}/saves`)">
              {{ t('saves.versions') }}
            </el-button>
          </template>
        </el-table-column>
      </el-table>

      <div style="display: flex; justify-content: flex-end; margin-top: 14px">
        <el-pagination
          v-model:current-page="query.page"
          v-model:page-size="query.pageSize"
          :total="total"
          :page-sizes="[10, 20, 50]"
          layout="total, sizes, prev, pager, next"
          @change="load"
        />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { api, showError, unwrap } from '../api/client'
import { formatBytes, formatTime } from '../utils/format'

interface Row {
  id: string
  userId: string
  userName: string
  fileName: string
  sizeBytes: number
  mtimeMs: number
  currentSeq: number
}

const { t } = useI18n()
const router = useRouter()
const rows = ref<Row[]>([])
const total = ref(0)
const loading = ref(false)
const query = reactive({ page: 1, pageSize: 20, userName: '', fileName: '' })

async function load() {
  loading.value = true
  try {
    const data = await unwrap<{ items: Row[]; total: number }>(
      api.get('/admin/saves', {
        params: {
          page: query.page,
          pageSize: query.pageSize,
          userName: query.userName || undefined,
          fileName: query.fileName || undefined,
        },
      }),
    )
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

onMounted(load)
</script>
