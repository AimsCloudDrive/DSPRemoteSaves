<!-- FileListDownloadTest.vue -->
<template>
  <div class="download-container">
    <ElCard class="box-card">
      <template #header>
        <div class="card-header">
          <span>文件下载测试</span>
        </div>
      </template>

      <ElForm :model="form" label-width="100px">
        <ElFormItem label="用户名">
          <ElInput v-model="form.username" />
        </ElFormItem>
        <ElFormItem label="密码">
          <ElInput v-model="form.password" type="password" />
        </ElFormItem>
        <ElButton type="primary" @click="fetchFiles" :loading="isLoading">
          获取文件列表
        </ElButton>
      </ElForm>

      <ElTable :data="fileList" style="width: 100%; margin-top: 20px">
        <ElTableColumn prop="fileName" label="文件名" />
        <ElTableColumn label="大小">
          <template #default="{ row }">
            {{ formatFileSize(row.fileSize) }}
          </template>
        </ElTableColumn>
        <ElTableColumn label="操作">
          <template #default="{ row }">
            <ElButton size="small" @click="downloadFile(row.fileName)">
              下载
            </ElButton>
          </template>
        </ElTableColumn>
      </ElTable>
    </ElCard>
  </div>
</template>

<script setup lang="ts">
import { ref } from "vue";
import { ElMessage } from "element-plus";

interface FileInfo {
  fileName: string;
  fileSize: number;
}

interface DownloadForm {
  username: string;
  password: string;
}

const form = ref<DownloadForm>({
  username: "",
  password: "",
});

const fileList = ref<FileInfo[]>([]);
const isLoading = ref(false);

const formatFileSize = (bytes: number) => {
  if (bytes === 0) return `0 B (0 B)`;
  const units = ["B", "KB", "MB", "GB", "TB"];
  let unitIndex = 0;
  let size = bytes;

  // 计算合适单位
  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex++;
  }

  // 四舍五入保留一位小数
  let formatted = size.toFixed(1);
  // 去除.0后缀
  if (formatted.endsWith(".0")) {
    formatted = formatted.slice(0, -2);
  }

  // 构造显示格式
  return `${formatted}${units[unitIndex]} (${bytes} B)`;
};

// 保持其他方法不变
const fetchJSON = async (url: string, options: RequestInit = {}) => {
  const response = await fetch("http://localhost:9999" + url, {
    ...options,
    headers: {
      "Content-Type": "application/json",
      ...options.headers,
    },
  });
  if (!response.ok) throw new Error(response.statusText);
  return response.json();
};

const fetchFiles = async () => {
  if (!form.value.username || !form.value.password) {
    ElMessage.warning("请输入用户名和密码");
    return;
  }

  try {
    isLoading.value = true;
    const data = await fetchJSON("/dsp.saves/api/download/file-list", {
      method: "POST",
      body: JSON.stringify({
        userName: form.value.username,
        password: form.value.password,
      }),
    });
    fileList.value = data.files;
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : "获取列表失败");
  } finally {
    isLoading.value = false;
  }
};

const downloadFile = async (fileName: string) => {
  try {
    const response = await fetch(
      "http://localhost:9999" + "/dsp.saves/api/download/download",
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          userName: form.value.username,
          password: form.value.password,
          fileName,
          isChunk: false,
        }),
      }
    );

    if (!response.ok) throw new Error("下载失败");

    const blob = await response.blob();
    const url = window.URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.setAttribute("download", fileName);
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : "下载失败");
  }
};
</script>

<style scoped>
.download-container {
  max-width: 800px;
  margin: 20px auto;
}
.box-card {
  padding: 20px;
}
</style>
