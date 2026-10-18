using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text;
using BepInEx;

using BepInEx.Configuration;
using System.Net.Http;
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
    [BepInDependency("com.bepis.bepinex.configurationmanager", BepInDependency.DependencyFlags.SoftDependency)]
    [BepInProcess(GAME_PROCESS)]
    public partial class DSPRemoteSaves : BaseUnityPlugin
    {
        public const string GUID = "com.aims.remotesaves";
        public const string NAME = "RemoteSaves";
        public const string VERSION = "2.2.0";
        public const string GAME_PROCESS = "DSPGAME.exe";

        #region Config Settings
        private ConfigEntry<string> configServerURL;
        private ConfigEntry<string> configUsername;
        private ConfigEntry<string> configPassword;
        private ConfigEntry<string> configFileExtensions;
        private ConfigEntry<int> configChunkSize;
        private ConfigEntry<int> configChunkThresholdMB;
        private ConfigEntry<bool> configEnabled;
        private ConfigEntry<int> configMaxParallelDownloadCount;
        private ConfigEntry<int> configMaxParallelUploadCount;
        private ConfigEntry<int> configMaxChunkParallelUploadCount;
        private ConfigEntry<KeyCode> configPanelHotkey;
        private ConfigEntry<int> configAutoRefreshSeconds;
        #endregion Config Settings

        /// <summary>云端与本地时间戳比较时允许的误差（毫秒）</summary>
        private const long ClockToleranceMs = 2000;
        /// <summary>服务端允许的最小分片（对应 acc-server 的 chunk_size 下限 64KB）</summary>
        private const int MinChunkBytes = 64 * 1024;
        /// <summary>服务端允许的单个分片上限（对应 storage.max_chunk_size 默认值 64MB），超过只能分片</summary>
        private const int MaxSingleChunkBytes = 64 * 1024 * 1024;

        private HttpClient httpClient;
        /// <summary>服务端 SM2 公钥；首次请求时拉取，收到 2005 时强制刷新</summary>
        private PublicKeyInfo _publicKey;
        private DateTime _publicKeyFetchedUtc = DateTime.MinValue;
        /// <summary>配置不完整/地址非法时的提示，供面板展示</summary>
        private string _configError;
        /// <summary>游戏内云存档面板（手动上传/下载）</summary>
        private SavePanel _panel;
        /// <summary>Configuration Manager 自定义绘制器对应的入口项</summary>
        private ConfigEntry<bool> configPanelEntry;
        /// <summary>最近一次服务端返回的错误信息（用于面板错误行）</summary>
        private string _lastServerError;
        internal string LastServerError => _lastServerError;

        public void Awake()
        {
            try
            {
                // 只做配置与网络客户端初始化，不再在启动/退出时自动传输：
                // 上传与下载全部由游戏内云存档面板交给玩家逐项确认。
                InitConfig();
                RefreshHttpClient();

                _panel = new SavePanel(this);
            }
            catch (Exception ex)
            {
                Logger.LogError($"插件加载失败: {FormatException(ex)}");
                // 确保异常能被Unity捕获
                throw new InvalidOperationException("Critical plugin failure", ex);
            }
        }

        private void InitConfig()
        {
            configEnabled = Config.Bind("General", "Enabled", true, new ConfigDescription("启用远程存档同步功能"));

            configPanelHotkey = Config.Bind("General", "PanelHotkey", KeyCode.F8,
                new ConfigDescription("打开/关闭云存档同步面板的快捷键（默认 F8）"));

            configServerURL = Config.Bind("Server", "Remote ServerURL", "",
                new ConfigDescription("远程服务器地址（示例：http://example.com:9999）"));

            configUsername = Config.Bind("User", "Username", "", new ConfigDescription("远程服务器用户名"));
            configPassword = Config.Bind("User", "Password", "", new ConfigDescription("远程服务器密码"));

            configFileExtensions = Config.Bind("Performance", "SyncFileExtensions", ".dsv,.moddsv,.server",
                new ConfigDescription("面板中显示的文件扩展名列表（使用英文逗号分隔）"));
            configMaxParallelUploadCount = Config.Bind("Performance", "MaxParallelUploadCount", 3,
                new ConfigDescription("同时上传的存档数量上限，超出的存档在面板中排队", new AcceptableValueRange<int>(1, 3)));
            configChunkSize = Config.Bind("Performance", "ChunkSize(MB)", 5,
                new ConfigDescription("大文件分块上传时每个分块的大小（单位：MB）", new AcceptableValueRange<int>(5, 50)));
            configChunkThresholdMB = Config.Bind("Performance", "ChunkThreshold(MB)", 5,
                new ConfigDescription("分片阈值（单位：MB）：不超过该大小的文件整块上传（只有 1 个分片），超过该大小才按 ChunkSize(MB) 切分。设为 0 表示始终分片；服务端单分片上限 64MB，超过 64MB 的文件始终分片",
                    new AcceptableValueRange<int>(0, 64)));
            configMaxChunkParallelUploadCount = Config.Bind("Performance", "MaxChunkParallelUploadCount", 5,
                new ConfigDescription("单个文件分块上传时的最大并行分块数", new AcceptableValueRange<int>(1, 10)));
            configMaxParallelDownloadCount = Config.Bind("Performance", "MaxParallelDownloadCount", 5,
                new ConfigDescription("同时下载的存档数量上限", new AcceptableValueRange<int>(1, 10)));
            configAutoRefreshSeconds = Config.Bind("Performance", "AutoRefresh(s)", 30,
                new ConfigDescription("面板打开时的自动刷新间隔（秒），0 表示不自动刷新", new AcceptableValueRange<int>(0, 300)));

            Config.Save();
        }

        /// <summary>
        /// F1 里的入口在 Start 阶段绑定：此时 BepInEx 已加载完所有插件（包括
        /// Configuration Manager），反射一定能找到它的标签类型。
        /// </summary>
        public void Start()
        {
            try
            {
                var cmAttr = SavePanel.CreateConfigurationManagerAttributes(
                    entry => _panel?.DrawConfigEntry(entry));
                var panelDescription = cmAttr != null
                    ? new ConfigDescription("打开游戏内云存档同步面板，自行选择要上传或下载的存档", null, new object[] { cmAttr })
                    : new ConfigDescription("打开游戏内云存档同步面板（未检测到 Configuration Manager，可改用快捷键 " + configPanelHotkey.Value + "）");
                configPanelEntry = Config.Bind("General", "OpenSyncPanel", false, panelDescription);
                Config.Save();

                Logger.LogInfo($"{NAME} v{VERSION} 已加载，按 {configPanelHotkey.Value} 打开云存档面板");
                if (!SavePanel.ConfigurationManagerAvailable)
                {
                    Logger.LogWarning("未检测到 BepInEx Configuration Manager：F1 中的面板入口不可用，可直接用快捷键打开面板");
                }
            }
            catch (Exception ex)
            {
                Logger.LogError($"面板入口初始化失败（快捷键仍可用）: {FormatException(ex)}");
            }
        }

        private void Update()
        {
            _panel?.Tick();
        }

        private void OnGUI()
        {
            _panel?.Draw();
        }

        /// <summary>
        /// 校验基础配置并（重新）建立 HttpClient。切换服务器或账号后会在下一次
        /// 打开/刷新面板时生效；有传输正在进行时不允许切换，避免中途换连接。
        /// </summary>
        private bool RefreshHttpClient()
        {
            _configError = null;

            if (!configEnabled.Value)
            {
                _configError = "插件已禁用（F1 → General → Enabled）";
                return false;
            }

            var url = (configServerURL.Value ?? string.Empty).Trim().TrimEnd('/');
            if (url.Length == 0)
            {
                _configError = "未配置服务器地址（F1 → Server）";
                return false;
            }

            if (!Uri.TryCreate(url + "/dsp.saves/api", UriKind.Absolute, out var baseUri) ||
                (baseUri.Scheme != Uri.UriSchemeHttp && baseUri.Scheme != Uri.UriSchemeHttps))
            {
                _configError = "服务器地址格式不正确";
                return false;
            }

            if (string.IsNullOrEmpty(configUsername.Value) || string.IsNullOrEmpty(configPassword.Value))
            {
                _configError = "未配置账号或密码（F1 → User）";
                return false;
            }

            if (httpClient != null && httpClient.BaseAddress != baseUri)
            {
                if (_panel != null && _panel.HasActiveTransfers)
                {
                    _configError = "有存档正在传输，请等待完成后再切换服务器";
                    return false;
                }
                httpClient.Dispose();
                httpClient = null;
                _publicKey = null;
            }

            if (httpClient == null)
            {
                // 超时按请求单独设置，避免大分片被全局 30 秒超时打断
                httpClient = new HttpClient
                {
                    Timeout = System.Threading.Timeout.InfiniteTimeSpan,
                    BaseAddress = baseUri
                };
                _publicKey = null;
            }

            return true;
        }

        /// <summary>面板在发起列表/传输请求前调用，返回可展示的失败原因。</summary>
        private bool PrepareConnection(out string error)
        {
            error = null;
            if (!RefreshHttpClient())
            {
                error = _configError;
                return false;
            }
            return true;
        }

        public void OnDestroy()
        {
            // 关闭面板并取消未完成的传输；HttpClient 交给进程回收，避免退出时卡住
            _panel?.Shutdown();
            Logger.LogInfo("RemoteSaves unloaded");
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
                .Select(e => e.Trim().ToLowerInvariant())
                .DefaultIfEmpty(".dsv")
                .ToArray();
        }
        #region 数据模型
        public class AuthRequest
        {
            [JsonProperty("userName")]
            public string UserName { get; set; }

            [JsonProperty("password")]
            public string Password { get; set; }

            /// <summary>请求毫秒时间戳（新后端防重放校验）</summary>
            [JsonProperty("timestamp")]
            public long? Timestamp { get; set; }

            /// <summary>一次性随机数（新后端防重放校验）</summary>
            [JsonProperty("nonce")]
            public string Nonce { get; set; }
        }
        public class FileListRequest : AuthRequest
        {
            [JsonProperty("syncFileExtensions")]
            public List<string> SyncFileExtensions { get; set; } = new List<string>();

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

        public class CompleteUploadRequest : AuthRequest
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

            /// <summary>用户配额（字节）；旧服务端不返回，值为 0</summary>
            [JsonProperty("quotaBytes")]
            public long QuotaBytes { get; set; }

            [JsonProperty("usedBytes")]
            public long UsedBytes { get; set; }

            /// <summary>已预留但尚未完成上传的字节数</summary>
            [JsonProperty("reservedBytes")]
            public long ReservedBytes { get; set; }
        }

        /// <summary>云端列表 + 配额快照</summary>
        public class RemoteListing
        {
            public List<RemoteFile> Files = new List<RemoteFile>();
            public long QuotaBytes;
            public long UsedBytes;
            public long ReservedBytes;
            public bool HasQuota => QuotaBytes > 0;
        }

        public class RemoteFile
        {
            [JsonProperty("fileName")]
            public string FileName { get; set; }

            [JsonProperty("fileSize")]
            public long FileSize { get; set; }

            /// <summary>云端文件最后修改时间（Unix 毫秒），用于冲突判定</summary>
            [JsonProperty("lastModified")]
            public long LastModified { get; set; }
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

            /// <summary>分片上传凭据，只在 init 返回一次</summary>
            [JsonProperty("uploadToken")]
            public string UploadToken { get; set; }
        }

        public class PublicKeyInfo
        {
            public string KeyId { get; set; }
            public string PublicKey { get; set; }
        }

        public class PublicKeyResponse : BaseResponse
        {
            [JsonProperty("payload")]
            public PublicKeyPayload Payload { get; set; }
        }

        public class PublicKeyPayload
        {
            [JsonProperty("keyId")]
            public string KeyId { get; set; }

            [JsonProperty("publicKey")]
            public string PublicKey { get; set; }

            [JsonProperty("sm2Mode")]
            public string Sm2Mode { get; set; }

            [JsonProperty("sm4Mode")]
            public string Sm4Mode { get; set; }

            [JsonProperty("serverTime")]
            public long ServerTime { get; set; }
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
                        fileInfo.LastWriteTimeUtc > index[lowerKey].LastWriteTimeUtc)
                    {
                        index[lowerKey] = fileInfo;
                    }
                }
                catch (Exception e)
                {
                    Logger.LogWarning($"无法读取本地文件 {filePath}: {e.Message}");
                }
            }
            return index;
        }

        /// <summary>
        /// 下载单个存档到本地。onProgress 报告本次读取的字节数（增量），
        /// 取消由面板行的 CancellationToken 控制；覆盖前自动备份。
        /// </summary>
        private async Task<bool> DownloadSingleFile(RemoteFile remoteFile, Action<long> onProgress, CancellationToken token)
        {
            var fileName = remoteFile.FileName;
            var savePath = GetSavePath();
            var destPath = Path.Combine(savePath, Path.GetFileName(fileName));
            var tempPath = destPath + ".remote.tmp";

            try
            {
                var payload = new FileDownloadRequest
                {
                    UserName = configUsername.Value,
                    Password = configPassword.Value,
                    FileName = fileName,
                    IsChunk = false
                };
                Logger.LogInfo($"开始下载存档 {payload.UserName}/{payload.FileName}");

                // 用 ResponseHeadersRead 流式落盘，避免把整个存档先读进内存
                using var cts = CancellationTokenSource.CreateLinkedTokenSource(token);
                cts.CancelAfter(TimeSpan.FromMinutes(30));
                using var response = await PostSealedStreamAsync(payload, "/download/download", cts.Token)
                    .ConfigureAwait(false);

                if (!response.IsSuccessStatusCode)
                {
                    var body = await response.Content.ReadAsStringAsync().ConfigureAwait(false);
                    Logger.LogError($"下载失败 HTTP {(int)response.StatusCode}: {body}");
                    return false;
                }

                Directory.CreateDirectory(savePath);
                if (File.Exists(tempPath)) File.Delete(tempPath);

                using (var fileStream = new FileStream(tempPath, FileMode.CreateNew, FileAccess.Write, FileShare.None, 81920, true))
                using (var source = await response.Content.ReadAsStreamAsync().ConfigureAwait(false))
                {
                    var buffer = new byte[81920];
                    while (true)
                    {
                        int read = await source.ReadAsync(buffer, 0, buffer.Length, cts.Token).ConfigureAwait(false);
                        if (read <= 0) break;
                        await fileStream.WriteAsync(buffer, 0, read, cts.Token).ConfigureAwait(false);
                        onProgress?.Invoke(read);
                    }
                }

                var downloadedLength = new FileInfo(tempPath).Length;
                if (remoteFile.FileSize > 0 && downloadedLength != remoteFile.FileSize)
                {
                    File.Delete(tempPath);
                    Logger.LogError($"下载大小不一致 ({downloadedLength}/{remoteFile.FileSize})，已丢弃: {fileName}");
                    return false;
                }

                // 覆盖前先备份，再用替换保证原子性，避免写坏现有存档
                if (File.Exists(destPath))
                {
                    BackupLocalFile(destPath);
                    try
                    {
                        File.Replace(tempPath, destPath, null);
                    }
                    catch (Exception replaceEx)
                    {
                        // 某些文件系统不支持 Replace，退化为删除后移动
                        Logger.LogWarning($"File.Replace 失败，改用移动覆盖: {replaceEx.Message}");
                        File.Delete(destPath);
                        File.Move(tempPath, destPath);
                    }
                }
                else
                {
                    File.Move(tempPath, destPath);
                }

                // 把本地时间对齐到云端时间，避免两台机器时钟不一致导致反复下载
                if (remoteFile.LastModified > 0)
                {
                    File.SetLastWriteTimeUtc(destPath, DateTimeOffset.FromUnixTimeMilliseconds(remoteFile.LastModified).UtcDateTime);
                }

                Logger.LogInfo($"下载完成 {payload.UserName}/{payload.FileName}");
                return true;
            }
            catch (OperationCanceledException) when (token.IsCancellationRequested)
            {
                Logger.LogInfo($"已取消下载: {fileName}");
                try { if (File.Exists(tempPath)) File.Delete(tempPath); } catch { }
                throw;
            }
            catch (OperationCanceledException)
            {
                Logger.LogError($"下载超时: {fileName}");
                try { if (File.Exists(tempPath)) File.Delete(tempPath); } catch { }
                return false;
            }
            catch (Exception e)
            {
                Logger.LogError($"文件下载失败 ({fileName}): {FormatException(e)}");
                try { if (File.Exists(tempPath)) File.Delete(tempPath); } catch { }
                return false;
            }
        }

        private void BackupLocalFile(string filePath)
        {
            try
            {
                var backupRoot = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                    "DSPRemoteSaves",
                    "backup",
                    DateTime.Now.ToString("yyyyMMdd-HHmmss"));
                Directory.CreateDirectory(backupRoot);
                var target = Path.Combine(backupRoot, Path.GetFileName(filePath));
                File.Copy(filePath, target, true);
                Logger.LogInfo($"已备份本地存档: {target}");
            }
            catch (Exception ex)
            {
                Logger.LogWarning($"备份本地存档失败: {ex.Message}");
            }
        }


        /// <summary>拉取云端存档列表与配额；失败返回 null（日志中保留原因）。</summary>
        private async Task<RemoteListing> GetRemoteFileList()
        {
            try
            {
                var payload = new FileListRequest
                {
                    UserName = configUsername.Value,
                    Password = configPassword.Value,
                    SyncFileExtensions = GetFileExtensions().ToList()
                };

                using var response = await PostSealed("/download/file-list", payload).ConfigureAwait(false);
                if (!response.IsSuccessStatusCode)
                {
                    await HandleErrorResponse(response).ConfigureAwait(false);
                    return null;
                }

                var responseString = await response.Content.ReadAsStringAsync().ConfigureAwait(false);
                var result = new CustomJsonSerializer().Deserialize<FileListResponse>(responseString);

                switch (result?.Code)
                {
                    case 0:
                        var payloadData = result.Payload ?? new FileListPayload();
                        return new RemoteListing
                        {
                            Files = payloadData.Files ?? new List<RemoteFile>(),
                            QuotaBytes = payloadData.QuotaBytes,
                            UsedBytes = payloadData.UsedBytes,
                            ReservedBytes = payloadData.ReservedBytes
                        };

                    default:
                        await HandleErrorResponse(response).ConfigureAwait(false);
                        return null;
                }
            }
            catch (Exception e)
            {
                Logger.LogError($"获取文件列表失败: {FormatException(e)}");
                return null;
            }
        }

        /// <summary>
        /// 上传单个存档。onProgress 报告每个分片成功后的字节数（增量），
        /// 取消会中断后续分片并丢弃本次上传会话（服务端按 TTL 清理）。
        /// </summary>
        private async Task<bool> UploadSingleFile(string filePath, Action<long> onProgress, CancellationToken token)
        {
            try
            {
                token.ThrowIfCancellationRequested();
                var fileName = Path.GetFileName(filePath);
                var fileInfo = new FileInfo(filePath);

                // 初始化上传
                var (fileId, chunkSize, uploadToken) = await InitUpload(fileName, fileInfo.Length).ConfigureAwait(false);
                if (string.IsNullOrEmpty(fileId)) return false;
                token.ThrowIfCancellationRequested();

                // 上传分片
                if (!await UploadChunks(filePath, fileId, chunkSize, uploadToken, onProgress, token).ConfigureAwait(false))
                {
                    return false;
                }

                token.ThrowIfCancellationRequested();

                // 完成上传
                return await CompleteUpload(fileId).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                Logger.LogInfo($"已取消上传: {Path.GetFileName(filePath)}");
                throw;
            }
            catch (Exception e)
            {
                Logger.LogError($"文件上传失败 ({Path.GetFileName(filePath)}): {FormatException(e)}");
                return false;
            }
        }
        private async Task<(string FileId, int ChunkSize, string UploadToken)> InitUpload(string fileName, long fileSize)
        {
            try
            {
                var chunkSize = ResolveChunkSize(fileSize);
                var initPayload = new UploadInitRequest
                {
                    UserName = configUsername.Value,
                    Password = configPassword.Value,
                    FileName = fileName,
                    FileSize = fileSize,
                    ChunkSize = chunkSize
                };

                using var response = await PostSealed("/upload/init", initPayload).ConfigureAwait(false);
                var responseString = await response.Content.ReadAsStringAsync().ConfigureAwait(false);
                var result = new CustomJsonSerializer().Deserialize<UploadInitResponse>(responseString);

                if (result?.Code == 0 && result.Payload?.Data != null)
                {
                    return (result.Payload.Data.FileId, result.Payload.Data.ChunkSize, result.Payload.Data.UploadToken);
                }

                await HandleErrorResponse(response).ConfigureAwait(false);
                return (null, 0, null);
            }
            catch (Exception e)
            {
                Logger.LogError($"上传初始化失败: {FormatException(e)}");
                return (null, 0, null);
            }
        }

        /// <summary>
        /// 按文件大小决定 init 时上报的分片大小：
        /// 文件不超过 ChunkThreshold(MB) 时整块上传（服务端算出 totalChunks=1，不切分）；
        /// 超过阈值则按 ChunkSize(MB) 切分。
        /// 服务端要求 64KB ≤ chunkSize ≤ max_chunk_size（默认 64MB），单块大小会被夹到该区间。
        /// </summary>
        private int ResolveChunkSize(long fileSize)
        {
            var thresholdBytes = (long)configChunkThresholdMB.Value * 1024 * 1024;
            if (thresholdBytes > 0 && fileSize > 0 &&
                fileSize <= thresholdBytes && fileSize <= MaxSingleChunkBytes)
            {
                // 整块上传：chunkSize 取文件自身大小，服务端即得 1 个分片
                Logger.LogDebug($"文件 {fileSize / 1024.0 / 1024.0:0.00}MB 未超过分片阈值 {configChunkThresholdMB.Value}MB，整块上传");
                return (int)Math.Max(MinChunkBytes, fileSize);
            }
            return configChunkSize.Value * 1024 * 1024;
        }

        private async Task<bool> UploadChunks(
            string filePath,
            string fileId,
            int chunkSize,
            string uploadToken,
            Action<long> onProgress,
            CancellationToken token)
        {
            try
            {
                var fileInfo = new FileInfo(filePath);
                long fileLength = fileInfo.Length;
                if (chunkSize <= 0)
                {
                    Logger.LogError("服务端返回的分片大小无效");
                    return false;
                }

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
                var semaphore = new SemaphoreSlim(Math.Max(1, configMaxChunkParallelUploadCount.Value));
                var failedChunks = new ConcurrentBag<int>();
                using var cts = CancellationTokenSource.CreateLinkedTokenSource(token);

                // 并行上传任务
                var uploadTasks = chunkMetadata.Select(async meta =>
                {
                    try
                    {
                        await semaphore.WaitAsync(cts.Token).ConfigureAwait(false);
                    }
                    catch (OperationCanceledException)
                    {
                        return;
                    }

                    try
                    {
                        if (cts.IsCancellationRequested)
                            return;

                        // 独立文件流读取避免并发问题
                        var buffer = new byte[meta.ChunkLength];
                        using (var fs = new FileStream(filePath, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, FileOptions.Asynchronous))
                        {
                            fs.Seek(meta.StartPos, SeekOrigin.Begin);
                            await ReadExactlyAsync(fs, buffer, meta.ChunkLength, cts.Token).ConfigureAwait(false);
                        }

                        using var content = new ByteArrayContent(buffer);
                        using var requestCts = CancellationTokenSource.CreateLinkedTokenSource(cts.Token);
                        // 按分片大小放宽超时：保底 30 秒，再按 32KB/s 估算传输时间
                        requestCts.CancelAfter(TimeSpan.FromSeconds(30 + meta.ChunkLength / 32768.0));

                        using var chunkRequest = new HttpRequestMessage(
                            HttpMethod.Post,
                            httpClient.BaseAddress + $"/upload/chunk?fileId={fileId}&chunkIndex={meta.Index}")
                        {
                            Content = content
                        };
                        // chunk 只认 init 签发的一次性 token，不再携带密码
                        chunkRequest.Headers.TryAddWithoutValidation("uploadToken", uploadToken);
                        using var response = await httpClient
                            .SendAsync(chunkRequest, requestCts.Token)
                            .ConfigureAwait(false);

                        if (!response.IsSuccessStatusCode)
                        {
                            await HandleErrorResponse(response).ConfigureAwait(false);
                            failedChunks.Add(meta.Index);
                            cts.Cancel();
                        }
                        else
                        {
                            // 分片是并行完成的，进度按累计字节数上报
                            onProgress?.Invoke(meta.ChunkLength);
                        }
                    }
                    catch (OperationCanceledException)
                    {
                        // 用户取消或同文件其他分片失败：不再记录为超时
                        if (!cts.IsCancellationRequested)
                        {
                            Logger.LogError($"分片{meta.Index}上传超时");
                            failedChunks.Add(meta.Index);
                            cts.Cancel();
                        }
                    }
                    catch (Exception ex)
                    {
                        Logger.LogError($"分片{meta.Index}上传失败: {ex.Message}");
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
                    await Task.WhenAll(uploadTasks).ConfigureAwait(false);
                }
                catch
                {
                    // 具体错误已在各任务中处理
                }

                if (token.IsCancellationRequested)
                {
                    throw new OperationCanceledException(token);
                }

                if (!failedChunks.IsEmpty)
                {
                    Logger.LogError($"以下分片上传失败: {string.Join(", ", failedChunks)}");
                    return false;
                }

                Logger.LogInfo($"成功上传 {totalChunks} 个分片");
                return true;
            }
            catch (OperationCanceledException)
            {
                throw;
            }
            catch (Exception e)
            {
                Logger.LogError($"分片上传过程异常: {FormatException(e)}");
                return false;
            }
        }

        /// <summary>FileStream.ReadAsync 可能只返回部分数据，必须循环读满</summary>
        private static async Task ReadExactlyAsync(Stream stream, byte[] buffer, int count, CancellationToken token)
        {
            int offset = 0;
            while (offset < count)
            {
                int read = await stream.ReadAsync(buffer, offset, count - offset, token).ConfigureAwait(false);
                if (read <= 0)
                {
                    throw new EndOfStreamException($"文件读取提前结束 ({offset}/{count} bytes)");
                }
                offset += read;
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
                var payload = new CompleteUploadRequest
                {
                    FileId = fileId,
                    UserName = configUsername.Value,
                    Password = configPassword.Value
                };
                using var response = await PostSealed("/upload/complete", payload, TimeSpan.FromMinutes(5)).ConfigureAwait(false);
                var responseString = await response.Content.ReadAsStringAsync().ConfigureAwait(false);
                var result = new CustomJsonSerializer().Deserialize<UploadCompleteResponse>(responseString);
                switch (result?.Code)
                {
                    case 0 when response.IsSuccessStatusCode:
                        return true;
                    case 1 when result.Payload?.Nots != null:
                        Logger.LogError($"缺失分片: {string.Join(", ", result.Payload.Nots)}");
                        return false;
                    default:
                        await HandleErrorResponse(response).ConfigureAwait(false);
                        return false;
                }
            }
            catch (Exception e)
            {
                Logger.LogError($"上传完成失败: {FormatException(e)}");
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

                var responseString = await response.Content.ReadAsStringAsync().ConfigureAwait(false);
                try
                {
                    var errorResponse = new CustomJsonSerializer().Deserialize<UploadCompleteResponse>(responseString);
                    if (!string.IsNullOrEmpty(errorResponse?.Message))
                    {
                        _lastServerError = errorResponse.Message;
                    }
                    errorMessage.AppendLine($"错误码: {errorResponse?.Code}")
                               .AppendLine($"错误信息: {errorResponse?.Message}");

                    if (errorResponse?.Payload?.Nots?.Count > 0)
                        errorMessage.AppendLine($"缺失分片索引: {string.Join(", ", errorResponse.Payload.Nots)}");

                    if (errorResponse?.Payload?.Error != null)
                        errorMessage.AppendLine($"服务端异常: {errorResponse.Payload.Error.Message}");
                }
                catch (Exception jsonEx)
                {
                    errorMessage.AppendLine($"原始响应内容: {responseString}")
                               .AppendLine($"JSON解析失败: {jsonEx.Message}");
                }

                Logger.LogError(errorMessage.ToString());
            }
            catch (Exception ex)
            {
                Logger.LogError($"错误处理过程中发生异常: {FormatException(ex)}");
            }
        }

        private async Task<HttpResponseMessage> PostJson<T>(
            string endpoint,
            T payload,
            TimeSpan? timeout = null,
            Dictionary<string, string> extraHeaders = null)
        {
            var effectiveTimeout = timeout ?? TimeSpan.FromSeconds(30);
            try
            {
                var json = new CustomJsonSerializer().Serialize(payload);
                using var content = new StringContent(json, Encoding.UTF8, "application/json");
                using var cts = new CancellationTokenSource(effectiveTimeout);
                using var request = new HttpRequestMessage(
                    HttpMethod.Post,
                    httpClient.BaseAddress + endpoint)
                {
                    Content = content
                };
                if (extraHeaders != null)
                {
                    foreach (var header in extraHeaders)
                    {
                        request.Headers.TryAddWithoutValidation(header.Key, header.Value);
                    }
                }

                // 不打印请求体，避免把用户名密码写进日志
                Logger.LogDebug($"POST {endpoint} ({json.Length} bytes)");
                return await httpClient.SendAsync(request, cts.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                Logger.LogError($"请求超时: {endpoint}");
                throw new TimeoutException($"请求超时: {endpoint}");
            }
            catch (HttpRequestException e)
            {
                Logger.LogError($"网络请求失败: {e.Message}");
                throw new Exception($"网络请求失败: {e.Message}", e);
            }
        }
        /// <summary>拉取（或刷新）服务端 SM2 公钥。</summary>
        private async Task<bool> EnsurePublicKeyAsync(bool force = false)
        {
            if (!force && _publicKey != null &&
                (DateTime.UtcNow - _publicKeyFetchedUtc).TotalHours < 12)
            {
                return true;
            }

            try
            {
                using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(15));
                using var response = await httpClient
                    .GetAsync(httpClient.BaseAddress + "/crypto/public-key", cts.Token)
                    .ConfigureAwait(false);
                if (!response.IsSuccessStatusCode)
                {
                    Logger.LogError($"获取公钥失败: HTTP {(int)response.StatusCode}");
                    return false;
                }

                var text = await response.Content.ReadAsStringAsync().ConfigureAwait(false);
                var parsed = new CustomJsonSerializer().Deserialize<PublicKeyResponse>(text);
                if (parsed?.Code != 0 || parsed.Payload == null ||
                    string.IsNullOrEmpty(parsed.Payload.PublicKey) ||
                    string.IsNullOrEmpty(parsed.Payload.KeyId))
                {
                    Logger.LogError("公钥响应格式非法");
                    return false;
                }

                _publicKey = new PublicKeyInfo
                {
                    KeyId = parsed.Payload.KeyId,
                    PublicKey = parsed.Payload.PublicKey
                };
                _publicKeyFetchedUtc = DateTime.UtcNow;
                Logger.LogInfo($"已获取国密公钥 keyId={_publicKey.KeyId}");
                return true;
            }
            catch (Exception e)
            {
                Logger.LogError($"获取公钥异常: {FormatException(e)}");
                return false;
            }
        }

        /// <summary>把密码加密后写入 payload，并返回需要附加的请求头。</summary>
        private async Task<Dictionary<string, string>> SealAsync(AuthRequest payload, bool forceRefresh = false)
        {
            if (!await EnsurePublicKeyAsync(forceRefresh).ConfigureAwait(false))
            {
                throw new InvalidOperationException("无法获取服务端国密公钥");
            }

            SmCrypto.SealPassword(
                _publicKey.PublicKey,
                configPassword.Value,
                out string smkey,
                out string cipher);

            payload.UserName = configUsername.Value;
            payload.Password = cipher;
            // 新后端的防重放字段：毫秒时间戳 + 一次性随机数
            payload.Timestamp = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
            payload.Nonce = Guid.NewGuid().ToString("N");
            return new Dictionary<string, string>
            {
                { "smkey", smkey },
                { "smkeyid", _publicKey.KeyId }
            };
        }

        /// <summary>发送带国密信封的 JSON；遇到 2005 自动刷新公钥并重试一次。</summary>
        private async Task<HttpResponseMessage> PostSealed<T>(
            string endpoint,
            T payload,
            TimeSpan? timeout = null) where T : AuthRequest
        {
            HttpResponseMessage last = null;
            for (int attempt = 0; attempt < 2; attempt++)
            {
                var headers = await SealAsync(payload, attempt > 0).ConfigureAwait(false);
                last = await PostJson(endpoint, payload, timeout, headers).ConfigureAwait(false);
                if (last.IsSuccessStatusCode)
                {
                    return last;
                }

                var body = await last.Content.ReadAsStringAsync().ConfigureAwait(false);
                if (body.Contains("\"code\":2005") || body.Contains("\"code\": 2005"))
                {
                    last.Dispose();
                    continue;
                }
                return last;
            }
            return last;
        }

        /// <summary>流式下载：同样走国密信封，并支持 2005 重试。</summary>
        private async Task<HttpResponseMessage> PostSealedStreamAsync(
            AuthRequest payload,
            string endpoint,
            CancellationToken token)
        {
            HttpResponseMessage last = null;
            for (int attempt = 0; attempt < 2; attempt++)
            {
                var headers = await SealAsync(payload, attempt > 0).ConfigureAwait(false);
                var json = new CustomJsonSerializer().Serialize(payload);
                var content = new StringContent(json, Encoding.UTF8, "application/json");
                var request = new HttpRequestMessage(
                    HttpMethod.Post,
                    httpClient.BaseAddress + endpoint)
                {
                    Content = content
                };
                foreach (var header in headers)
                {
                    request.Headers.TryAddWithoutValidation(header.Key, header.Value);
                }

                last = await httpClient
                    .SendAsync(request, HttpCompletionOption.ResponseHeadersRead, token)
                    .ConfigureAwait(false);
                if (last.IsSuccessStatusCode)
                {
                    return last;
                }

                var body = await last.Content.ReadAsStringAsync().ConfigureAwait(false);
                if (body.Contains("\"code\":2005") || body.Contains("\"code\": 2005"))
                {
                    last.Dispose();
                    continue;
                }
                return last;
            }
            return last;
        }

        private string FormatException(Exception e)
        {
            return $"{e.GetType().Name}: {e.Message}\nStackTrace: {e.StackTrace}";
        }

        private static long ToUnixMs(DateTime utcTime)
        {
            return new DateTimeOffset(DateTime.SpecifyKind(utcTime, DateTimeKind.Utc)).ToUnixTimeMilliseconds();
        }

        private bool ShouldProcessFile(string fileName, IEnumerable<string> extensions)
        {
            var ext = Path.GetExtension(fileName);
            return extensions.Any(e => string.Equals(e, ext, StringComparison.OrdinalIgnoreCase));
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
                // 布尔值必须小写，且不能用 ToString().ToLower() 统一处理
                else if (type == typeof(bool))
                {
                    sb.Append((bool)value ? "true" : "false");
                }
                // 枚举按字符串输出，避免产生非法 JSON
                else if (type.IsEnum)
                {
                    sb.Append($"\"{EscapeString(value.ToString())}\"");
                }
                // 数值类型使用不变文化，避免小数点被本地化成逗号
                else if (type.IsPrimitive || type == typeof(decimal))
                {
                    sb.Append(Convert.ToString(value, CultureInfo.InvariantCulture));
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
                else if (index + 4 < json.Length && json.Substring(index, 4) == "null")
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
                    if (index >= json.Length) break;
                    if (json[index] == '}') break;

                    // 解析JSON键名
                    string jsonKey = ParseString(json, ref index);
                    SkipWhitespace(json, ref index);
                    if (index >= json.Length) break;
                    if (json[index] != ':') throw new FormatException("Expected colon");
                    index++; // 跳过 ':'

                    // 查找匹配的属性；未知字段必须跳过，否则服务端新增字段会导致整个解析失败
                    if (propMap.TryGetValue(jsonKey, out PropertyInfo prop) && prop.CanWrite)
                    {
                        object value = ParseValue(json, ref index, prop.PropertyType);
                        prop.SetValue(obj, value);
                    }
                    else
                    {
                        SkipValue(json, ref index);
                    }

                    SkipWhitespace(json, ref index);
                    if (index < json.Length && json[index] == ',') index++;
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
                    if (index >= json.Length) break;
                    if (json[index] == ']') break;

                    // 解析元素
                    object element = ParseValue(json, ref index, elementType);
                    list.Add(element);

                    // 处理逗号
                    SkipWhitespace(json, ref index);
                    if (index < json.Length && json[index] == ',') index++;
                }

                index++; // 跳过 ']'
                return ConvertListToTargetType(list, targetType, elementType);
            }

            /// <summary>跳过一个未知字段的值（字符串/对象/数组/字面量）</summary>
            private void SkipValue(string json, ref int index)
            {
                SkipWhitespace(json, ref index);
                if (index >= json.Length) return;

                char current = json[index];
                if (current == '"')
                {
                    ParseString(json, ref index);
                    return;
                }

                if (current == '{' || current == '[')
                {
                    char close = current == '{' ? '}' : ']';
                    int depth = 0;
                    while (index < json.Length)
                    {
                        char ch = json[index];
                        if (ch == '"')
                        {
                            ParseString(json, ref index);
                            continue;
                        }
                        if (ch == current) depth++;
                        else if (ch == close)
                        {
                            depth--;
                            index++;
                            if (depth == 0) return;
                            continue;
                        }
                        index++;
                    }
                    return;
                }

                while (index < json.Length && !IsEndOfPrimitive(json[index])) index++;
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
                    return bool.Parse(value);
                }

                if (targetType.IsEnum)
                {
                    return Enum.Parse(targetType, value, true);
                }

                // 处理数值类型
                try
                {
                    return Convert.ChangeType(value, targetType, CultureInfo.InvariantCulture);
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
