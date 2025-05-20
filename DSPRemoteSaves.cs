using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;
using BepInEx;

using BepInEx.Configuration;
using System.Net.Http;
using HarmonyLib;
using System.IO;
using System.Reflection;
using System.Threading.Tasks;
using UnityEngine;
using System.Collections;
using System.Threading;
using System.Collections.Concurrent;

namespace DSPRemoteSaves
{
    [BepInPlugin(GUID, NAME, VERSION)]
    [BepInProcess(GAME_PROCESS)]
    public class DSPRemoteSaves : BaseUnityPlugin
    {
        public const string GUID = "com.aims.remotesaves";
        public const string NAME = "RemoteSaves";
        public const string VERSION = "1.0.0";
        public const string GAME_PROCESS = "DSPGAME.exe";

        private ConfigEntry<string> configServerURL;
        private ConfigEntry<string> configUsername;
        private ConfigEntry<string> configPassword;
        private ConfigEntry<bool> configDownloadAll;
        private ConfigEntry<string> configFileExtensions;
        private ConfigEntry<int> configChunkSize;
        private ConfigEntry<bool> configEnabled;
        private ConfigEntry<int> configMaxParallelDownloadCount;
        private ConfigEntry<int> configMaxParallelUploadCount;
        private ConfigEntry<int> configMaxChunkParallelUploadCount;
        private HttpClient httpClient;

        public void Awake()
        {
            try
            {
                // 配置初始化
                InitConfig();
                // 网络客户端初始化
                if (!CheckConfig()) {
                    return;
                }
                httpClient = new HttpClient
                {
                    Timeout = TimeSpan.FromSeconds(30),
                    BaseAddress = new Uri(configServerURL.Value + "/dsp.saves/api")
                };
                Harmony.CreateAndPatchAll(typeof(DSPRemoteSaves));
            }
            catch (Exception ex)
            {
                Debug.LogError("插件加载失败");
                Debug.LogException(ex);
                // 确保异常能被Unity捕获
                throw new InvalidOperationException("Critical plugin failure", ex);
            }
        }

        private void InitConfig()
        {
            configEnabled = Config.Bind("General", "Enabled", true, "Enable mod");

            configServerURL = Config.Bind("Server", "Remote ServerURL", "",
                "Remote server address (e.g. http://example.com:9999)");

            configUsername = Config.Bind("User", "Username", "", "Remote UserName");
            configPassword = Config.Bind("User", "Password", "", "Remote password");

            configDownloadAll = Config.Bind("Performance", "DownloadAll", false,
                "是否下载云端全部存档（true：全覆盖，false：仅下载本地没有的）");
            configFileExtensions = Config.Bind("Performance", "FileExtensions", ".dsv,.moddsv,.server",
                "上传文件的拓展名(用英文逗号分割)");
            configMaxParallelUploadCount = Config.Bind("Performance", "MaxParallelUploadCount", 3, new ConfigDescription("上传时最大文件并发数", new AcceptableValueRange<int>(1, 3)));
            configChunkSize = Config.Bind("Performance", "ChunkSize(MB)", 5, new ConfigDescription("上传时每个分片大小（单位：MB）", new AcceptableValueRange<int>(5, 50)));
            configMaxChunkParallelUploadCount = Config.Bind("Performance", "MaxChunkParallelUploadCount", 5, new ConfigDescription("上传分片时最大并发分片数", new AcceptableValueRange<int>(1, 10)));
            configMaxParallelDownloadCount = Config.Bind("Performance", "MaxParallelDownloadCount", 5, new ConfigDescription("下载时最大并发下载数", new AcceptableValueRange<int>(1, 10)));
            Config.Save();
        }

        //[HarmonyPostfix]
        //[HarmonyPatch(typeof(GameMain), "Begin")]
        public void Start()
        {
            if (!CheckConfig()) return;
            Debug.Log("OnGameStart");
            // 同步执行下载任务（带超时机制）
            var uploadTask = Task.Run(() => DownloadSaves());
            try
            {
                uploadTask.Wait(TimeSpan.FromSeconds(30)); // 最多等待30秒
            }
            catch (AggregateException ae)
            {
                foreach (var e in ae.InnerExceptions)
                {
                    Debug.LogError($"下载终止: {FormatException(e)}");
                }
            }
        }

        //[HarmonyPostfix]
        //[HarmonyPatch(typeof(GameMain), "End")]
        public void OnDestroy()
        {
            if (!CheckConfig()) return;
            Debug.Log("OnGameEnd");
            return;

            // 同步执行上传任务（带超时机制）
            var uploadTask = Task.Run(() => UploadSaves());
            try
            {
                uploadTask.Wait(TimeSpan.FromSeconds(30)); // 最多等待30秒
            }
            catch (AggregateException ae)
            {
                foreach (var e in ae.InnerExceptions)
                {
                    Debug.LogError($"上传终止: {FormatException(e)}");
                }
            }
        }

        private bool CheckConfig()
        {
            return configEnabled.Value &&
                   !string.IsNullOrEmpty(configServerURL.Value) &&
                   !string.IsNullOrEmpty(configUsername.Value) &&
                   !string.IsNullOrEmpty(configPassword.Value);
        }

        private string GetSavePath()
        {
            string savePath = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments),
                "Dyson Sphere Program",
                "Save"
            );

            try
            {
                var gamePath = Path.GetDirectoryName(Application.dataPath);
                var pathConfig = Path.Combine(gamePath, "Configs", "path.txt");

                if (File.Exists(pathConfig))
                {
                    var customPath = File.ReadAllText(pathConfig).Trim();
                    savePath = Path.Combine(customPath, "Dyson Sphere Program", "Save");
                }
            }
            catch { }
            return savePath;
        }

        private IEnumerable<string> GetFileExtensions()
        {
            return (configFileExtensions.Value ?? "")
                .Split(new[] { ',' }, StringSplitOptions.RemoveEmptyEntries)
                .Select(e => e.Trim().ToLower())
                .DefaultIfEmpty(".dsv");
        }
        #region 数据模型
        public class AuthRequest
        {
            [JsonProperty("userName")]
            public string UserName { get; set; }

            [JsonProperty("password")]
            public string Password { get; set; }
        }
        public class UploadInitRequest : AuthRequest
        {
            [JsonProperty("fileName")]
            public string FileName { get; set; }

            [JsonProperty("fileSize")]
            public long FileSize { get; set; }

            [JsonProperty("chunkSize")]
            public int ChunkSize { get; set; }
        }

        public class CompleteUploadRequest
        {
            [JsonProperty("fileId")]
            public string FileId { get; set; }
        }

        public class FileDownloadRequest : AuthRequest
        {
            [JsonProperty("fileName")]
            public string FileName { get; set; }

            [JsonProperty("isChunk")]
            public bool IsChunk { get; set; }
        }

        public class BaseResponse
        {
            [JsonProperty("code")]
            public int Code { get; set; }

            [JsonProperty("message")]
            public string Message { get; set; }
        }

        public class FileListResponse : BaseResponse
        {
            [JsonProperty("payload")]
            public FileListPayload Payload { get; set; }
        }

        public class FileListPayload
        {
            [JsonProperty("files")]
            public List<RemoteFile> Files { get; set; } = new List<RemoteFile>();
        }

        public class RemoteFile
        {
            [JsonProperty("fileName")]
            public string FileName { get; set; }

            [JsonProperty("fileSize")]
            public long FileSize { get; set; }
        }

        public class UploadInitResponse : BaseResponse
        {
            [JsonProperty("payload")]
            public UploadInitPayload Payload { get; set; }
        }

        public class UploadInitPayload
        {
            [JsonProperty("data")]
            public UploadInitData Data { get; set; }
        }

        public class UploadInitData
        {
            [JsonProperty("fileId")]
            public string FileId { get; set; }

            [JsonProperty("chunkSize")]
            public int ChunkSize { get; set; }

            [JsonProperty("totalChunks")]
            public int TotalChunks { get; set; }
        }

        public class UploadCompleteResponse : BaseResponse
        {
            [JsonProperty("payload")]
            public UploadCompletePayload Payload { get; set; }
        }

        public class UploadCompletePayload
        {
            [JsonProperty("nots")]
            public List<int> Nots { get; set; }

            [JsonProperty("error")]
            public ErrorInfo Error { get; set; }
        }

        public class ErrorInfo
        {
            [JsonProperty("name")]
            public string Name { get; set; }

            [JsonProperty("message")]
            public string Message { get; set; }
        }
        #endregion
        #region 核心逻辑
        private async Task DownloadSaves()
        {
            try
            {
                // 获取远程文件列表
                var remoteFiles = await GetRemoteFileList();
                Debug.Log("GetRemoteFileList" + new CustomJsonSerializer().Serialize(remoteFiles));
                if (remoteFiles == null || remoteFiles.Count == 0)
                {
                    Debug.Log("服务器没有可用的存档文件");
                    return;
                }

                // 获取本地文件索引（文件名不区分大小写）
                var localFiles = GetLocalFileIndex();
                Debug.Log("GetLocalFileIndex");
                // 根据配置策略筛选需要下载的文件
                var filesToDownload = remoteFiles.Where(remoteFile =>
                    ShouldDownloadFile(remoteFile.FileName, localFiles)
                ).ToList();

                if (filesToDownload.Count == 0)
                {
                    Debug.Log("没有需要下载的存档文件");
                    return;
                }

                Debug.Log($"开始下载 {filesToDownload.Count} 个存档文件...");

                // 控制最大并发数
                var semaphore = new SemaphoreSlim(configMaxParallelDownloadCount.Value);
                // 创建并发下载任务集合
                var downloadTasks = filesToDownload.Select(async file =>
                {
                    await semaphore.WaitAsync();
                    try
                    {
                        Debug.Log($"正在下载文件：{file.FileName}");

                        // 调用实际下载方法（需实现DownloadFileAsync）
                        await DownloadSingleFile(file.FileName);

                        Debug.Log($"成功下载文件：{file.FileName}");
                    }
                    catch (Exception ex)
                    {
                        Debug.LogError($"下载文件 {file.FileName} 时出错：{FormatException(ex)}");
                    }
                    finally { semaphore.Release(); }
                }).ToList();

                // 等待所有下载任务完成
                await Task.WhenAll(downloadTasks);
                Debug.Log("所有文件下载完成");
            }
            catch (Exception e)
            {
                Debug.LogError($"存档下载失败: {FormatException(e)}");
            }
        }

        /// <summary>
        /// 生成本地文件索引（文件名小写格式）
        /// </summary>
        private Dictionary<string, FileInfo> GetLocalFileIndex()
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
                    Debug.LogWarning($"无法读取本地文件 {filePath}: {e.Message}");
                }
            }
            return index;
        }

        /// <summary>
        /// 判断是否需要下载文件（考虑文件名大小写差异）
        /// </summary>
        private bool ShouldDownloadFile(string remoteFileName, Dictionary<string, FileInfo> localIndex)
        {
            // 统一使用小写文件名比较
            var lowerName = remoteFileName.ToLowerInvariant();

            // 配置强制覆盖模式：总是下载
            if (configDownloadAll.Value) return true;

            // 本地不存在该文件：需要下载
            if (!localIndex.ContainsKey(lowerName)) return true;

            // 本地存在同名文件但大小不同：建议下载（可选逻辑）
            //var localFile = localIndex[lowerName];
            //var remoteFile = localIndex[lowerName]; // 此处需要远程文件大小信息

            // 注意：需要确保GetRemoteFileList返回的文件包含size信息
            // if (remoteFile.fileSize != localFile.Length) return true;

            // 默认逻辑：本地存在则跳过
            return false;
        }
        private async Task<bool> DownloadSingleFile(string fileName)
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
                Debug.Log($"下载存档{payload.UserName}/{payload.FileName}...");

                using var response = await PostJson("/download/download", payload);
                if (!response.IsSuccessStatusCode) return false;

                var savePath = Path.Combine(GetSavePath(), fileName);
                using var fileStream = File.Create(savePath);
                await response.Content.CopyToAsync(fileStream);
                Debug.Log($"下载完成{payload.UserName}/{payload.FileName}...");
                return true;
            }
            catch (Exception e)
            {
                Debug.LogError($"文件下载失败 ({fileName}): {FormatException(e)}");
                return false;
            }
        }


        private async Task<List<RemoteFile>> GetRemoteFileList()
        {
            try
            {
                var payload = new AuthRequest
                {
                    UserName = configUsername.Value,
                    Password = configPassword.Value
                };

                using var response = await PostJson("/download/file-list", payload);
                if (!response.IsSuccessStatusCode)
                {

                    Debug.Log("请求失败");
                    return null;
                };

                var responseString = await response.Content.ReadAsStringAsync();
                var result = new CustomJsonSerializer().Deserialize<FileListResponse>(responseString);

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
                Debug.LogError($"获取文件列表失败: {FormatException(e)}");
                return null;
            }
        }
        private async Task UploadSaves()
        {
            try
            {
                var savePath = GetSavePath();
                if (!Directory.Exists(savePath)) return;

                var files = Directory.GetFiles(savePath)
                    // 扩展名匹配
                    .Where(f => ShouldProcessFile(f, GetFileExtensions()))
                    .ToList(); // 立即执行避免延迟

                Debug.Log($"开始上传 {files.Count} 个存档文件...");

                // 添加并行上传支持
                var semaphore = new SemaphoreSlim(configMaxParallelUploadCount.Value);
                var uploadTasks = files.Select(async filePath =>
                {
                    await semaphore.WaitAsync();
                    try
                    {
                        await UploadSingleFile(filePath);
                    }
                    finally
                    {
                        semaphore.Release();
                    }
                });

                await Task.WhenAll(uploadTasks);
            }
            catch (Exception e)
            {
                Debug.LogError($"存档上传失败: {FormatException(e)}");
            }
        }

        private async Task<bool> UploadSingleFile(string filePath)
        {
            try
            {
                var fileName = Path.GetFileName(filePath);
                var fileInfo = new FileInfo(filePath);

                // 初始化上传
                var (fileId, chunkSize) = await InitUpload(fileName, fileInfo.Length).ConfigureAwait(false);
                if (string.IsNullOrEmpty(fileId)) return false;

                // 上传分片
                if (!await UploadChunks(filePath, fileId, chunkSize)) return false;

                // 完成上传
                return await CompleteUpload(fileId).ConfigureAwait(false);
            }
            catch (Exception e)
            {
                Debug.LogError($"文件上传失败 ({Path.GetFileName(filePath)}): {FormatException(e)}");
                return false;
            }
        }
        private async Task<(string FileId, int ChunkSize)> InitUpload(string fileName, long fileSize)
        {
            try
            {
                var initPayload = new UploadInitRequest
                {
                    UserName = configUsername.Value,
                    Password = configPassword.Value,
                    FileName = fileName,
                    FileSize = fileSize,
                    ChunkSize = configChunkSize.Value * 1024 * 1024
                };

                using var response = await PostJson("/upload/init", initPayload);
                var responseString = await response.Content.ReadAsStringAsync();
                var result = new CustomJsonSerializer().Deserialize<UploadInitResponse>(responseString);

                return result?.Code == 0
                    ? (result.Payload.Data.FileId, result.Payload.Data.ChunkSize)
                    : (null, 0);
            }
            catch (Exception e)
            {
                Debug.LogError($"上传初始化失败: {FormatException(e)}");
                return (null, 0);
            }
        }
        private async Task<bool> UploadChunks(string filePath, string fileId, int chunkSize)
        {
            try
            {
                var fileInfo = new FileInfo(filePath);
                long fileLength = fileInfo.Length;
                int totalChunks = (int)Math.Ceiling(fileLength / (double)chunkSize);

                // 生成分片元数据列表
                var chunkMetadata = Enumerable.Range(0, totalChunks)
                    .Select(i => new ChunkMeta
                    {
                        Index = i,
                        StartPos = i * (long)chunkSize,
                        ChunkLength = (int)Math.Min(chunkSize, fileLength - i * (long)chunkSize)
                    })
                    .ToList();

                // 并行处理配置
                var semaphore = new SemaphoreSlim(configMaxChunkParallelUploadCount.Value);
                var failedChunks = new ConcurrentBag<int>();
                var cts = new CancellationTokenSource();

                // 并行上传任务
                var uploadTasks = chunkMetadata.Select(async meta =>
                {
                    await semaphore.WaitAsync(cts.Token);
                    try
                    {
                        if (cts.IsCancellationRequested)
                            return;

                        // 独立文件流读取避免并发问题
                        using (var fs = new FileStream(filePath, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, FileOptions.Asynchronous))
                        {
                            var buffer = new byte[meta.ChunkLength];
                            fs.Seek(meta.StartPos, SeekOrigin.Begin);
                            var bytesRead = await fs.ReadAsync(buffer, 0, meta.ChunkLength, cts.Token);

                            if (bytesRead != meta.ChunkLength)
                            {
                                Debug.LogError($"分片{meta.Index}读取不完整 ({bytesRead}/{meta.ChunkLength} bytes)");
                                failedChunks.Add(meta.Index);
                                cts.Cancel();
                                return;
                            }

                            using var content = new ByteArrayContent(buffer, 0, bytesRead);
                            var response = await httpClient.PostAsync(
                                $"/upload/chunk?fileId={fileId}&chunkIndex={meta.Index}",
                                content,
                                cts.Token
                            );

                            if (!response.IsSuccessStatusCode)
                            {
                                await HandleErrorResponse(response);
                                failedChunks.Add(meta.Index);
                                cts.Cancel();
                            }
                        }
                    }
                    catch (OperationCanceledException)
                    {
                        // 忽略取消请求
                    }
                    catch (Exception ex)
                    {
                        Debug.LogError($"分片{meta.Index}上传失败: {ex.Message}");
                        failedChunks.Add(meta.Index);
                        cts.Cancel();
                    }
                    finally
                    {
                        semaphore.Release();
                    }
                });

                try
                {
                    await Task.WhenAll(uploadTasks);
                }
                catch
                {
                    // 具体错误已在各任务中处理
                }

                if (!failedChunks.IsEmpty)
                {
                    Debug.LogError($"以下分片上传失败: {string.Join(", ", failedChunks)}");
                    return false;
                }

                Debug.Log($"成功上传 {totalChunks} 个分片");
                return true;
            }
            catch (Exception e)
            {
                Debug.LogError($"分片上传过程异常: {FormatException(e)}");
                return false;
            }
        }

        private class ChunkMeta
        {
            public int Index { get; set; }
            public long StartPos { get; set; }
            public int ChunkLength { get; set; }
        }
        private async Task<bool> CompleteUpload(string fileId)
        {
            try
            {
                var payload = new CompleteUploadRequest { FileId = fileId };
                using var response = await PostJson("/upload/complete", payload);
                var responseString = await response.Content.ReadAsStringAsync();
                var result = new CustomJsonSerializer().Deserialize<UploadCompleteResponse>(responseString);

                switch (result?.Code)
                {
                    case 0:
                        return true;
                    case 1 when result.Payload?.Nots != null:
                        Debug.LogError($"缺失分片: {string.Join(", ", result.Payload.Nots)}");
                        return false;
                    default:
                        await HandleErrorResponse(response);
                        return false;
                }
            }
            catch (Exception e)
            {
                Debug.LogError($"上传完成失败: {FormatException(e)}");
                return false;
            }
        }
        #endregion

        #region 工具方法
        private async Task HandleErrorResponse(HttpResponseMessage response)
        {
            try
            {
                var errorMessage = new StringBuilder()
                    .AppendLine($"HTTP错误: {response.StatusCode} ({(int)response.StatusCode})");

                var responseString = await response.Content.ReadAsStringAsync();
                try
                {
                    var errorResponse = new CustomJsonSerializer().Deserialize<BaseResponse>(responseString);
                    errorMessage.AppendLine($"错误码: {errorResponse?.Code}")
                               .AppendLine($"错误信息: {errorResponse?.Message}");

                    if (errorResponse is UploadCompleteResponse uploadError)
                    {
                        if (uploadError.Payload?.Nots?.Count > 0)
                            errorMessage.AppendLine($"缺失分片索引: {string.Join(", ", uploadError.Payload.Nots)}");

                        if (uploadError.Payload?.Error != null)
                            errorMessage.AppendLine($"服务端异常: {uploadError.Payload.Error.Message}");
                    }
                }
                catch (Exception jsonEx)
                {
                    errorMessage.AppendLine($"原始响应内容: {responseString}")
                               .AppendLine($"JSON解析失败: {jsonEx.Message}");
                }

                Debug.LogError(errorMessage.ToString());
            }
            catch (Exception ex)
            {
                Debug.LogError($"错误处理过程中发生异常: {FormatException(ex)}");
            }
        }

        private async Task<HttpResponseMessage> PostJson<T>(string endpoint, T payload)
        {
            try
            {
                var json = new CustomJsonSerializer().Serialize(payload);
                var content = new StringContent(json, Encoding.UTF8, "application/json");

                Debug.Log($"Sending to {endpoint}: {json}");
                return await httpClient.PostAsync(httpClient.BaseAddress + endpoint, content);
            }
            catch (TaskCanceledException)
            {
                Debug.LogError("请求超时");
                throw new Exception("请求超时");
            }
            catch (HttpRequestException e)
            {
                Debug.LogError($"网络请求失败: {e.Message}");
                throw new Exception("网络请求失败");
            }
        }
        private string FormatException(Exception e)
        {
            return $"{e.GetType().Name}: {e.Message}\nStackTrace: {e.StackTrace}";
        }

        // 其他工具方法（GetSavePath、ShouldProcessFile等）保持不变...
        private bool ShouldProcessFile(string fileName, IEnumerable<string> extensions)
        {
            var ext = Path.GetExtension(fileName).ToLower();
            return extensions.Contains(ext);
        }

        // 自定义特性类
        [AttributeUsage(AttributeTargets.Property)]
        public class JsonPropertyAttribute : Attribute
        {
            public string Name { get; set; }
            public bool IgnoreNull { get; set; }  // 是否忽略空值

            public JsonPropertyAttribute(string name)
            {
                Name = name;
                IgnoreNull = true;
            }
        }

        public class CustomJsonSerializer
        {
            // 序列化入口
            public string Serialize(object obj)
            {
                var sb = new StringBuilder();
                SerializeValue(obj, sb, 0);
                return sb.ToString();
            }

            // 反序列化入口
            public T Deserialize<T>(string json) where T : new()
            {
                int index = 0;
                return (T)ParseValue(json, ref index, typeof(T));
            }

            #region 序列化逻辑
            private void SerializeValue(object value, StringBuilder sb, int depth)
            {
                if (value == null)
                {
                    sb.Append("null");
                    return;
                }

                Type type = value.GetType();

                // 处理字符串
                if (type == typeof(string))
                {
                    sb.Append($"\"{EscapeString((string)value)}\"");
                }
                // 处理数值类型
                else if (type.IsPrimitive || type.IsEnum || type == typeof(decimal))
                {
                    sb.Append(value.ToString().ToLower());
                }
                // 处理字典
                else if (IsDictionary(type))
                {
                    SerializeDictionary((IDictionary)value, sb, depth);
                }
                // 处理集合
                else if (IsEnumerable(type))
                {
                    SerializeArray((IEnumerable)value, sb, depth);
                }
                // 处理普通对象
                else
                {
                    SerializeObject(value, sb, depth);
                }
            }

            #region 修改后的序列化对象方法（支持JsonProperty）
            private void SerializeObject(object obj, StringBuilder sb, int depth)
            {
                sb.Append("{");
                var properties = obj.GetType().GetProperties();
                bool first = true;

                foreach (var prop in properties)
                {
                    if (!prop.CanRead) continue;

                    // 获取JsonProperty特性
                    var jsonProp = prop.GetCustomAttribute<JsonPropertyAttribute>();
                    var propName = jsonProp?.Name ?? prop.Name;

                    var value = prop.GetValue(obj);
                    if (first) first = false;
                    else sb.Append(",");

                    sb.Append($"\"{propName}\":");
                    SerializeValue(value, sb, depth + 1);
                }
                sb.Append("}");
            }
            #endregion

            // 序列化数组/集合
            private void SerializeArray(IEnumerable collection, StringBuilder sb, int depth)
            {
                sb.Append("[");
                bool first = true;

                foreach (var item in collection)
                {
                    if (first) first = false;
                    else sb.Append(",");

                    SerializeValue(item, sb, depth + 1);
                }
                sb.Append("]");
            }

            // 序列化字典
            private void SerializeDictionary(IDictionary dict, StringBuilder sb, int depth)
            {
                sb.Append("{");
                bool first = true;
                var enumerator = dict.GetEnumerator();

                while (enumerator.MoveNext())
                {
                    if (first) first = false;
                    else sb.Append(",");

                    var key = enumerator.Key;
                    var value = enumerator.Value;

                    sb.Append($"\"{key}\":");
                    SerializeValue(value, sb, depth + 1);
                }
                sb.Append("}");
            }
            #endregion

            #region 反序列化逻辑
            private object ParseValue(string json, ref int index, Type targetType)
            {
                SkipWhitespace(json, ref index);

                if (index >= json.Length) return null;

                char current = json[index];

                // 处理对象
                if (current == '{')
                {
                    return ParseObject(json, ref index, targetType);
                }
                // 处理数组
                else if (current == '[')
                {
                    return ParseArray(json, ref index, targetType);
                }
                // 处理字符串
                else if (current == '"')
                {
                    return ParseString(json, ref index);
                }
                // 处理null
                else if (json.Substring(index, 4) == "null")
                {
                    index += 4;
                    return null;
                }
                // 处理数值/布尔值
                else
                {
                    return ParsePrimitive(json, ref index, targetType);
                }
            }

            #region 修改后的对象解析方法（支持JsonProperty）
            private object ParseObject(string json, ref int index, Type targetType)
            {
                index++; // 跳过 '{'
                var obj = Activator.CreateInstance(targetType);
                var propMap = CreatePropertyMap(targetType);

                while (index < json.Length)
                {
                    SkipWhitespace(json, ref index);
                    if (json[index] == '}') break;

                    // 解析JSON键名
                    string jsonKey = ParseString(json, ref index);
                    SkipWhitespace(json, ref index);
                    if (json[index] != ':') throw new FormatException("Expected colon");
                    index++; // 跳过 ':'

                    // 查找匹配的属性
                    if (propMap.TryGetValue(jsonKey, out PropertyInfo prop) && prop.CanWrite)
                    {
                        object value = ParseValue(json, ref index, prop.PropertyType);
                        prop.SetValue(obj, value);
                    }

                    SkipWhitespace(json, ref index);
                    if (json[index] == ',') index++;
                }

                index++; // 跳过 '}'
                return obj;
            }
            // 创建属性名称映射表
            private Dictionary<string, PropertyInfo> CreatePropertyMap(Type type)
            {
                var map = new Dictionary<string, PropertyInfo>(StringComparer.OrdinalIgnoreCase);
                foreach (var prop in type.GetProperties())
                {
                    var jsonProp = prop.GetCustomAttribute<JsonPropertyAttribute>();
                    var key = jsonProp?.Name ?? prop.Name;
                    map[key] = prop;
                }
                return map;
            }
            #endregion

            // 解析数组
            private object ParseArray(string json, ref int index, Type targetType)
            {
                index++; // 跳过 '['
                var list = new List<object>();
                Type elementType = GetCollectionElementType(targetType);

                while (index < json.Length)
                {
                    SkipWhitespace(json, ref index);
                    if (json[index] == ']') break;

                    // 解析元素
                    object element = ParseValue(json, ref index, elementType);
                    list.Add(element);

                    // 处理逗号
                    SkipWhitespace(json, ref index);
                    if (json[index] == ',') index++;
                }

                index++; // 跳过 ']'
                return ConvertListToTargetType(list, targetType, elementType);
            }
            #endregion

            #region 辅助方法
            // 类型判断
            private bool IsEnumerable(Type type)
            {
                return typeof(IEnumerable).IsAssignableFrom(type) &&
                       type != typeof(string);
            }

            private bool IsDictionary(Type type)
            {
                return typeof(IDictionary).IsAssignableFrom(type);
            }

            // 获取集合元素类型
            private Type GetCollectionElementType(Type collectionType)
            {
                if (collectionType.IsArray)
                    return collectionType.GetElementType();

                if (collectionType.IsGenericType)
                    return collectionType.GetGenericArguments()[0];

                return typeof(object);
            }

            // 类型转换逻辑
            private object ConvertListToTargetType(List<object> list, Type targetType, Type elementType)
            {
                if (targetType.IsArray)
                {
                    Array array = Array.CreateInstance(elementType, list.Count);
                    for (int i = 0; i < list.Count; i++)
                        array.SetValue(list[i], i);
                    return array;
                }

                if (targetType.IsGenericType)
                {
                    Type listType = typeof(List<>).MakeGenericType(elementType);
                    var typedList = Activator.CreateInstance(listType);
                    MethodInfo addMethod = listType.GetMethod("Add");

                    foreach (var item in list)
                        addMethod.Invoke(typedList, new[] { item });

                    return typedList;
                }

                return list;
            }

            // 字符串处理
            #region 字符串处理
            private string EscapeString(string s)
            {
                if (string.IsNullOrEmpty(s)) return "";

                var sb = new StringBuilder(s.Length);
                foreach (char c in s)
                {
                    switch (c)
                    {
                        case '\\': sb.Append("\\\\"); break;
                        case '"': sb.Append("\\\""); break;
                        case '\n': sb.Append("\\n"); break;
                        case '\r': sb.Append("\\r"); break;
                        case '\t': sb.Append("\\t"); break;
                        default: sb.Append(c); break;
                    }
                }
                return sb.ToString();
            }

            private string UnescapeString(string s)
            {
                if (string.IsNullOrEmpty(s)) return "";

                var sb = new StringBuilder(s.Length);
                for (int i = 0; i < s.Length; i++)
                {
                    if (s[i] == '\\' && i + 1 < s.Length)
                    {
                        switch (s[++i])
                        {
                            case '\\': sb.Append('\\'); break;
                            case '"': sb.Append('"'); break;
                            case 'n': sb.Append('\n'); break;
                            case 'r': sb.Append('\r'); break;
                            case 't': sb.Append('\t'); break;
                            default: sb.Append('\\').Append(s[i]); break;
                        }
                    }
                    else
                    {
                        sb.Append(s[i]);
                    }
                }
                return sb.ToString();
            }
            #endregion

            #region 解析辅助方法
            // 跳过空白字符
            private void SkipWhitespace(string json, ref int index)
            {
                while (index < json.Length && char.IsWhiteSpace(json[index]))
                {
                    index++;
                }
            }

            // 解析字符串
            private string ParseString(string json, ref int index)
            {
                index++; // 跳过起始引号
                var sb = new StringBuilder();

                while (index < json.Length)
                {
                    char c = json[index++];
                    if (c == '"') break;

                    if (c == '\\' && index < json.Length)
                    {
                        c = json[index++];
                        switch (c)
                        {
                            case '\\': sb.Append('\\'); break;
                            case '"': sb.Append('"'); break;
                            case 'n': sb.Append('\n'); break;
                            case 'r': sb.Append('\r'); break;
                            case 't': sb.Append('\t'); break;
                            default: sb.Append('\\').Append(c); break;
                        }
                    }
                    else
                    {
                        sb.Append(c);
                    }
                }

                return sb.ToString();
            }

            // 解析基础类型
            private object ParsePrimitive(string json, ref int index, Type targetType)
            {
                int start = index;

                while (index < json.Length && !IsEndOfPrimitive(json[index]))
                {
                    index++;
                }

                string value = json.Substring(start, index - start);

                if (targetType == typeof(bool))
                {
                    return bool.Parse(value.ToLower());
                }

                if (targetType.IsEnum)
                {
                    return Enum.Parse(targetType, value, true);
                }

                // 处理数值类型
                try
                {
                    return Convert.ChangeType(value, targetType);
                }
                catch
                {
                    throw new FormatException($"Cannot convert '{value}' to {targetType.Name}");
                }
            }

            // 判断基础类型结束位置
            private bool IsEndOfPrimitive(char c)
            {
                return c == ',' || c == '}' || c == ']' || char.IsWhiteSpace(c);
            }
            #endregion
            #endregion


        }
        #endregion
    }
}
