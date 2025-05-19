import { useNotificationStore } from "../stores/notification";

// src/api/index.ts
const API_BASE = "http://localhost:9999/dsp.saves/api";

interface FileInfo {
  fileName: string;
  fileSize: number;
}

interface InitUploadResponse {
  data: {
    fileId: string;
    chunkSize: number;
    totalChunks: number;
  };
}

interface ApiResponse<T = any> {
  code: number;
  message?: string;
  payload?: T;
}

async function fetchApi<T = any>(
  endpoint: string,
  options: RequestInit = {},
  body?: any
): Promise<ApiResponse<T>> {
  const notification = useNotificationStore();
  let loadingInstance: ReturnType<typeof notification.showLoading> | undefined;
  let showLoadingTimer: number | undefined;
  try {
    const url = `${API_BASE}${endpoint}`;

    const headers = new Headers(options.headers || {});
    if (body && !(body instanceof FormData)) {
      headers.set("Content-Type", "application/json");
    }
    showLoadingTimer = setTimeout(() => {
      loadingInstance = notification.showLoading();
    }, 500);

    const response = await fetch(url, {
      ...options,
      headers,
      body: body instanceof FormData ? body : JSON.stringify(body),
    });
    clearTimeout(showLoadingTimer);
    loadingInstance?.close();
    if (!response.ok) {
      throw new Error(`HTTP error! status: ${response.status}`);
    }

    const result: ApiResponse = await response.json();

    // 根据业务code显示提示
    if (result.code !== 0) {
      result.message && notification.showError(result.message);
    }
    return result;
  } catch (error) {
    showLoadingTimer && clearTimeout(showLoadingTimer);
    loadingInstance?.close();

    const errorMessage =
      error instanceof Error ? error.message : "网络请求失败";
    notification.showError(errorMessage);

    return { code: -1, message: errorMessage };
  }
}

export default {
  // 用户注册
  async register(userName: string, password: string) {
    return fetchApi(
      "/users/create",
      { method: "POST" },
      { userName, password }
    );
  },

  // 用户登录
  async login(userName: string, password: string) {
    return fetchApi("/users/login", { method: "POST" }, { userName, password });
  },
  // 用户登出
  async logout(userName: string, password: string) {
    return fetchApi(
      "/users/logout",
      { method: "POST" },
      { userName, password }
    );
  },

  // 修改密码
  async updatePassword(
    userName: string,
    password: string,
    newPassword: string
  ) {
    return fetchApi(
      "/users/update",
      { method: "POST" },
      { userName, password, newPassword }
    );
  },

  // 删除用户
  async deleteUser(userName: string, password: string) {
    return fetchApi(
      "/users/delete",
      { method: "POST" },
      { userName, password }
    );
  },

  // 初始化上传
  async initUpload(
    fileName: string,
    fileSize: number,
    chunkSize: number,
    userName: string,
    password: string
  ) {
    return fetchApi<InitUploadResponse>(
      "/upload/init",
      { method: "POST" },
      { fileName, fileSize, chunkSize, userName, password }
    );
  },

  // 上传分片
  async uploadChunk(
    fileId: string,
    chunkIndex: number,
    chunk: Blob,
    userName: string,
    password: string
  ) {
    const formData = new FormData();
    formData.append("chunk", chunk);

    return fetchApi(
      `/upload/chunk?fileId=${fileId}&chunkIndex=${chunkIndex}&userName=${userName}&password=${password}`,
      { method: "POST", body: formData }
    );
  },

  // 完成上传
  async completeUpload(fileId: string, userName: string, password: string) {
    return fetchApi(
      "/upload/complete",
      { method: "POST" },
      { fileId, userName, password }
    );
  },

  // 获取文件列表
  async getFileList(userName: string, password: string) {
    return fetchApi<{ files: FileInfo[] }>(
      "/download/file-list",
      { method: "POST" },
      { userName, password }
    );
  },

  // 下载文件
  async downloadFile(fileName: string, userName: string, password: string) {
    try {
      const response = await fetch(`${API_BASE}/download/download`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ fileName, userName, password }),
      });

      if (!response.ok) throw new Error("Download failed");
      return {
        code: 0,
        payload: await response.blob(), // 将数据存放在payload字段
      };
    } catch (error) {
      console.error("Download failed:", error);
      return { code: -1 };
    }
  },
};
