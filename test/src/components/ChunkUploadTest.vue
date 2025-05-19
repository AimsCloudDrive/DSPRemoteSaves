<!-- ChunkUploadTest.vue -->
<template>
  <div class="upload-container">
    <ElCard class="box-card">
      <template #header>
        <div class="card-header">
          <span>分片上传测试</span>
        </div>
      </template>

      <ElForm :model="form" label-width="100px">
        <ElFormItem label="用户名">
          <ElInput v-model="form.username" />
        </ElFormItem>
        <ElFormItem label="密码">
          <ElInput v-model="form.password" type="password" />
        </ElFormItem>
        <ElFormItem label="选择文件">
          <input type="file" @change="handleFileSelect" />
        </ElFormItem>

        <ElProgress
          :percentage="uploadProgress"
          :status="uploadStatus"
          style="margin-bottom: 20px"
        />

        <ElButton type="primary" @click="startUpload" :loading="isUploading">
          开始上传
        </ElButton>
      </ElForm>
    </ElCard>
  </div>
</template>

<script setup lang="ts">
import { ref } from "vue";
import { ElMessage } from "element-plus";

interface UploadForm {
  username: string;
  password: string;
}

const form = ref<UploadForm>({
  username: "",
  password: "",
});

const selectedFile = ref<File | null>(null);
const isUploading = ref(false);
const uploadProgress = ref(0);
const uploadStatus = ref<"success" | "exception" | "">("");
const CHUNK_SIZE = 5 * 1024 * 1024; // 5MB

const handleFileSelect = (e: Event) => {
  const input = e.target as HTMLInputElement;
  selectedFile.value = input.files?.[0] || null;
};

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

const startUpload = async () => {
  if (!selectedFile.value || !form.value.username || !form.value.password) {
    ElMessage.warning("请填写完整信息");
    return;
  }

  try {
    isUploading.value = true;
    uploadStatus.value = "";

    // 初始化上传
    const initData = await fetchJSON("/dsp.saves/api/upload/init", {
      method: "POST",
      body: JSON.stringify({
        userName: form.value.username,
        password: form.value.password,
        fileName: selectedFile.value.name,
        fileSize: selectedFile.value.size,
        chunkSize: CHUNK_SIZE,
      }),
    });

    const { fileId, totalChunks } = initData.payload.data;

    // 上传分片
    for (let index = 0; index < totalChunks; index++) {
      const start = index * CHUNK_SIZE;
      const end = Math.min(start + CHUNK_SIZE, selectedFile.value.size);
      const chunk = selectedFile.value.slice(start, end);

      const formData = new FormData();
      formData.append("chunk", chunk);

      const response = await fetch(
        "http://localhost:9999" +
          `/dsp.saves/api/upload/chunk?fileId=${fileId}&chunkIndex=${index}`,
        {
          method: "POST",
          body: formData,
        }
      );

      if (!response.ok) throw new Error("分片上传失败");

      uploadProgress.value = Math.round(((index + 1) / totalChunks) * 100);
    }

    // 完成上传
    await fetchJSON("/dsp.saves/api/upload/complete", {
      method: "POST",
      body: JSON.stringify({ fileId }),
    });

    uploadStatus.value = "success";
    ElMessage.success("上传成功");
  } catch (error) {
    uploadStatus.value = "exception";
    ElMessage.error(error instanceof Error ? error.message : "上传失败");
  } finally {
    isUploading.value = false;
  }
};
</script>

<style scoped>
.upload-container {
  max-width: 600px;
  margin: 20px auto;
}
.box-card {
  padding: 20px;
}
</style>
