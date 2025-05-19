<script setup lang="ts">
import { ref } from "vue";
import FileUploadDemo from "./components/ChunkUploadTest.vue";
import FileDownloadDemo from "./components/FileListDownloadTest.vue";

const showUpload = ref(false);
const toggleView = () => {
  showUpload.value = !showUpload.value;
};
</script>

<template>
  <div class="container">
    <button class="toggle-button" @click="toggleView">
      {{ showUpload ? "切换到下载" : "切换到上传" }}
    </button>

    <keep-alive>
      <component :is="showUpload ? FileUploadDemo : FileDownloadDemo" />
    </keep-alive>
  </div>
</template>

<style scoped>
.container {
  max-width: 1200px;
  margin: 20px auto;
  padding: 20px;
}

.toggle-button {
  margin-bottom: 20px;
  padding: 10px 20px;
  background-color: #007bff;
  color: white;
  border: none;
  border-radius: 4px;
  cursor: pointer;
  transition: background-color 0.3s;
}

.toggle-button:hover {
  background-color: #0056b3;
}

.logo {
  height: 6em;
  padding: 1.5em;
  will-change: filter;
  transition: filter 300ms;
}
.logo:hover {
  filter: drop-shadow(0 0 2em #646cffaa);
}
.logo.vue:hover {
  filter: drop-shadow(0 0 2em #42b883aa);
}
</style>
