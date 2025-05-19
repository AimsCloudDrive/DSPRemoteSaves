import { createApp } from "vue";
import ElementPlus from "element-plus";
import "element-plus/dist/index.css";
import App from "./App.vue";
import App2 from "./App copy.vue";
import router from "./router";
import { createPinia } from "pinia";
import "./style.css";
import { useNotificationStore } from "./stores/notification";

const app = createApp(App2);
app.use(ElementPlus);
app.use(createPinia());
app.use(router);
app.mount("#app");

// src/main.ts
app.config.errorHandler = (err) => {
  const notification = useNotificationStore();
  notification.showError(
    `应用程序错误: ${err instanceof Error ? err.message : "未知错误"}`
  );
};
