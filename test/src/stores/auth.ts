import { defineStore } from "pinia";

export const useAuthStore = defineStore("auth", {
  state: () => ({
    isAuthenticated: false,
    userName: "",
    password: "",
  }),
  actions: {
    login(userName: string, password: string) {
      this.isAuthenticated = true;
      this.userName = userName;
      this.password = password;
    },
    logout() {
      this.isAuthenticated = false;
      this.userName = "";
      this.password = "";
    },
  },
  // persist: true, // 使用插件持久化存储
});
