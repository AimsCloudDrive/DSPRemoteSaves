using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;
using BepInEx;
using BepInEx.Logging;
using BepInEx.Configuration;
using System.Net.Http;
using System.Net.Http.Json;
using System.Text.Json;
using System.Text.Json.Serialization;
using HarmonyLib;
using System.IO;
using System.Reflection;
using System.Threading.Tasks;
using UnityEngine;

namespace DSPRemoteSaves
{
    [BepInPlugin(GUID, NAME, VERSION)]
    [BepInProcess(GAME_PROCESS)]
    public class RemoteSaves: BaseUnityPlugin
    {
        public const string GUID = "com.aims.remotesaves";
        public const string NAME = "RemoteSaves";
        public const string VERSION = "1.0.0";
        public const string GAME_PROCESS = "DSPGAME.exe";

        private static ConfigEntry<string> configServerURL;
        private static ConfigEntry<string> configUsername;
        private static ConfigEntry<string> configPassword;
        private static ConfigEntry<bool> configDownloadAll;
        private static ConfigEntry<string> configFileExtensions;
        private static ConfigEntry<int> configChunkSize;
        private static ConfigEntry<bool> configEnabled;
        private static ManualLogSource staticLogger;
        private static HttpClient httpClient;
        private static JsonSerializerOptions JsonOptions;

        public void Awake()
        {
            try
            {
                staticLogger = this.Logger;
                staticLogger.LogInfo($"[1/5] 插件开始加载 | BepInEx v{typeof(BaseUnityPlugin).Assembly.GetName().Version}");
                JsonOptions = new JsonSerializerOptions()
                {
                    PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
                    DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
                    WriteIndented = false,
                    Converters = { new JsonStringEnumConverter() }
                };
                staticLogger.LogInfo("RemoteSaves load started!");
                // 验证程序集加载
                staticLogger.LogInfo($"[2/5] 程序集验证: {Assembly.GetExecutingAssembly().Location}");
                // 配置初始化
                InitConfig();
                staticLogger.LogInfo("[3/5] 配置初始化完成");
                // 网络客户端初始化
                httpClient = new HttpClient
                {
                    Timeout = TimeSpan.FromSeconds(30),
                    BaseAddress = new Uri(configServerURL.Value)
                };
                staticLogger.LogInfo("[4/5] HTTP客户端已创建");

                Harmony.CreateAndPatchAll(typeof(RemoteSaves));
                staticLogger.LogInfo("RemoteSaves loaded!");
            }
            catch (Exception ex)
            {
                staticLogger.LogFatal($"插件加载失败: {FormatException(ex)}");
                // 确保异常能被Unity捕获
                throw new InvalidOperationException("Critical plugin failure", ex);
            }

        }

        private void InitConfig()
        {
            staticLogger.LogInfo("configPath: " + Path.Combine(Paths.ConfigPath, GUID));
            configServerURL = Config.Bind("General", "ServerURL", "",
                "Remote server address (e.g. http://example.com:9999/dsp.saves/api)");
            configUsername = Config.Bind("General", "Username", "", "Login username");
            configPassword = Config.Bind("General", "Password", "", "Login password");
            configDownloadAll = Config.Bind("General", "OverwriteAll", false,
                "是否下载云端全部存档（true：全覆盖，false：仅下载本地没有的）");
            configFileExtensions = Config.Bind("General", "FileExtensions", ".dsv,.moddsv,.server",
                "Allowed file extensions (comma separated)");
            configChunkSize = Config.Bind("General", "ChunkSize(MB)", 5, "文件上传时分片大小（单位：MB）");
            configEnabled = Config.Bind("General", "Enabled", true, "Enable mod");
            Config.Save();
        }

        [HarmonyPostfix]
        [HarmonyPatch(typeof(GameMain), "Begin")]
        public static void OnGameStart()
        {
            if (!CheckConfig()) return;
            staticLogger.LogInfo("OnGameStart");
            Task.Run(async () => await DownloadSaves());
        }

        [HarmonyPostfix]
        [HarmonyPatch(typeof(GameMain), "End")]
        public static void OnGameExit()
        {
            if (!CheckConfig()) return;
            Task.Run(async () => await UploadSaves());
        }

        private static bool CheckConfig()
        {
            return configEnabled.Value &&
                   !string.IsNullOrEmpty(configServerURL.Value) &&
                   !string.IsNullOrEmpty(configUsername.Value) &&
                   !string.IsNullOrEmpty(configPassword.Value);
        }

        private static string GetSavePath()
        {
            try
            {
                var gamePath = Path.GetDirectoryName(Application.dataPath);
                var pathConfig = Path.Combine(gamePath, "config", "path");

                if (File.Exists(pathConfig))
                {
                    var customPath = File.ReadAllText(pathConfig).Trim();
                    return Path.Combine(customPath, "Dyson Sphere Program", "Save");
                }
            }
            catch { }

            return Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments),
                "Dyson Sphere Program",
                "Save"
            );
        }

        private static IEnumerable<string> GetFileExtensions()
        {
            return (configFileExtensions.Value ?? "")
                .Split(new[] { ',' }, StringSplitOptions.RemoveEmptyEntries)
                .Select(e => e.Trim().ToLower())
                .DefaultIfEmpty(".dsv");
        }

        #region 数据模型
        public class AuthRequest
        {
            [JsonPropertyName("userName")]
            public string UserName { get; set; }

            [JsonPropertyName("password")]
            public string Password { get; set; }
        }
        public class UploadInitRequest : AuthRequest
        {
            [JsonPropertyName("fileName")]
            public string FileName { get; set; }

            [JsonPropertyName("fileSize")]
            public long FileSize { get; set; }

            [JsonPropertyName("chunkSize")]
            public int ChunkSize { get; set; }
        }
        public class CompleteUploadRequest
        {
            [JsonPropertyName("fileId")]
            public string FileId { get; set; }
        }
        public class FileDownloadRequest : AuthRequest
        {
            [JsonPropertyName("fileName")]
            public string FileName { get; set; }

            [JsonPropertyName("isChunk")]
            public bool IsChunk { get; set; }
        }


        public class BaseResponse
        {
            [JsonPropertyName("code")]
            public int Code { get; set; }

            [JsonPropertyName("message")]
            public string Message { get; set; }
        }

        public class FileListResponse : BaseResponse
        {
            [JsonPropertyName("payload")]
            public FileListPayload Payload { get; set; }
        }

        public class FileListPayload
        {
            [JsonPropertyName("files")]
            public List<RemoteFile> Files { get; set; } = new List<RemoteFile>();
        }

        public class RemoteFile
        {
            [JsonPropertyName("fileName")]
            public string FileName { get; set; }

            [JsonPropertyName("fileSize")]
            public long FileSize { get; set; }
        }

        public class UploadInitResponse : BaseResponse
        {
            [JsonPropertyName("payload")]
            public UploadInitPayload Payload { get; set; }
        }

        public class UploadInitPayload
        {
            [JsonPropertyName("data")]
            public UploadInitData Data { get; set; }
        }

        public class UploadInitData
        {
            [JsonPropertyName("fileId")]
            public string FileId { get; set; }

            [JsonPropertyName("chunkSize")]
            public int ChunkSize { get; set; }

            [JsonPropertyName("totalChunks")]
            public int TotalChunks { get; set; }
        }

        public class UploadCompleteResponse : BaseResponse
        {
            [JsonPropertyName("payload")]
            public UploadCompletePayload Payload { get; set; }
        }

        public class UploadCompletePayload
        {
            [JsonPropertyName("nots")]
            public List<int> Nots { get; set; }

            [JsonPropertyName("error")]
            public ErrorInfo Error { get; set; }
        }

        public class ErrorInfo
        {
            [JsonPropertyName("name")]
            public string Name { get; set; }

            [JsonPropertyName("message")]
            public string Message { get; set; }
        }
        #endregion
        #region 核心逻辑
        private static async Task DownloadSaves()
        {
            try
            {
                // 获取远程文件列表
                var remoteFiles = await GetRemoteFileList();
                staticLogger.LogInfo("GetRemoteFileList");
                if (remoteFiles == null || remoteFiles.Count == 0)
                {
                    staticLogger.LogDebug("服务器没有可用的存档文件");
                    return;
                }

                // 获取本地文件索引（文件名不区分大小写）
                var localFiles = GetLocalFileIndex();
                staticLogger.LogInfo("GetLocalFileIndex");
                // 根据配置策略筛选需要下载的文件
                var filesToDownload = remoteFiles.Where(remoteFile =>
                    ShouldDownloadFile(remoteFile.FileName, localFiles)
                ).ToList();

                if (filesToDownload.Count == 0)
                {
                    staticLogger.LogDebug("没有需要下载的存档文件");
                    return;
                }

                staticLogger.LogDebug($"开始下载 {filesToDownload.Count} 个存档文件...");
                foreach (var file in filesToDownload)
                {
                    await DownloadSingleFile(file.FileName);
                }
            }
            catch (Exception e)
            {
                staticLogger.LogError($"存档下载失败: {FormatException(e)}");
            }
        }

        /// <summary>
        /// 生成本地文件索引（文件名小写格式）
        /// </summary>
        private static Dictionary<string, FileInfo> GetLocalFileIndex()
        {
            var savePath = GetSavePath();
            var index = new Dictionary<string, FileInfo>();

            if (!Directory.Exists(savePath)) return index;

            var extensions = GetFileExtensions();
            foreach (var filePath in Directory.GetFiles(savePath))
            {
                try
                {
                    if (!ShouldProcessFile(filePath, extensions)) continue;

                    var fileInfo = new FileInfo(filePath);
                    var lowerKey = fileInfo.Name.ToLowerInvariant();

                    // 保留最新版本的文件信息
                    if (!index.ContainsKey(lowerKey) ||
                        fileInfo.LastWriteTime > index[lowerKey].LastWriteTime)
                    {
                        index[lowerKey] = fileInfo;
                    }
                }
                catch (Exception e)
                {
                    staticLogger.LogWarning($"无法读取本地文件 {filePath}: {e.Message}");
                }
            }
            return index;
        }

        /// <summary>
        /// 判断是否需要下载文件（考虑文件名大小写差异）
        /// </summary>
        private static bool ShouldDownloadFile(string remoteFileName, Dictionary<string, FileInfo> localIndex)
        {
            // 统一使用小写文件名比较
            var lowerName = remoteFileName.ToLowerInvariant();

            // 配置强制覆盖模式：总是下载
            if (configDownloadAll.Value) return true;

            // 本地不存在该文件：需要下载
            if (!localIndex.ContainsKey(lowerName)) return true;

            // 本地存在同名文件但大小不同：建议下载（可选逻辑）
            var localFile = localIndex[lowerName];
            var remoteFile = localIndex[lowerName]; // 此处需要远程文件大小信息

            // 注意：需要确保GetRemoteFileList返回的文件包含size信息
            // if (remoteFile.fileSize != localFile.Length) return true;

            // 默认逻辑：本地存在则跳过
            return false;
        }
        private static async Task<bool> DownloadSingleFile(string fileName)
        {
            try
            {
                // 使用强类型模型
                var payload = new FileDownloadRequest
                {
                    UserName = configUsername.Value,
                    Password = configPassword.Value,
                    FileName = fileName,
                    IsChunk = false
                };

                using var response = await PostJson("/download/download", payload);
                if (!response.IsSuccessStatusCode) return false;

                var savePath = Path.Combine(GetSavePath(), fileName);
                using var fileStream = File.Create(savePath);
                await response.Content.CopyToAsync(fileStream);
                return true;
            }
            catch (Exception e)
            {
                staticLogger.LogError($"文件下载失败 ({fileName}): {FormatException(e)}");
                return false;
            }
        }


        private static async Task<List<RemoteFile>> GetRemoteFileList()
        {
            try
            {
                var payload = new AuthRequest
                {
                    UserName = configUsername.Value,
                    Password = configPassword.Value
                };

                using var response = await PostJson("/download/file-list", payload);
                if (!response.IsSuccessStatusCode) return null;

                var result = await response.Content.ReadFromJsonAsync<FileListResponse>(JsonOptions);

                switch (result?.Code)
                {
                    case 0:
                        return result.Payload?.Files ?? new List<RemoteFile>();
                    default:
                        await HandleErrorResponse(response);
                        return null;
                }
            }
            catch (Exception e)
            {
                staticLogger.LogError($"获取文件列表失败: {FormatException(e)}");
                return null;
            }
        }
        private static async Task UploadSaves()
        {
            try
            {
                var savePath = GetSavePath();
                if (!Directory.Exists(savePath)) return;

                var files = Directory.GetFiles(savePath)
                    .Where(f => ShouldProcessFile(f, GetFileExtensions()))
                    .Select(Path.GetFileName);

                foreach (var file in files)
                {
                    await UploadSingleFile(Path.Combine(savePath, file));
                }
            }
            catch (Exception e)
            {
                staticLogger.LogError($"存档上传失败: {FormatException(e)}");
            }
        }

        private static async Task<bool> UploadSingleFile(string filePath)
        {
            try
            {
                var fileName = Path.GetFileName(filePath);
                var fileInfo = new FileInfo(filePath);

                // 初始化上传
                var (fileId, chunkSize) = await InitUpload(fileName, fileInfo.Length);
                if (string.IsNullOrEmpty(fileId)) return false;

                // 上传分片
                if (!await UploadChunks(filePath, fileId, chunkSize)) return false;

                // 完成上传
                return await CompleteUpload(fileId);
            }
            catch (Exception e)
            {
                staticLogger.LogError($"文件上传失败 ({Path.GetFileName(filePath)}): {FormatException(e)}");
                return false;
            }
        }

        private static async Task<(string FileId, int ChunkSize)> InitUpload(string fileName, long fileSize)
        {
            try
            {
                // 使用强类型模型代替匿名类型
                var initPayload = new UploadInitRequest
                {
                    UserName = configUsername.Value,
                    Password = configPassword.Value,
                    FileName = fileName,
                    FileSize = fileSize,
                    ChunkSize = configChunkSize.Value * 1024 * 1024
                };

                using var response = await PostJson("/upload/init", initPayload);
                var result = await response.Content.ReadFromJsonAsync<UploadInitResponse>(JsonOptions);

                return result?.Code == 0
                    ? (result.Payload.Data.FileId, result.Payload.Data.ChunkSize)
                    : (null, 0);
            }
            catch (Exception e)
            {
                staticLogger.LogError($"上传初始化失败: {FormatException(e)}");
                return (null, 0);
            }
        }
        private static async Task<bool> UploadChunks(string filePath, string fileId, int chunkSize)
        {
            try
            {
                using var fileStream = File.OpenRead(filePath);
                var buffer = new byte[chunkSize];
                int chunkIndex = 0;

                while (fileStream.Position < fileStream.Length)
                {
                    var bytesRead = await fileStream.ReadAsync(buffer, 0, buffer.Length);
                    using var chunkContent = new ByteArrayContent(buffer, 0, bytesRead);

                    var url = $"/upload/chunk?fileId={fileId}&chunkIndex={chunkIndex}";
                    using var response = await httpClient.PostAsync(configServerURL.Value + url, chunkContent);

                    if (!response.IsSuccessStatusCode)
                    {
                        await HandleErrorResponse(response);
                        return false;
                    }

                    chunkIndex++;
                }
                return true;
            }
            catch (Exception e)
            {
                staticLogger.LogError($"分片上传失败: {FormatException(e)}");
                return false;
            }
        }
        private static async Task<bool> CompleteUpload(string fileId)
        {
            try
            {
                // 使用强类型代替匿名类型
                var payload = new CompleteUploadRequest { FileId = fileId };

                using var response = await PostJson("/upload/complete", payload);
                var result = await response.Content.ReadFromJsonAsync<UploadCompleteResponse>(JsonOptions);

                switch (result?.Code)
                {
                    case 0:
                        return true;
                    case 1 when result.Payload?.Nots != null:
                        staticLogger.LogError($"缺失分片: {string.Join(", ", result.Payload.Nots)}");
                        return false;
                    default:
                        await HandleErrorResponse(response);
                        return false;
                }
            }
            catch (Exception e)
            {
                staticLogger.LogError($"上传完成失败: {FormatException(e)}");
                return false;
            }
        }
        #endregion

        #region 工具方法
        private static async Task HandleErrorResponse(HttpResponseMessage response)
        {
            try
            {
                var errorMessage = new StringBuilder()
                    .AppendLine($"HTTP错误: {response.StatusCode} ({(int)response.StatusCode})");

                // 尝试读取错误详情
                try
                {
                    var errorContent = await response.Content.ReadFromJsonAsync<BaseResponse>(JsonOptions);
                    errorMessage.AppendLine($"错误码: {errorContent?.Code}")
                               .AppendLine($"错误信息: {errorContent?.Message}");

                    // 处理特殊错误类型
                    if (errorContent is UploadCompleteResponse uploadError)
                    {
                        if (uploadError.Payload?.Nots?.Count > 0)
                            errorMessage.AppendLine($"缺失分片索引: {string.Join(", ", uploadError.Payload.Nots)}");

                        if (uploadError.Payload?.Error != null)
                            errorMessage.AppendLine($"服务端异常: {uploadError.Payload.Error.Message}");
                    }
                }
                catch (JsonException jsonEx)
                {
                    var rawContent = await response.Content.ReadAsStringAsync();
                    errorMessage.AppendLine($"原始响应内容: {rawContent}")
                               .AppendLine($"JSON解析失败: {jsonEx.Message}");
                }

                staticLogger.LogError(errorMessage.ToString());
            }
            catch (Exception ex)
            {
                staticLogger.LogError($"错误处理过程中发生异常: {FormatException(ex)}");
            }
        }
        private static string FormatException(Exception e)
        {
            return $"{e.GetType().Name}: {e.Message}\nStackTrace: {e.StackTrace}";
        }

        private static async Task<HttpResponseMessage> PostJson<T>(string endpoint, T payload)
        {
            try
            {
                staticLogger.LogDebug($"Sending to {endpoint}: {JsonSerializer.Serialize(payload)}");
                return await httpClient.PostAsJsonAsync(
                    configServerURL.Value + endpoint,
                    payload,
                    JsonOptions
                );
            }
            catch (TaskCanceledException)
            {
                staticLogger.LogError("请求超时");
                throw;
            }
            catch (HttpRequestException e)
            {
                staticLogger.LogError($"网络请求失败: {e.Message}");
                throw;
            }
        }

        // 其他工具方法（GetSavePath、ShouldProcessFile等）保持不变...
        private static bool ShouldProcessFile(string fileName, IEnumerable<string> extensions)
        {
            var ext = Path.GetExtension(fileName).ToLower();
            return extensions.Contains(ext);
        }

        #endregion

        [ExecuteInEditMode]
        [RuntimeInitializeOnLoadMethod(RuntimeInitializeLoadType.AfterSceneLoad)]
        public static void ForceInitialization()
        {
            var instance = FindObjectOfType<RemoteSaves>();
            if (instance == null)
            {
                new GameObject("RemoteSaves_Loader")
                    .AddComponent<RemoteSaves>();
            }
        }

    }
}
