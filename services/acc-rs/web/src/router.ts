import { createRouter, createWebHistory } from 'vue-router'
import { useAuthStore } from './stores/auth'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/login', name: 'login', component: () => import('./views/Login.vue'), meta: { public: true } },
    { path: '/register', name: 'register', component: () => import('./views/Register.vue'), meta: { public: true } },
    { path: '/reset', name: 'reset', component: () => import('./views/Reset.vue'), meta: { public: true } },
    {
      path: '/',
      component: () => import('./components/Layout.vue'),
      children: [
        { path: '', name: 'dashboard', component: () => import('./views/Dashboard.vue'), meta: { admin: true } },
        { path: 'users', name: 'users', component: () => import('./views/Users.vue'), meta: { admin: true } },
        {
          path: 'users/:id/saves',
          name: 'user-saves',
          component: () => import('./views/UserSaves.vue'),
          meta: { admin: true },
        },
        { path: 'saves', name: 'saves', component: () => import('./views/Saves.vue'), meta: { admin: true } },
        { path: 'audit', name: 'audit', component: () => import('./views/Audit.vue'), meta: { admin: true } },
        { path: 'settings', name: 'settings', component: () => import('./views/Settings.vue'), meta: { admin: true } },
        { path: 'my-saves', name: 'my-saves', component: () => import('./views/MySaves.vue') },
        { path: 'profile', name: 'profile', component: () => import('./views/Profile.vue') },
      ],
    },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

router.beforeEach((to) => {
  const auth = useAuthStore()
  if (to.meta.public) {
    if (auth.isLoggedIn) return { name: auth.isAdmin ? 'dashboard' : 'my-saves' }
    return true
  }
  if (!auth.isLoggedIn) {
    return { name: 'login', query: { redirect: to.fullPath } }
  }
  if (to.meta.admin && !auth.canAdmin) {
    // 管理员但只有 user 受众：引导走管理端登录（内网/VPN）
    return { name: 'login', query: { admin: '1', redirect: to.fullPath } }
  }
  if (!to.meta.admin && to.name === 'dashboard' && !auth.isAdmin) {
    return { name: 'my-saves' }
  }
  return true
})

export default router
