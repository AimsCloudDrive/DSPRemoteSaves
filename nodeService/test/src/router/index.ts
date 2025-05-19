import { createRouter, createWebHistory } from "vue-router";

// src/router/index.ts
const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: "/",
      redirect: "/login",
      meta: { hideHeader: true }, // 可选：添加路由元信息
    },
    {
      path: "/login",
      component: () => import("../views/Login.vue"),
      meta: { title: "登录" },
    },
    {
      path: "/register",
      component: () => import("../views/Registor.vue"),
      meta: { title: "注册" },
    },
    {
      path: "/files",
      component: () => import("../views/FileList.vue"),
      meta: { requiresAuth: true, title: "文件列表" },
    },
  ],
});

// 添加路由守卫处理标题
router.beforeEach((to) => {
  if (to.meta.title) {
    document.title = `${to.meta.title} | DSP云存档系统`;
  }
});

export default router;
