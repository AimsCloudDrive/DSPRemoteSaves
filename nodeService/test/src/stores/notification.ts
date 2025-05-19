// src/stores/notification.ts
import { defineStore } from "pinia";
import { ElNotification, ElLoading } from "element-plus";
import type { LoadingOptions } from "element-plus";

export const useNotificationStore = defineStore("notification", {
  actions: {
    showSuccess(message: string) {
      ElNotification.success({
        title: "成功",
        message,
        duration: 3000,
      });
    },

    showError(message: string) {
      ElNotification.error({
        title: "错误",
        message,
        duration: 5000,
      });
    },

    showLoading(options?: LoadingOptions) {
      return ElLoading.service({
        lock: true,
        text: "加载中...",
        background: "rgba(0, 0, 0, 0.7)",
        ...options,
      });
    },
  },
});
