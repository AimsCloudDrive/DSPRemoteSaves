using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Net.Http;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;
using DSPRemoteSaves;

namespace DSPSavesUploader
{
    /// <summary>
    /// DSP 云存档后台上传器：游戏退出时由插件启动的独立进程，
    /// 把未完成的存档清单继续上传到服务器。游戏退出不影响本进程。
    ///
    /// 用法：DSPSavesUploader.exe --task &lt;pending.json&gt;
    /// pending.json：{"server":"...","userName":"...","password":"...","logPath":"...","files":["...","..."]}
    /// 任务结束后该文件会被删除，日志写入 logPath。
    /// </summary>
    internal static class Program
    {
        private const int OverallTimeoutMinutes = 30;   // 整体自退保护
        private const int MaxAttemptsPerFile = 2;       // 单文件尝试次数
        private static Mutex _singleInstance;

        private static TextWriter _log = Console.Out;
        private static HttpClient _http;

        private static int Main(string[] args)
        {
            // 单实例：前一个实例还在上传时等待其结束，避免同文件并发产生多余版本
            _singleInstance = new Mutex(true, @"Global\DSPSavesUploader", out var createdNew);
            if (!createdNew)
            {
                try
                {
                    if (!_singleInstance.WaitOne(TimeSpan.FromMinutes(10)))
                    {
                        return 3; // 等待超时，放弃（下次游戏退出会重新派发）
                    }
                }
                catch (AbandonedMutexException) { /* 上个实例异常退出，接管 */ }
            }

            try { Console.OutputEncoding = Encoding.UTF8; } catch { }
            var overall = Stopwatch.StartNew();
            try
            {
                // 自检：SM3/SM4 标准向量 + 信封输出合法性（用于排查运行时差异）
            if (args.Any(a => a == "--selftest"))
            {
                var sm3 = SmCrypto.BytesToHex(SmCrypto.Sm3(Encoding.UTF8.GetBytes("abc")));
                var sm4 = SmCrypto.Sm4EcbEncryptHex(
                    "0123456789abcdeffedcba9876543210",
                    SmCrypto.HexToBytes("0123456789abcdeffedcba9876543210"));
                Log("SM3(abc)=" + sm3 + " " + (sm3 == "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0" ? "OK" : "MISMATCH"));
                Log("SM4[0..16]=" + sm4.Substring(0, 32) + " " + (sm4.StartsWith("681edf34d206965e86b3e94f536e4246") ? "OK" : "MISMATCH"));
                var pk = "04c9c5e4a23c2fe03d992298c0c29ffbb6b7b73bcce48275ee93b71b755b697200067529a96d4bd85e7951477ab5f97b7506351d719c91b35db5d876e11935b76b";

                System.Reflection.FieldInfo sboxField = typeof(SmCrypto).GetField("Sm4Sbox",
                    System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Static);
                Func<string> dumpSbox = () =>
                {
                    var arr = (byte[])sboxField.GetValue(null);
                    var sb = new StringBuilder();
                    for (int i = 0; i < 8; i++) sb.Append(arr[i].ToString("x2"));
                    return sb.ToString();
                };
                Log("Sbox BEFORE sm4 = " + dumpSbox());

                var sk2 = SmCrypto.Sm4EcbEncryptHex(
                    "0123456789abcdeffedcba9876543210",
                    SmCrypto.HexToBytes("0123456789abcdeffedcba9876543210"));
                Log("Sbox AFTER  sm4 = " + dumpSbox() + "  sm4=" + sk2.Substring(0, 32));

                SmCrypto.SealPassword(pk, "test-password", out var sk, out var cp);
                Log("Sbox AFTER  seal= " + dumpSbox());
                Log("smkey len=" + sk.Length + " hex=" + IsHex(sk));
                Log("cipher len=" + cp.Length + " hex=" + IsHex(cp) + " sample=" + cp.Substring(0, Math.Min(32, cp.Length)));
                return 0;
            }

            var taskPath = GetArgValue(args, "--task");
                if (string.IsNullOrEmpty(taskPath) || !File.Exists(taskPath))
                {
                    Console.WriteLine("用法: DSPSavesUploader.exe --task <pending.json>");
                    return 1;
                }

                var json = File.ReadAllText(taskPath);
                var server = ExtractString(json, "server");
                var user = ExtractString(json, "userName");
                var password = ExtractString(json, "password");
                var logPath = ExtractString(json, "logPath");
                var files = ExtractStringArray(json, "files");

                if (!string.IsNullOrEmpty(logPath))
                {
                    try
                    {
                        var dir = Path.GetDirectoryName(Path.GetFullPath(logPath));
                        if (!string.IsNullOrEmpty(dir)) Directory.CreateDirectory(dir);
                        var writer = new StreamWriter(logPath, append: true) { AutoFlush = true };
                        _log = writer;
                    }
                    catch { /* 日志不可用时继续用控制台 */ }
                }

                if (string.IsNullOrEmpty(server) || string.IsNullOrEmpty(user) || files.Count == 0)
                {
                    Log("任务清单不完整，放弃");
                    return 1;
                }

                Log($"===== 后台上传器启动 pid={Process.GetCurrentProcess().Id} 文件数={files.Count} =====");

                using (_http = new HttpClient { Timeout = TimeSpan.FromMinutes(15) })
                {
                    // task 里的 server 与插件配置同格式（不带路径），与插件一样自动拼接 API 前缀
                    server = server.TrimEnd('/') + "/dsp.saves/api/";
                    var publicKey = EnsurePublicKey(server).GetAwaiter().GetResult();

                    var ok = 0;
                    foreach (var file in files)
                    {
                        if (overall.Elapsed > TimeSpan.FromMinutes(OverallTimeoutMinutes))
                        {
                            Log("达到整体时限，剩余文件将由下次游戏退出重新派发");
                            break;
                        }
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
                                UploadOne(server, user, password, publicKey, file).GetAwaiter().GetResult();
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

                    Log($"===== 后台上传结束 成功 {ok}/{files.Count} 用时 {overall.Elapsed.TotalSeconds:0}s =====");
                }

                try { File.Delete(taskPath); } catch { /* 任务文件删除失败无碍 */ }
                return 0;
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
            var code = ExtractLong(body, "code");
            if (code != 0) throw new Exception("获取公钥失败: " + body);
            var info = new PublicKeyInfo
            {
                KeyId = ExtractString(body, "keyId"),
                PublicKey = ExtractString(body, "publicKey"),
            };
            if (string.IsNullOrEmpty(info.KeyId) || string.IsNullOrEmpty(info.PublicKey))
                throw new Exception("公钥响应格式异常: " + body);
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
            Log($"init ok file={fileName} size={size} chunks={totalChunks} (chunkSize={chunkSize})");

            // 2) 分片（uploadToken 头，流式读取避免大文件占内存）
            using (var fs = new FileStream(filePath, FileMode.Open, FileAccess.Read, FileShare.Read))
            {
                var buffer = new byte[chunkSize];
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

        // ------------------------------------------------------------------
        // 工具
        // ------------------------------------------------------------------

        private static void Log(string message)
        {
            var line = $"[{DateTime.Now:yyyy-MM-dd HH:mm:ss}] {message}";
            try { _log.WriteLine(line); } catch { }
            Console.WriteLine(line);
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

        private static string IsHex(string s)
        {
            if (string.IsNullOrEmpty(s) || s.Length % 2 != 0) return "BAD(length=" + (s?.Length ?? -1) + ")";
            foreach (var c in s)
            {
                var ok = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
                if (!ok) return "BAD(char='" + c + "')";
            }
            return "OK";
        }

        private static string Trim(string s, int max)
        {
            if (string.IsNullOrEmpty(s)) return "";
            return s.Length <= max ? s : s.Substring(0, max) + "…";
        }
    }
}
