<script setup lang="ts">
import { ref } from "vue";
import { useRouter } from "vue-router";
import { useAuthStore } from "../stores/auth";
import api from "../api";
import { useNotificationStore } from "../stores/notification";

const router = useRouter();
const authStore = useAuthStore();

const form = ref({
  userName: "",
  password: "",
});
const notification = useNotificationStore();

const handleLogin = async () => {
  try {
    const res = await api.login(form.value.userName, form.value.password);
    // 从 res.code 访问状态码，而不是 res.data.code
    if (res.code === 0) {
      authStore.login(form.value.userName, form.value.password);
      router.push("/files");
    } else {
      notification.showError("密码错误，请重试");
    }
  } catch (error) {
    console.error("Login failed:", error);
  }
};
</script>

<template>
  <el-form :model="form" label-width="80px">
    <el-form-item label="用户名">
      <el-input v-model="form.userName" />
    </el-form-item>
    <el-form-item label="密码">
      <el-input v-model="form.password" type="password" />
    </el-form-item>
    <el-form-item>
      <el-button type="primary" @click="handleLogin">登录</el-button>
      <el-button @click="$router.push('/register')">注册</el-button>
    </el-form-item>
  </el-form>
</template>
