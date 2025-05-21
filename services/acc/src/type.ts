// 用户相关接口
declare namespace UserAPI {
  interface UserRequestBody {
    userName: string;
    password: string;
  }

  interface UpdateRequestBody extends UserRequestBody {
    newPassword: string;
  }
}

// 文件上传相关接口
declare namespace UploadAPI {
  interface InitRequestBody extends UserAPI.UserRequestBody {
    fileName: string;
    fileSize: number; // 单位：字节
    chunkSize: number; // 单位：字节
    ttl?: number; // 单位：秒，默认 300
  }

  interface ChunkRequestQuery extends CompleteRequestBody {
    chunkIndex: number; // 通过 URL 参数传递
  }

  interface CompleteRequestBody {
    fileId: string;
  }
}

// 文件下载相关接口
declare namespace DownloadAPI {
  interface FileListRequestBody extends UserAPI.UserRequestBody {
    syncFileExtensions?: string[] | string; // 支持数组或逗号分隔字符串
  }

  interface DownloadRequestBody extends UserAPI.UserRequestBody {
    fileName: string;
    isChunk?: boolean; // 是否启用分块下载
  }

  interface FileInfo {
    fileName: string;
    fileSize: number;
  }
}

// 响应数据结构
declare namespace Response {
  interface InitResponse {
    fileId: string;
    chunkSize: number;
    totalChunks: number;
  }

  interface UploadProgress {
    nots?: number[]; // 缺失的分块索引
    error?: {
      name: string;
      message: string;
    };
  }

  interface FileListResponse {
    files: DownloadAPI.FileInfo[];
  }
}

export { UserAPI, UploadAPI, DownloadAPI, Response };
