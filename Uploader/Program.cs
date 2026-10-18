using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Net.Http;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;
using DSPRemoteSaves;

namespace DSPSavesUploader
{
    /// <summary>
    /// DSP 云存档后台上传器：由插件在「启动补传」和「退出续传」时派发的独立进程。
    /// 与游戏进程树完全独立（经 WMI 创建），游戏退出不影响上传。
    ///
    /// 任务发现（二选一）：
    ///   1. --task &lt;pending.json&gt;   指定单个清单（兼容旧调用）
    ///   2. 无参数                    扫描约定目录（exe 同目录 / DSPRemoteSaves 子目录）下全部
    ///                                pending-upload-*.json，处理完一轮后继续等待新清单，
    ///                                直到 20 秒内无新任务才退出。
    /// 清单格式：{"server":"http://host:9999","userName":"..","password":"..","files":["..",".."]}
    /// </summary>
    internal static class Program
    {
        private const int MaxAttemptsPerFile = 2;
        private static readonly TimeSpan NewTaskWait = TimeSpan.FromSeconds(20);
        private static Mutex _singleInstance;
        private static TextWriter _log = Console.Out;
        private static HttpClient _http;

        private static int Main(string[] args)
        {
            // 单实例：避免多个上传器并发上传同一文件；新实例会等待旧实例结束再接手剩余清单
            _singleInstance = new Mutex(true, @"Global\DSPSavesUploader", out var createdNew);
            if (!createdNew)
            {
                Log("已有上传器实例在运行，等待其结束...");
                try
                {
                    if (!_singleInstance.WaitOne(TimeSpan.FromMinutes(120)))
                    {
                        Log("等待超时，本次退出（清单保留，下次派发时继续）");
                        return 3;
                    }
                }
                catch (AbandonedMutexException) { /* 上个实例异常退出，接管 */ }
            }

            try { Console.OutputEncoding = Encoding.UTF8; } catch { }

            try
            {
                // 自检：SM3/SM4 标准向量（排查运行环境差异用）
                if (args.Any(a => a == "--selftest"))
                {
                    var sm3 = SmCrypto.BytesToHex(SmCrypto.Sm3(Encoding.UTF8.GetBytes("abc")));
                    var sm4 = SmCrypto.Sm4EcbEncryptHex(
                        "0123456789abcdeffedcba9876543210",
                        SmCrypto.HexToBytes("0123456789abcdeffedcba9876543210"));
                    Log("SM3(abc)=" + sm3 + (sm3 == "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0" ? " OK" : " MISMATCH"));
                    Log("SM4[0..16]=" + sm4.Substring(0, 32) + (sm4.StartsWith("681edf34d206965e86b3e94f536e4246") ? " OK" : " MISMATCH"));
                    return 0;
                }

                var baseDir = AppContext.BaseDirectory;
                var pendingDir = Path.Combine(baseDir, "DSPRemoteSaves");
                var searchDirs = new[] { pendingDir, baseDir };

                // 日志固定写在上传器旁的 DSPRemoteSaves/uploader.log
                try
                {
                    Directory.CreateDirectory(pendingDir);
                    _log = new StreamWriter(Path.Combine(pendingDir, "uploader.log"), true) { AutoFlush = true };
                }
                catch { /* 日志不可用时继续用控制台 */ }

                Log($"===== 后台上传器启动 pid={Process.GetCurrentProcess().Id} =====");

                using (_http = new HttpClient { Timeout = TimeSpan.FromMinutes(15) })
                {
                    var processed = new HashSet<string>(StringComparer.OrdinalIgnoreCase);

                    while (true)
                    {
                        var batch = CollectPendingTasks(args, searchDirs, processed);
                        if (batch.Count == 0)
                        {
                            Log("队列已清空，上传器退出");
                            return 0;
                        }

                        RunBatch(batch, processed);

                        // 处理完一批后等待新清单（游戏正常退出时插件会再派发），20 秒无新任务才退出
                        var more = WaitForNewTasks(args, searchDirs, processed);
                        if (more.Count == 0)
                        {
                            Log("队列已清空，上传器退出");
                            return 0;
                        }
                    }
                }
            }
            catch (Exception ex)
            {
                Log($"后台上传器异常退出: {ex}");
                return 2;
            }
            finally
            {
                _singleInstance.ReleaseMutex();
                _singleInstance.Dispose();
                if (_log != Console.Out) _log.Dispose();
            }
        }

        // ------------------------------------------------------------------
        // 任务发现与调度
        // ------------------------------------------------------------------

        /// <summary>收集尚未处理的清单：--task 优先，否则扫描约定目录。</summary>
        private static List<string> CollectPendingTasks(string[] args, string[] searchDirs, HashSet<string> processed)
        {
            var result = new List<string>();
            var taskArg = GetArgValue(args, "--task");
            if (!string.IsNullOrEmpty(taskArg))
            {
                var full = Path.GetFullPath(taskArg);
                if (File.Exists(full) && processed.Add(full)) result.Add(full);
                return result;
            }

            foreach (var dir in searchDirs)
            {
                if (string.IsNullOrEmpty(dir) || !Directory.Exists(dir)) continue;
                foreach (var f in Directory.GetFiles(dir, "pending-upload-*.json"))
                {
                    var full = Path.GetFullPath(f);
                    if (processed.Add(full)) result.Add(full);
                }
            }
            return result;
        }

        /// <summary>最多等 NewTaskWait 时间轮询新清单，有则返回（并计入 processed）。</summary>
        private static List<string> WaitForNewTasks(string[] args, string[] searchDirs, HashSet<string> processed)
        {
            var deadline = DateTime.UtcNow + NewTaskWait;
            while (DateTime.UtcNow < deadline)
            {
                Thread.Sleep(2000);
                var more = CollectPendingTasks(args, searchDirs, processed);
                if (more.Count > 0) return more;
            }
            return new List<string>();
        }

        /// <summary>处理一批清单：逐清单读文件列表并上传，成功后删除清单文件。</summary>
        private static void RunBatch(List<string> taskFiles, HashSet<string> processed)
        {
            var batchStopwatch = Stopwatch.StartNew();
            foreach (var taskPath in taskFiles)
            {
                try
                {
                    var json = File.ReadAllText(taskPath);
                    var server = ExtractString(json, "server");
                    var user = ExtractString(json, "userName");
                    var password = ExtractString(json, "password");
                    var files = ExtractStringArray(json, "files");

                    if (string.IsNullOrEmpty(server) || string.IsNullOrEmpty(user) || files.Count == 0)
                    {
                        Log($"任务清单不完整，跳过: {Path.GetFileName(taskPath)}");
                        SafeDelete(taskPath);
                        continue;
                    }

                    Log($"处理清单 {Path.GetFileName(taskPath)}（{files.Count} 个文件）");

                    // server 与插件配置同格式（不带路径），与插件一样自动拼接 API 前缀
                    var serverUrl = server.TrimEnd('/') + "/dsp.saves/api/";
                    var pk = EnsurePublicKey(serverUrl).GetAwaiter().GetResult();

                    var ok = 0;
                    foreach (var file in files)
                    {
                        if (!File.Exists(file))
                        {
                            Log($"文件不存在，跳过: {file}");
                            continue;
                        }

                        var done = false;
                        for (var attempt = 1; attempt <= MaxAttemptsPerFile && !done; attempt++)
                        {
                            try
                            {
                                // 文件可能仍被游戏写入：确认稳定后再传
                                if (!FileReady(file))
                                {
                                    Log($"文件仍在写入，本文件跳过（下次游戏退出会重新派发）: {Path.GetFileName(file)}");
                                    break;
                                }

                                UploadOne(serverUrl, user, password, pk, file).GetAwaiter().GetResult();
                                done = true;
                                ok++;
                            }
                            catch (Exception ex)
                            {
                                Log($"上传失败（第 {attempt}/{MaxAttemptsPerFile} 次）[{Path.GetFileName(file)}]: {ex.Message}");
                                if (attempt < MaxAttemptsPerFile) Thread.Sleep(3000);
                            }
                        }
                    }

                    SafeDelete(taskPath);
                    Log($"清单处理完成：成功 {ok}/{files.Count}（本批累计 {batchStopwatch.Elapsed.TotalMinutes:0.0} 分钟）");
                }
                catch (Exception ex)
                {
                    // 清单文件保留，下次派发时重试
                    Log($"处理清单异常（文件保留待重试）: {Path.GetFileName(taskPath)} → {ex.Message}");
                }
            }
        }

        // ------------------------------------------------------------------
        // 协议实现（与插件一致的模组协议：公钥 → 国密封装 → init/chunk/complete）
        // ------------------------------------------------------------------

        private sealed class PublicKeyInfo
        {
            public string KeyId;
            public string PublicKey;
        }

        private static async Task<PublicKeyInfo> EnsurePublicKey(string baseUrl)
        {
            var resp = await _http.GetAsync(baseUrl + "crypto/public-key").ConfigureAwait(false);
            var body = await resp.Content.ReadAsStringAsync().ConfigureAwait(false);
            if (ExtractLong(body, "code") != 0) throw new Exception("获取公钥失败: " + Trim(body, 200));
            var info = new PublicKeyInfo
            {
                KeyId = ExtractString(body, "keyId"),
                PublicKey = ExtractString(body, "publicKey"),
            };
            if (string.IsNullOrEmpty(info.KeyId) || string.IsNullOrEmpty(info.PublicKey))
                throw new Exception("公钥响应格式异常: " + Trim(body, 200));
            Log($"已获取国密公钥 keyId={info.KeyId}");
            return info;
        }

        private static async Task UploadOne(string server, string user, string password, PublicKeyInfo pk, string filePath)
        {
            var fileName = Path.GetFileName(filePath);
            var size = new FileInfo(filePath).Length;

            // 1) init（每请求独立信封，与插件 SealAsync 一致）
            var seal = Seal(pk.PublicKey, password);
            var initBody = "{\"userName\":\"" + Esc(user) + "\",\"password\":\"" + Esc(seal.Cipher) +
                           "\",\"fileName\":\"" + Esc(fileName) + "\",\"fileSize\":" + size +
                           ",\"chunkSize\":" + (4 * 1024 * 1024) +
                           ",\"timestamp\":" + DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() +
                           ",\"nonce\":\"" + Guid.NewGuid().ToString("N") + "\"}";
            var initResp = await PostJson(server + "upload/init", initBody, seal.Smkey, pk.KeyId).ConfigureAwait(false);
            var fileId = ExtractString(initResp, "fileId");
            var chunkSize = ExtractLong(initResp, "chunkSize");
            var totalChunks = ExtractLong(initResp, "totalChunks");
            var uploadToken = ExtractString(initResp, "uploadToken");
            if (string.IsNullOrEmpty(fileId) || string.IsNullOrEmpty(uploadToken) || chunkSize <= 0)
                throw new Exception("init 响应缺少关键字段: " + Trim(initResp, 300));
            Log($"init ok file={fileName} size={size} chunks={totalChunks}");

            // 2) 分片（uploadToken 头，流式读取避免大文件占内存）
            using (var fs = new FileStream(filePath, FileMode.Open, FileAccess.Read, FileShare.Read))
            {
                var buffer = new byte[chunkSize];
                var progressEvery = Math.Max(1, (int)totalChunks / 8);
                for (long index = 0; index < totalChunks; index++)
                {
                    var read = fs.Read(buffer, 0, buffer.Length);
                    if (read <= 0) break;
                    using (var content = new ByteArrayContent(buffer, 0, read))
                    {
                        content.Headers.TryAddWithoutValidation("Content-Type", "application/octet-stream");
                        using (var req = new HttpRequestMessage(HttpMethod.Post,
                                   server + $"upload/chunk?fileId={Uri.EscapeDataString(fileId)}&chunkIndex={index}")
                               { Content = content })
                        {
                            req.Headers.TryAddWithoutValidation("uploadToken", uploadToken);
                            var resp = await _http.SendAsync(req).ConfigureAwait(false);
                            var text = await resp.Content.ReadAsStringAsync().ConfigureAwait(false);
                            if (!resp.IsSuccessStatusCode || ExtractLong(text, "code") != 0)
                                throw new Exception($"chunk {index} 失败: HTTP {(int)resp.StatusCode} {Trim(text, 200)}");
                        }
                    }
                    if ((index + 1) % progressEvery == 0 || index + 1 == totalChunks)
                        Log($"进度 {fileName} 分片 {index + 1}/{totalChunks}");
                }
            }

            // 3) complete
            var seal2 = Seal(pk.PublicKey, password);
            var completeBody = "{\"userName\":\"" + Esc(user) + "\",\"password\":\"" + Esc(seal2.Cipher) +
                               "\",\"fileId\":\"" + Esc(fileId) +
                               "\",\"timestamp\":" + DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() +
                               ",\"nonce\":\"" + Guid.NewGuid().ToString("N") + "\"}";
            var completeResp = await PostJson(server + "upload/complete", completeBody, seal2.Smkey, pk.KeyId).ConfigureAwait(false);
            Log($"complete ok file={fileName}");
        }

        private static async Task<string> PostJson(string url, string body, string smkey, string keyId)
        {
            using (var req = new HttpRequestMessage(HttpMethod.Post, url)
                   { Content = new StringContent(body, Encoding.UTF8, "application/json") })
            {
                req.Headers.TryAddWithoutValidation("smkey", smkey);
                req.Headers.TryAddWithoutValidation("smkeyid", keyId);
                var resp = await _http.SendAsync(req).ConfigureAwait(false);
                return await resp.Content.ReadAsStringAsync().ConfigureAwait(false);
            }
        }

        private static (string Smkey, string Cipher) Seal(string publicKey, string plain)
        {
            SmCrypto.SealPassword(publicKey, plain, out var smkey, out var cipher);
            return (smkey, cipher);
        }

        /// <summary>采样两次文件大小与修改时间，稳定（未被游戏写入）才返回 true。</summary>
        private static bool FileReady(string path)
        {
            for (var i = 0; i < 2; i++)
            {
                try
                {
                    var fi1 = new FileInfo(path);
                    Thread.Sleep(800);
                    var fi2 = new FileInfo(path);
                    return fi1.Length == fi2.Length && fi1.LastWriteTimeUtc == fi2.LastWriteTimeUtc;
                }
                catch (IOException) { Thread.Sleep(500); }
            }
            return false;
        }

        // ------------------------------------------------------------------
        // 工具
        // ------------------------------------------------------------------

        private static void Log(string message)
        {
            var line = $"[{DateTime.Now:yyyy-MM-dd HH:mm:ss}] {message}";
            try { _log.WriteLine(line); } catch { }
            Console.WriteLine(line);
        }

        private static void SafeDelete(string path)
        {
            try { File.Delete(path); } catch { /* 删除失败无碍，processed 集合已防重复 */ }
        }

        private static string GetArgValue(string[] args, string name)
        {
            for (var i = 0; i < args.Length - 1; i++)
            {
                if (string.Equals(args[i], name, StringComparison.OrdinalIgnoreCase)) return args[i + 1];
            }
            return null;
        }

        private static List<string> ExtractStringArray(string json, string field)
        {
            var list = new List<string>();
            var m = Regex.Match(json, "\"" + field + "\"\\s*:\\s*\\[(.*?)\\]", RegexOptions.Singleline);
            if (!m.Success) return list;
            foreach (Match item in Regex.Matches(m.Groups[1].Value, "\"((?:[^\"\\\\]|\\\\.)*)\""))
            {
                list.Add(Regex.Unescape(item.Groups[1].Value));
            }
            return list;
        }

        private static string ExtractString(string json, string field)
        {
            var m = Regex.Match(json, "\"" + field + "\"\\s*:\\s*\"((?:[^\"\\\\]|\\\\.)*)\"");
            return m.Success ? Regex.Unescape(m.Groups[1].Value) : null;
        }

        private static long ExtractLong(string json, string field)
        {
            var m = Regex.Match(json, "\"" + field + "\"\\s*:\\s*(-?\\d+)");
            long v;
            return m.Success && long.TryParse(m.Groups[1].Value, out v) ? v : long.MinValue;
        }

        private static string Esc(string s)
        {
            if (s == null) return "";
            var sb = new StringBuilder(s.Length + 8);
            foreach (var c in s)
            {
                switch (c)
                {
                    case '"': sb.Append("\\\""); break;
                    case '\\': sb.Append("\\\\"); break;
                    case '\b': sb.Append("\\b"); break;
                    case '\f': sb.Append("\\f"); break;
                    case '\n': sb.Append("\\n"); break;
                    case '\r': sb.Append("\\r"); break;
                    case '\t': sb.Append("\\t"); break;
                    default:
                        if (c < ' ') sb.Append("\\u").Append(((int)c).ToString("x4"));
                        else sb.Append(c);
                        break;
                }
            }
            return sb.ToString();
        }

        private static string Trim(string s, int max)
        {
            if (string.IsNullOrEmpty(s)) return "";
            return s.Length <= max ? s : s.Substring(0, max) + "…";
        }
    }
}
