<template>
  <el-container>
    <el-aside width="230px" class="dsp-sidebar">
      <div class="dsp-logo">
        <div class="dsp-logo-mark">D</div>
        <span>{{ t('common.appName') }}</span>
      </div>
      <el-menu :default-active="active" router>
        <template v-if="auth.isAdmin">
          <el-menu-item index="/">
            <el-icon><DataLine /></el-icon><span>{{ t('common.dashboard') }}</span>
          </el-menu-item>
          <el-menu-item index="/users">
            <el-icon><User /></el-icon><span>{{ t('common.users') }}</span>
          </el-menu-item>
          <el-menu-item index="/saves">
            <el-icon><Folder /></el-icon><span>{{ t('common.saves') }}</span>
          </el-menu-item>
          <el-menu-item index="/audit">
            <el-icon><Document /></el-icon><span>{{ t('common.audit') }}</span>
          </el-menu-item>
          <el-menu-item index="/settings">
            <el-icon><Setting /></el-icon><span>{{ t('common.settings') }}</span>
          </el-menu-item>
        </template>
        <el-menu-item index="/my-saves">
          <el-icon><FolderOpened /></el-icon><span>{{ t('saves.myTitle') }}</span>
        </el-menu-item>
        <el-menu-item index="/profile">
          <el-icon><Avatar /></el-icon><span>{{ t('common.profile') }}</span>
        </el-menu-item>
      </el-menu>
      <div class="dsp-sidebar-footer">v2.0 · {{ t('auth.adminLoginSubtitle') }}</div>
    </el-aside>

    <el-container>
      <el-header class="dsp-header">
        <div class="dsp-header-left">
          <span>{{ pageTitle }}</span>
          <el-tag v-if="auth.isAdmin" size="small" type="warning" effect="plain">
            {{ t('users.roleAdmin') }}
          </el-tag>
        </div>
        <div class="dsp-header-right">
          <el-dropdown trigger="click" @command="changeLocale">
            <el-button text>
              <el-icon><Switch /></el-icon>
              <span style="margin-left: 6px">{{ currentLocaleLabel }}</span>
            </el-button>
            <template #dropdown>
              <el-dropdown-menu>
                <el-dropdown-item command="zh-CN">{{ t('common.chinese') }}</el-dropdown-item>
                <el-dropdown-item command="en-US">{{ t('common.english') }}</el-dropdown-item>
              </el-dropdown-menu>
            </template>
          </el-dropdown>

          <el-dropdown trigger="click" @command="onUserCommand">
            <span class="dsp-header-user" style="cursor: pointer; display: flex; align-items: center; gap: 8px">
              <el-avatar :size="28">{{ initial }}</el-avatar>
              <span>{{ auth.user?.userName }}</span>
            </span>
            <template #dropdown>
              <el-dropdown-menu>
                <el-dropdown-item command="profile">{{ t('common.profile') }}</el-dropdown-item>
                <el-dropdown-item command="logout" divided>{{ t('common.logout') }}</el-dropdown-item>
              </el-dropdown-menu>
            </template>
          </el-dropdown>
        </div>
      </el-header>

      <el-main class="dsp-main">
        <el-alert
          v-if="auth.user?.mustChangePassword && route.name !== 'profile'"
          type="warning"
          show-icon
          :closable="false"
          style="margin: 16px 24px 0"
          :title="t('auth.mustChangePassword')"
        />
        <router-view />
      </el-main>
    </el-container>
  </el-container>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import {
  Avatar,
  DataLine,
  Document,
  Folder,
  FolderOpened,
  Setting,
  Switch,
  User,
} from '@element-plus/icons-vue'
import { useAuthStore } from '../stores/auth'
import { applyLocale } from '../i18n'

const { t, locale } = useI18n()
const route = useRoute()
const router = useRouter()
const auth = useAuthStore()

const active = computed(() => (route.path === '/' ? '/' : route.path))
const initial = computed(() => (auth.user?.userName || '?').slice(0, 1).toUpperCase())
const currentLocaleLabel = computed(() => (locale.value === 'en-US' ? t('common.english') : t('common.chinese')))

const pageTitle = computed(() => {
  const key = route.name as string
  const map: Record<string, string> = {
    dashboard: 'common.dashboard',
    users: 'users.title',
    'user-saves': 'saves.title',
    saves: 'saves.title',
    audit: 'audit.title',
    settings: 'settings.title',
    'my-saves': 'saves.myTitle',
    profile: 'profile.title',
  }
  return map[key] ? t(map[key]) : t('common.appName')
})

async function changeLocale(value: string) {
  const next = value === 'en-US' ? 'en-US' : 'zh-CN'
  applyLocale(next)
  try {
    await auth.setLocale(next)
  } catch {
    /* 语言本地已生效，服务端同步失败不阻塞 */
  }
}

async function onUserCommand(command: string) {
  if (command === 'profile') {
    router.push('/profile')
    return
  }
  if (command === 'logout') {
    await auth.logout()
    router.push('/login')
  }
}
</script>
