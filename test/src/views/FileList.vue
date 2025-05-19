<script setup lang="ts">
import { onMounted, ref } from "vue";
import { assert } from "@ocean/common";
import { useAuthStore } from "../stores/auth";
import api from "../api";
import { useNotificationStore } from "../stores/notification";

const authStore = useAuthStore();
const files = ref<Array<{ fileName: string; fileSize: number }>>([]);
const loading = ref(false);
const notification = useNotificationStore();

// 文件列表
const loadFiles = async () => {
  try {
    loading.value = true;
    const res = await api.getFileList(authStore.userName, authStore.password);
    // 访问res.payload.files
    if (res.code === 0 && res.payload) {
      files.value = res.payload.files;
    }
  } finally {
    loading.value = false;
  }
};

// 文件下载
const handleDownload = async (fileName: string) => {
  let downloadNotification:
    | ReturnType<(typeof notification)["showLoading"]>
    | undefined;
  try {
    downloadNotification = notification.showLoading({
      text: `正在下载 ${fileName}...`,
      spinner: "el-icon-download",
    });
    const res = await api.downloadFile(
      fileName,
      authStore.userName,
      authStore.password
    );
    if (res.code === 0 && res.payload) {
      notification.showSuccess(`${fileName} 下载完成`);
      const url = window.URL.createObjectURL(res.payload);
      const link = document.createElement("a");
      link.href = url;
      link.setAttribute("download", fileName);
      document.body.appendChild(link);
      link.click();
      link.remove();
      URL.revokeObjectURL(url);
    }
  } catch (error) {
    notification.showError(`${fileName} 下载失败`);
  } finally {
    downloadNotification?.close();
  }
};

// 文件上传
const chunkSize = 5 * 1024 * 1024; // 5MB
const handleUpload = async (file: File | undefined) => {
  if (!file) {
    return;
  }
  let uploadNotification:
    | ReturnType<(typeof notification)["showLoading"]>
    | undefined;
  try {
    uploadNotification = notification.showLoading({
      text: `正在上传 ${file.name}...`,
      spinner: "el-icon-loading",
    });
    const initRes = await api.initUpload(
      file.name,
      file.size,
      chunkSize,
      authStore.userName,
      authStore.password
    );

    // 访问initRes.payload.data
    const { code, message, payload } = initRes;
    if (code !== 0) {
      message && notification.showError(message);
    }

    assert(payload);

    const { fileId, totalChunks } = payload.data;

    // 分片上传
    for (let i = 0; i < totalChunks; i++) {
      const chunk = file.slice(i * chunkSize, (i + 1) * chunkSize);
      await api.uploadChunk(
        fileId,
        i,
        chunk,
        authStore.userName,
        authStore.password
      );
    }

    // 完成上传
    await api.completeUpload(fileId, authStore.userName, authStore.password);
    notification.showSuccess(`${file.name} 上传成功`);
    loadFiles();
  } catch (error) {
    notification.showError(
      `${file.name} 上传失败: ${error instanceof Error ? error.message : error}`
    );
  } finally {
    uploadNotification?.close();
  }
};

// 用户注销
const handleLogout = () => {
  authStore.logout();
  window.location.reload();
};

onMounted(() => {
  loadFiles();
});
</script>

<template>
  <div>
    <el-button @click="handleLogout">注销</el-button>

    <el-upload
      :auto-upload="false"
      :show-file-list="false"
      :on-change="(file) => handleUpload(file.raw)"
    >
      <el-button type="primary">上传文件</el-button>
    </el-upload>

    <el-table :data="files" v-loading="loading">
      <el-table-column prop="fileName" label="文件名" />
      <el-table-column prop="fileSize" label="文件大小（bytes）" />
      <el-table-column label="操作">
        <template #default="{ row }">
          <el-button @click="handleDownload(row.fileName)">下载</el-button>
        </template>
      </el-table-column>
    </el-table>
  </div>
</template>
