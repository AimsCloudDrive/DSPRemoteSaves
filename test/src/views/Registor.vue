<script setup lang="ts">
import { ref } from "vue";
import { useRouter } from "vue-router";
import api from "../api";

const router = useRouter();
const form = ref({
  userName: "",
  password: "",
  confirmPassword: "",
});

const handleRegister = async () => {
  if (form.value.password !== form.value.confirmPassword) {
    alert("两次密码输入不一致");
    return;
  }

  try {
    const res = await api.register(form.value.userName, form.value.password);
    // 直接访问res.code
    if (res.code === 0) {
      alert("注册成功");
      router.push("/login");
    }
  } catch (error) {
    console.error("Registration failed:", error);
  }
};
</script>

<template>
  <el-form :model="form" label-width="100px">
    <el-form-item label="用户名">
      <el-input v-model="form.userName" />
    </el-form-item>
    <el-form-item label="密码">
      <el-input v-model="form.password" type="password" />
    </el-form-item>
    <el-form-item label="确认密码">
      <el-input v-model="form.confirmPassword" type="password" />
    </el-form-item>
    <el-form-item>
      <el-button type="primary" @click="handleRegister">注册</el-button>
      <el-button @click="$router.push('/login')">返回登录</el-button>
    </el-form-item>
  </el-form>
</template>
