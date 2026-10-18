<template>
  <div class="dsp-page">
    <div class="dsp-page-header">
      <div>
        <h2 class="dsp-page-title">{{ t('users.title') }}</h2>
        <div class="dsp-page-subtitle">{{ t('common.total', { n: total }) }}</div>
      </div>
      <el-button type="primary" @click="openCreate">{{ t('users.create') }}</el-button>
    </div>

    <div class="dsp-card" style="padding: 16px">
      <div class="dsp-toolbar">
        <el-input
          v-model="query.search"
          :placeholder="t('users.searchPlaceholder')"
          clearable
          style="width: 260px"
          @keyup.enter="reload"
        />
        <el-select v-model="query.status" clearable :placeholder="t('common.status')" style="width: 140px">
          <el-option :label="t('common.all')" value="" />
          <el-option label="active" value="active" />
          <el-option label="disabled" value="disabled" />
          <el-option label="deleted" value="deleted" />
        </el-select>
        <el-select v-model="query.role" clearable :placeholder="t('users.role')" style="width: 150px">
          <el-option :label="t('common.all')" value="" />
          <el-option :label="t('users.roleAdmin')" value="admin" />
          <el-option :label="t('users.roleUser')" value="user" />
        </el-select>
        <el-button @click="reload">{{ t('common.search') }}</el-button>
        <div class="spacer" />
        <el-button :loading="loading" @click="load">{{ t('common.refresh') }}</el-button>
      </div>

      <el-table v-loading="loading" :data="rows" stripe :empty-text="t('common.empty')">
        <el-table-column prop="userName" :label="t('auth.username')" min-width="150">
          <template #default="{ row }">
            <div style="font-weight: 600">{{ row.userName }}</div>
            <div class="dsp-muted" style="font-size: 12px">{{ row.email || '—' }}</div>
          </template>
        </el-table-column>
        <el-table-column :label="t('users.role')" width="120">
          <template #default="{ row }">
            <el-tag size="small" :type="row.role === 'admin' ? 'warning' : 'info'" effect="plain">
              {{ row.role === 'admin' ? t('users.roleAdmin') : t('users.roleUser') }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column :label="t('common.status')" width="110">
          <template #default="{ row }">
            <el-tag
              size="small"
              :type="row.status === 'active' ? 'success' : row.status === 'disabled' ? 'danger' : 'info'"
            >
              {{ row.status }}
            </el-tag>
            <el-tag v-if="row.mustChangePassword" size="small" type="warning" style="margin-left: 4px">
              {{ t('users.mustChange') }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column :label="t('users.quota')" min-width="190">
          <template #default="{ row }">
            <div style="display: flex; justify-content: space-between; font-size: 12px">
              <span>{{ formatBytes(row.usedBytes) }}</span>
              <span class="dsp-muted">/ {{ formatBytes(row.quotaBytes) }}</span>
            </div>
            <el-progress
              :percentage="usagePercent(row)"
              :stroke-width="6"
              :status="usagePercent(row) >= 100 ? 'exception' : undefined"
              :show-text="false"
              style="margin-top: 4px"
            />
          </template>
        </el-table-column>
        <el-table-column :label="t('users.lastLogin')" width="170">
          <template #default="{ row }">{{ formatTime(row.lastLoginAt) }}</template>
        </el-table-column>
        <el-table-column :label="t('common.actions')" width="210" fixed="right">
          <template #default="{ row }">
            <el-button text type="primary" size="small" @click="router.push(`/users/${row.id}/saves`)">
              {{ t('users.viewSaves') }}
            </el-button>
            <el-dropdown trigger="click" @command="(cmd: string) => onCommand(cmd, row)">
              <el-button text size="small">{{ t('common.edit') }}</el-button>
              <template #dropdown>
                <el-dropdown-menu>
                  <el-dropdown-item command="quota">{{ t('users.quota') }}</el-dropdown-item>
                  <el-dropdown-item command="reset">{{ t('users.resetPassword') }}</el-dropdown-item>
                  <el-dropdown-item command="role">
                    {{ row.role === 'admin' ? t('users.makeUser') : t('users.makeAdmin') }}
                  </el-dropdown-item>
                  <el-dropdown-item command="status">
                    {{ row.status === 'active' ? t('users.disable') : t('users.enable') }}
                  </el-dropdown-item>
                  <el-dropdown-item command="delete" divided>{{ t('common.delete') }}</el-dropdown-item>
                </el-dropdown-menu>
              </template>
            </el-dropdown>
          </template>
        </el-table-column>
      </el-table>

      <div style="display: flex; justify-content: flex-end; margin-top: 14px">
        <el-pagination
          v-model:current-page="query.page"
          v-model:page-size="query.pageSize"
          :total="total"
          :page-sizes="[10, 20, 50, 100]"
          layout="total, sizes, prev, pager, next"
          @change="load"
        />
      </div>
    </div>

    <!-- 新建用户 -->
    <el-dialog v-model="createVisible" :title="t('users.createTitle')" width="520px">
      <el-form label-position="top">
        <el-form-item :label="t('auth.username')">
          <el-input v-model="createForm.userName" :placeholder="t('auth.usernamePlaceholder')" />
        </el-form-item>
        <el-form-item :label="`${t('auth.email')} (${t('common.optional')})`">
          <el-input v-model="createForm.email" :placeholder="t('auth.emailPlaceholder')" />
        </el-form-item>
        <el-form-item :label="t('auth.password')">
          <el-input v-model="createForm.password" type="password" show-password autocomplete="new-password" />
          <PasswordChecklist :password="createForm.password" :user-name="createForm.userName" :email="createForm.email" />
        </el-form-item>
        <el-form-item :label="t('users.role')">
          <el-radio-group v-model="createForm.role">
            <el-radio-button value="user">{{ t('users.roleUser') }}</el-radio-button>
            <el-radio-button value="admin">{{ t('users.roleAdmin') }}</el-radio-button>
          </el-radio-group>
        </el-form-item>
        <el-form-item :label="`${t('users.quota')} (GB)`">
          <el-input-number v-model="createForm.quotaGb" :min="0.1" :step="0.5" :precision="1" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="createVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" :loading="creating" @click="submitCreate">{{ t('common.create') }}</el-button>
      </template>
    </el-dialog>

    <!-- 调整配额 -->
    <el-dialog v-model="quotaVisible" :title="t('users.quota')" width="420px">
      <div class="dsp-muted" style="margin-bottom: 10px">{{ editing?.userName }}</div>
      <el-input-number v-model="quotaGb" :min="0" :step="0.5" :precision="2" style="width: 100%" />
      <div class="dsp-muted" style="font-size: 12px; margin-top: 8px">
        {{ t('users.used') }} {{ formatBytes(editing?.usedBytes) }} ·
        {{ t('users.reserved') }} {{ formatBytes(editing?.reservedBytes) }}
      </div>
      <template #footer>
        <el-button @click="quotaVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="primary" :loading="saving" @click="submitQuota">{{ t('common.save') }}</el-button>
      </template>
    </el-dialog>

    <!-- 重置密码 -->
    <el-dialog v-model="resetVisible" :title="t('users.resetTitle')" width="480px">
      <div class="dsp-muted" style="margin-bottom: 12px">{{ editing?.userName }} · {{ t('users.resetHint') }}</div>
      <el-input v-model="resetPassword" type="password" show-password autocomplete="new-password" :placeholder="t('common.optional')" />
      <el-alert
        v-if="generatedPassword"
        type="warning"
        show-icon
        :closable="false"
        style="margin-top: 14px"
        :title="t('users.generatedPassword')"
      >
        <div class="dsp-mono" style="font-size: 16px; margin: 6px 0">{{ generatedPassword }}</div>
        <div style="font-size: 12px">{{ t('users.generatedHint') }}</div>
      </el-alert>
      <template #footer>
        <el-button @click="closeReset">{{ t('common.close') }}</el-button>
        <el-button type="primary" :loading="saving" @click="submitReset">{{ t('common.confirm') }}</el-button>
      </template>
    </el-dialog>

    <!-- 删除用户 -->
    <el-dialog v-model="deleteVisible" :title="t('users.deleteTitle')" width="480px">
      <el-alert type="error" :closable="false" show-icon :title="t('users.purgeWarning')" style="margin-bottom: 12px" />
      <div style="margin-bottom: 10px">{{ t('users.deleteHint', { name: editing?.userName }) }}</div>
      <el-input v-model="deleteConfirmName" :placeholder="t('users.confirmName')" />
      <el-radio-group v-model="purge" style="margin-top: 14px; display: grid; gap: 8px">
        <el-radio :value="false">{{ t('users.deleteSoft') }}</el-radio>
        <el-radio :value="true">{{ t('users.deletePurge') }}</el-radio>
      </el-radio-group>
      <template #footer>
        <el-button @click="deleteVisible = false">{{ t('common.cancel') }}</el-button>
        <el-button type="danger" :loading="saving" :disabled="deleteConfirmName !== editing?.userName" @click="submitDelete">
          {{ t('common.delete') }}
        </el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import PasswordChecklist from '../components/PasswordChecklist.vue'
import { api, showError, unwrap } from '../api/client'
import { envelopeHeaders, nonce, seal } from '../api/crypto'
import { formatBytes, formatTime } from '../utils/format'

interface UserRow {
  id: string
  userName: string
  email?: string | null
  role: 'admin' | 'user'
  status: string
  quotaBytes: number
  usedBytes: number
  reservedBytes: number
  mustChangePassword: boolean
  createdAt: number
  lastLoginAt?: number | null
}

const { t } = useI18n()
const router = useRouter()

const rows = ref<UserRow[]>([])
const total = ref(0)
const loading = ref(false)
const query = reactive({ page: 1, pageSize: 20, search: '', status: '', role: '' })

const createVisible = ref(false)
const creating = ref(false)
const createForm = reactive({ userName: '', email: '', password: '', role: 'user', quotaGb: 1 })

const quotaVisible = ref(false)
const quotaGb = ref(1)
const resetVisible = ref(false)
const resetPassword = ref('')
const generatedPassword = ref('')
const deleteVisible = ref(false)
const deleteConfirmName = ref('')
const purge = ref(false)
const editing = ref<UserRow | null>(null)
const saving = ref(false)

function usagePercent(row: UserRow) {
  if (!row.quotaBytes) return 100
  return Math.min(100, Math.round(((row.usedBytes + row.reservedBytes) / row.quotaBytes) * 100))
}

async function load() {
  loading.value = true
  try {
    const data = await unwrap<{ items: UserRow[]; total: number }>(
      api.get('/admin/users', {
        params: {
          page: query.page,
          pageSize: query.pageSize,
          search: query.search || undefined,
          status: query.status || undefined,
          role: query.role || undefined,
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

function openCreate() {
  createForm.userName = ''
  createForm.email = ''
  createForm.password = ''
  createForm.role = 'user'
  createForm.quotaGb = 1
  createVisible.value = true
}

async function submitCreate() {
  const name = createForm.userName.trim()
  if (name.length < 3 || name.length > 32) {
    ElMessage.warning(t('auth.usernameInvalid'))
    return
  }
  if (!createForm.password) {
    ElMessage.warning(t('auth.passwordPlaceholder') || t('pwd.required') || '请输入密码')
    return
  }
  if (createForm.email.trim() && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(createForm.email.trim())) {
    ElMessage.warning(t('auth.emailInvalid'))
    return
  }
  creating.value = true
  try {
    const sealed = await seal([createForm.password])
    await unwrap(
      api.post(
        '/admin/users',
        {
          userName: createForm.userName.trim(),
          email: createForm.email.trim() || null,
          password: sealed.values[0],
          role: createForm.role,
          quotaBytes: Math.round(createForm.quotaGb * 1024 ** 3),
          timestamp: Date.now(),
          nonce: nonce(),
        },
        { headers: envelopeHeaders(sealed) },
      ),
    )
    ElMessage.success(t('users.created'))
    createVisible.value = false
    load()
  } catch (error) {
    showError(error)
  } finally {
    creating.value = false
  }
}

function onCommand(command: string, row: UserRow) {
  editing.value = row
  if (command === 'quota') {
    quotaGb.value = Number((row.quotaBytes / 1024 ** 3).toFixed(2))
    quotaVisible.value = true
  } else if (command === 'reset') {
    resetPassword.value = ''
    generatedPassword.value = ''
    resetVisible.value = true
  } else if (command === 'delete') {
    deleteConfirmName.value = ''
    purge.value = false
    deleteVisible.value = true
  } else if (command === 'role') {
    patch({ role: row.role === 'admin' ? 'user' : 'admin' })
  } else if (command === 'status') {
    patch({ status: row.status === 'active' ? 'disabled' : 'active' })
  }
}

async function patch(body: Record<string, unknown>) {
  if (!editing.value) return
  saving.value = true
  try {
    await unwrap(api.patch(`/admin/users/${editing.value.id}`, body))
    ElMessage.success(t('users.statusUpdated'))
    load()
  } catch (error) {
    showError(error)
  } finally {
    saving.value = false
  }
}

async function submitQuota() {
  if (!editing.value) return
  saving.value = true
  try {
    await unwrap(
      api.patch(`/admin/users/${editing.value.id}`, {
        quotaBytes: Math.round(quotaGb.value * 1024 ** 3),
      }),
    )
    ElMessage.success(t('users.quotaUpdated'))
    quotaVisible.value = false
    load()
  } catch (error) {
    showError(error)
  } finally {
    saving.value = false
  }
}

async function submitReset() {
  if (!editing.value) return
  saving.value = true
  try {
    const sealed = resetPassword.value ? await seal([resetPassword.value]) : null
    const data = await unwrap<{ generatedPassword?: string }>(
      api.post(
        `/admin/users/${editing.value.id}/reset-password`,
        {
          password: sealed ? sealed.values[0] : null,
          timestamp: Date.now(),
          nonce: nonce(),
        },
        sealed ? { headers: envelopeHeaders(sealed) } : undefined,
      ),
    )
    generatedPassword.value = data.generatedPassword || ''
    ElMessage.success(t('users.resetDone'))
    if (!generatedPassword.value) closeReset()
    load()
  } catch (error) {
    showError(error)
  } finally {
    saving.value = false
  }
}

function closeReset() {
  resetVisible.value = false
  generatedPassword.value = ''
}

async function submitDelete() {
  if (!editing.value) return
  saving.value = true
  try {
    await unwrap(
      api.delete(`/admin/users/${editing.value.id}`, { params: { purgeSaves: purge.value } }),
    )
    ElMessage.success(t('users.deleted'))
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
