<!-- src/App.vue -->
<script setup lang="ts">
import { RouterView } from "vue-router";
import { ElContainer, ElHeader, ElMain } from "element-plus";

// 可在此处添加全局状态或逻辑
</script>

<template>
  <el-container class="app-container">
    <!-- 顶部导航栏 -->
    <el-header height="60px" class="app-header">
      <div class="header-content">
        <h1>DSP 云存档系统</h1>
        <nav class="nav-links">
          <router-link to="/login">登录</router-link>
          <router-link to="/register">注册</router-link>
          <router-link to="/files" v-if="$route.path !== '/files'"
            >文件列表</router-link
          >
        </nav>
      </div>
    </el-header>

    <!-- 主要内容区域 -->
    <el-main>
      <router-view v-slot="{ Component }">
        <transition name="fade" mode="out-in">
          <component :is="Component" />
        </transition>
      </router-view>
    </el-main>
  </el-container>
</template>

<style scoped>
.app-container {
  min-height: 100vh;
}

.app-header {
  background-color: #2c3e50;
  color: white;
  line-height: 60px;
  box-shadow: 0 2px 4px rgba(0, 0, 0, 0.1);
}

.header-content {
  display: flex;
  justify-content: space-between;
  align-items: center;
  max-width: 1200px;
  margin: 0 auto;
}

.nav-links {
  display: flex;
  gap: 20px;

  a {
    color: white;
    text-decoration: none;
    padding: 5px 10px;
    border-radius: 4px;
    transition: background-color 0.3s;

    &:hover {
      background-color: rgba(255, 255, 255, 0.1);
    }

    &.router-link-exact-active {
      background-color: rgba(255, 255, 255, 0.2);
    }
  }
}

/* 页面过渡动画 */
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.3s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
